use core::ffi::{c_int, c_void};
use core::marker::PhantomData;
use core::time::Duration;
use std::ffi::OsStr;
use std::thread;

use bela_sys::BelaInitSettings;

use crate::application::{BelaApplication, trampoline};
use crate::cmdline::{self, Arguments};
use crate::cpu;
use crate::error::Error;
use crate::settings::{self, Settings};
use crate::singleton::Claim;
use crate::task;

/// Owns an initialised Bela audio system and the application driven by
/// it.
///
/// Construction initialises the audio system (`Bela_initAudio`);
/// dropping the value stops audio if needed, runs `cleanup`
/// (`Bela_cleanupAudio`) and frees the application. For the common
/// "run until stopped" case, use [`Bela::run`].
///
/// Only one `Bela` may exist at a time: the underlying C API is a
/// process-wide singleton. A second [`new`](Bela::new) fails with
/// [`Error::AudioSystemExists`] rather than reaching into globals the
/// first one is using — from this thread or any other.
///
/// Only available on the device target (`aarch64-unknown-linux-gnu`).
pub struct Bela<T: BelaApplication> {
    // Owned; boxed so the address handed to libbela stays stable, kept
    // raw so the audio thread's access is never aliased by a &mut.
    app: *mut T,
    started: bool,
    /// Released once this audio system is gone, so the next one can be
    /// built. Declared last: fields drop after `Drop::drop`, so the
    /// claim outlives the teardown that runs there.
    _claim: Claim,
    _marker: PhantomData<T>,
}

impl<T: BelaApplication> Bela<T> {
    /// Initialises the audio system with `application` and `settings`
    /// applied on top of `Bela_defaultSettings()`.
    ///
    /// # Errors
    /// Returns [`Error::AudioSystemExists`] when another audio system
    /// is alive in this process, [`Error::Init`] when `Bela_initAudio`
    /// fails, e.g. when the audio hardware is unavailable or already in
    /// use, [`Error::ThreadCountUnsupported`] when more than one render
    /// thread is requested, and [`Error::CpuMonitoringCycle`],
    /// [`Error::CpuMonitoringPeriodSize`] or [`Error::CpuMonitoring`]
    /// when [`Settings::cpu_monitoring`] asks for something that cannot
    /// be served.
    ///
    /// # A failed initialisation is fatal to the process
    ///
    /// [`Error::Init`] means `Bela_initAudio` failed partway through,
    /// and libbela keeps no record of how far it got. Whatever the
    /// attempt had already taken is still held — up to and including
    /// the audio hardware, and the CPU monitoring counters this crate
    /// turns on just before the call when [`Settings::cpu_monitoring`]
    /// asked for them — and this crate does not call
    /// `Bela_cleanupAudio` to hand any of it back, because on that path
    /// the call itself segfaults; see "Audio thread" in
    /// `docs/board-facts.md`.
    ///
    /// The process-wide claim is released on the way out, so a second
    /// `Bela::new` is allowed to run. It will not work: on a Bela Gem
    /// it reports `Mcasp::start() called while already running`, fails
    /// to allocate its pipes and then segfaults, in every run measured.
    ///
    /// What is poisoned is this process, not the board. A new process
    /// gets a working audio system straight away, with nothing to reset
    /// in between — so treat this error as a reason to exit, and leave
    /// retrying to whatever started the program.
    ///
    /// The ordinary way to arrive here is a
    /// [`setup`](BelaApplication::setup) callback returning `false`,
    /// which fails the initialisation after the hardware is up.
    pub fn new(application: T, settings: &Settings) -> Result<Self, Error> {
        Self::init(application, settings, None)
    }

    /// Initialises the audio system like [`new`](Bela::new), with
    /// Bela's standard command-line options applied on top of
    /// `settings`.
    ///
    /// `args` is the whole argument list with the program name first,
    /// as [`std::env::args_os()`] yields it; parsing starts at the
    /// second entry, the way a C `main` does. The options are the ones
    /// every other way of writing a Bela program accepts — `--period`,
    /// `--verbose`, `--use-analog` and the rest, printed by
    /// [`print_usage`](crate::print_usage) — and they are applied last,
    /// so the application keeps the defaults it was built with but can
    /// still be reconfigured without rebuilding.
    ///
    /// Options of the program's own are not part of the set: parse them
    /// first, with whatever argument parser the program already uses,
    /// and hand on what is left. See
    /// [`examples/command_line.rs`][example]. An argument that is not
    /// an option at all is ignored — `getopt` moves those to the end of
    /// the list and never reports them, which is also what Bela's C
    /// program templates do with them.
    ///
    /// # Errors
    /// In addition to the errors [`new`](Bela::new) returns:
    /// [`Error::CommandLine`] when an argument is not one of the
    /// standard options, is missing its value or is otherwise rejected,
    /// and [`Error::CommandLineNul`] when an argument contains a NUL
    /// byte.
    ///
    /// [example]: https://github.com/akiomik/bela-rs/blob/main/bela/examples/command_line.rs
    pub fn new_with_args<I, S>(application: T, settings: &Settings, args: I) -> Result<Self, Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        // Before the claim: copying the arguments touches nothing an
        // audio system owns, and a list C cannot represent is worth
        // refusing whether or not one exists.
        let arguments = Arguments::new(args)?;
        Self::init(application, settings, Some(arguments))
    }

    /// Initialises the audio system, optionally letting `arguments`
    /// have the last word on the settings.
    fn init(
        application: T,
        settings: &Settings,
        mut arguments: Option<Arguments>,
    ) -> Result<Self, Error> {
        // First, because everything below reaches into libbela's
        // globals — including, before `Bela_initAudio` is called at
        // all, the CPU monitoring counters an audio system that is
        // already running would be writing.
        let claim = Claim::take()?;
        // Checked before anything is allocated, so a cycle libbela
        // cannot take costs nothing to reject.
        let monitoring = settings
            .cpu_monitoring_cycle()
            .map(cpu::check_cycle)
            .transpose()?;
        let app = Box::into_raw(Box::new(application));
        let ret = unsafe {
            let raw = bela_sys::Bela_InitSettings_alloc();
            bela_sys::Bela_defaultSettings(raw);
            settings.apply_to(&mut *raw);
            // The command line last, so it overrides the defaults the
            // application was built with rather than the other way
            // around: being able to reconfigure a binary from outside
            // is the whole point of it.
            //
            // Then refuse what the safe API cannot serve rather than
            // initialising something unsound, and turn monitoring on —
            // both once the settings are fully resolved, including
            // whatever the command line just changed, but before
            // `Bela_initAudio`, since the `setup` callback runs inside
            // that call and should already see the answer
            // `Context::cpu_usage` will give for the rest of the run.
            // The priming tic it takes is what the audio thread's first
            // reading is measured from, so everything from here to
            // `start` is startup time that reading includes.
            let prepared = arguments
                .as_mut()
                .map_or(Ok(()), |arguments| cmdline::parse(arguments, &mut *raw))
                .and_then(|()| Self::check_supported(&*raw, monitoring))
                .and_then(|()| cpu::apply_monitoring(monitoring));
            if let Err(error) = prepared {
                bela_sys::Bela_InitSettings_free(raw);
                drop(Box::from_raw(app));
                return Err(error);
            }
            (*raw).setup = Some(trampoline::setup::<T>);
            (*raw).render = Some(trampoline::render::<T>);
            (*raw).cleanup = Some(trampoline::cleanup::<T>);
            let ret = bela_sys::Bela_initAudio(raw, app.cast::<c_void>());
            bela_sys::Bela_InitSettings_free(raw);
            ret
        };
        if ret != 0 {
            // The audio system never took ownership of the callbacks.
            drop(unsafe { Box::from_raw(app) });
            return Err(Error::Init(ret));
        }
        Ok(Self {
            app,
            started: false,
            _claim: claim,
            _marker: PhantomData,
        })
    }

    /// Checks the resolved settings against what this crate can serve.
    fn check_supported(raw: &BelaInitSettings, monitoring: Option<c_int>) -> Result<(), Error> {
        settings::check_supported(raw)?;
        if monitoring.is_some() {
            // Needs the resolved period size: unset in `Settings` means
            // Bela's default, not "no period size".
            cpu::check_period_size(raw.periodSize)?;
        }
        Ok(())
    }

    /// Starts the real-time audio thread.
    ///
    /// # Errors
    /// Returns [`Error::Start`] when `Bela_startAudio` fails.
    pub fn start(&mut self) -> Result<(), Error> {
        if self.started {
            return Ok(());
        }
        let ret = unsafe { bela_sys::Bela_startAudio() };
        if ret != 0 {
            return Err(Error::Start(ret));
        }
        self.started = true;
        Ok(())
    }

    /// Stops the real-time audio thread. Also happens on drop.
    ///
    /// Auxiliary tasks do not survive this: the handles an application
    /// holds are retired, and creating one while it runs fails with
    /// [`Error::TaskCreateWhileStopping`].
    pub fn stop(&mut self) {
        if self.started {
            task::teardown(|| self.stop_audio());
        }
    }

    /// Stops audio if it is running, without touching the task
    /// lifecycle; callers do that around it.
    fn stop_audio(&mut self) {
        if self.started {
            unsafe { bela_sys::Bela_stopAudio() };
            self.started = false;
        }
    }

    /// Whether a stop has been requested (stop button, IDE, or
    /// [`Bela::request_stop`]).
    #[must_use]
    pub fn stop_requested() -> bool {
        unsafe { bela_sys::Bela_stopRequested() != 0 }
    }

    /// Requests the audio system to stop, e.g. from a signal handler
    /// or auxiliary thread.
    pub fn request_stop() {
        unsafe { bela_sys::Bela_requestStop() }
    }

    /// Initialises and starts the audio system, blocks until a stop is
    /// requested, then shuts down.
    ///
    /// Installs SIGINT/SIGTERM/SIGHUP handlers that request a stop, so
    /// Ctrl-C, `systemctl stop` and a dropped ssh connection all shut
    /// down cleanly (mirroring the C example templates).
    ///
    /// Note that Ctrl-C only reaches the program when ssh allocates a
    /// terminal (`ssh -t`); otherwise it just kills the local client
    /// and leaves the program running on the board.
    ///
    /// # Errors
    /// Returns [`Error::Init`] or [`Error::Start`] when the audio
    /// system fails to initialise or start. What an [`Error::Init`]
    /// leaves behind is described on [`new`](Bela::new); it is fatal to
    /// the process here too.
    pub fn run(application: T, settings: &Settings) -> Result<(), Error> {
        Self::new(application, settings)?.until_stopped()
    }

    /// Runs like [`run`](Bela::run), with Bela's standard command-line
    /// options applied on top of `settings`.
    ///
    /// See [`new_with_args`](Bela::new_with_args) for what they are and
    /// where they sit.
    ///
    /// # Errors
    /// The errors of [`new_with_args`](Bela::new_with_args), plus
    /// [`Error::Start`] when the audio system fails to start.
    pub fn run_with_args<I, S>(application: T, settings: &Settings, args: I) -> Result<(), Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Self::new_with_args(application, settings, args)?.until_stopped()
    }

    /// Starts the audio system, blocks until a stop is requested, then
    /// shuts down — [`run`](Bela::run) without the construction.
    ///
    /// This is the way to run an audio system that had something said
    /// to it between [`new`](Bela::new) and the run loop, rather than
    /// reimplementing the loop and its signal handling to get that
    /// window. Setting a level is the case it exists for:
    ///
    /// ```no_run
    /// use bela::{Bela, Channel, Settings};
    /// # use bela::{BelaApplication, Context};
    /// # struct App;
    /// # unsafe impl BelaApplication for App {
    /// #     fn render(&mut self, _context: &mut Context) {}
    /// # }
    ///
    /// fn main() -> Result<(), bela::Error> {
    ///     let mut bela = Bela::new(App, &Settings::new())?;
    ///     bela.set_audio_input_gain(Channel::All, 30.0)?;
    ///     bela.until_stopped()
    /// }
    /// ```
    ///
    /// Installs the same SIGINT/SIGTERM/SIGHUP handlers
    /// [`run`](Bela::run) does; see it for what they mean over ssh.
    ///
    /// # Errors
    /// Returns [`Error::Start`] when the audio system fails to start.
    pub fn until_stopped(mut self) -> Result<(), Error> {
        let handler = request_stop_on_signal as extern "C" fn(c_int);
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            unsafe { libc::signal(signal, handler as libc::sighandler_t) };
        }
        self.start()?;
        while !Self::stop_requested() {
            thread::sleep(Duration::from_millis(10));
        }
        self.stop();
        Ok(())
    }
}

// Async-signal-safe: Bela_requestStop only sets a flag.
extern "C" fn request_stop_on_signal(_signal: c_int) {
    unsafe { bela_sys::Bela_requestStop() }
}

impl<T: BelaApplication> Drop for Bela<T> {
    fn drop(&mut self) {
        // One teardown window over the whole shutdown, including the
        // cleanup callback and the case where audio was never started:
        // each Bela version deletes the auxiliary tasks somewhere in
        // here, and no handle may look live while that happens.
        task::teardown(|| {
            self.stop_audio();
            // Runs the cleanup callback, which still borrows the app.
            unsafe { bela_sys::Bela_cleanupAudio() };
        });
        // The app is only freed once the callback can no longer run.
        drop(unsafe { Box::from_raw(self.app) });
    }
}
