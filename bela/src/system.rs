use core::ffi::c_int;
use core::fmt;
use core::marker::PhantomData;
use core::time::Duration;
use std::ffi::OsStr;
use std::thread;

use bela_sys::BelaInitSettings;

use crate::application::BelaApplication;
use crate::cmdline::{self, Arguments};
use crate::cpu;
use crate::error::Error;
use crate::runtime::{Runtime, trampoline, user_data};
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
/// One at a time, and in some processes none at all: once a
/// `Bela_initAudio` has failed here, every later [`new`](Bela::new)
/// fails with [`Error::AudioSystemPoisoned`], which that method
/// explains.
///
/// Only available on the device target (`aarch64-unknown-linux-gnu`).
pub struct Bela<T: BelaApplication> {
    // Owned; boxed so the address handed to libbela stays stable, kept
    // raw so the audio threads' access is never aliased by a &mut.
    runtime: *mut Runtime<T>,
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
    /// is alive in this process, [`Error::AudioSystemPoisoned`] when an
    /// earlier initialisation in this process failed, [`Error::Init`]
    /// when `Bela_initAudio` fails, e.g. when the audio hardware is
    /// unavailable or already in use, and [`Error::CpuMonitoringCycle`],
    /// [`Error::CpuMonitoringPeriodSize`] or [`Error::CpuMonitoring`]
    /// when [`Settings::cpu_monitoring`] asks for something that cannot
    /// be served.
    ///
    /// Before any of that it returns [`Error::SampleRate`],
    /// [`Error::PruNumber`] or one of the four
    /// `Error::Multiplexer…` variants for a resolved configuration
    /// libbela would refuse inside `Bela_initAudio`, or not refuse at
    /// all and leave to the PRU firmware. These are as reachable from
    /// here as from [`new_with_args`](Bela::new_with_args), because the
    /// board's own `CL=` line in `~/.bela/belaconfig` goes through
    /// Bela's parser inside `Bela_defaultSettings`; the checks are
    /// listed there.
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
    /// the call itself segfaults. That is measured rather than assumed,
    /// including in the order this method would have to make the call;
    /// see "Audio thread" in `docs/board-facts.md`.
    ///
    /// So the process-wide claim is released as unusable rather than
    /// free, and every later `Bela::new` in this process fails with
    /// [`Error::AudioSystemPoisoned`] without touching libbela. That
    /// refusal is the whole of what this crate can do about it: going
    /// ahead is not a worse-behaved audio system but a segfault, on a
    /// Bela Gem reported as `Mcasp::start() called while already
    /// running` in every run measured.
    ///
    /// What is unusable is this process, not the board. A new process
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
    /// # What is checked before the audio system is built
    ///
    /// Six combinations are refused once the defaults, `settings` and
    /// the command line have all been applied, and before
    /// `Bela_initAudio` is called:
    ///
    /// | resolved settings | error |
    /// |---|---|
    /// | a sample rate of 0, which is what `--sample-rate` gives for anything `atof` cannot read and what a negative rate is clamped to | [`Error::SampleRate`] |
    /// | `--pru-number` other than 0 or 1 | [`Error::PruNumber`] |
    /// | `--mux-channels` other than 0, 2, 4 or 8 | [`Error::MultiplexerChannels`] |
    /// | `--mux-channels` with `--pru-number 0` | [`Error::MultiplexerPru`] |
    /// | `--mux-channels` with `--use-analog 0` | [`Error::MultiplexerWithoutAnalog`] |
    /// | `--mux-channels` with a number of analog input channels other than 8 | [`Error::MultiplexerAnalogChannels`] |
    ///
    /// None of them is a working configuration that has been taken
    /// away. Each was measured on a Gem Stereo failing in a place that
    /// costs the caller more than an error does.
    ///
    /// Five of the six fail inside `Bela_initAudio` — the sample rate
    /// in `Bela_getHwConfigPrivate`, the two PRU rules in `RTAudio.cpp`'s
    /// initial sanity checks, and the multiplexer channel and analog
    /// input counts in `PRU::initialise` — and what that costs is not
    /// the attempt but the process: no audio system can be built after
    /// it, as [`new`](Bela::new) describes.
    ///
    /// The sixth, the multiplexer with the analog inputs off, is the
    /// one libbela does not check at all: its count rules sit behind an
    /// `if` that analog being off skips, so the settings reach the PRU
    /// firmware, which gives up — `Invalid PRU configuration settings`,
    /// `PRU timeout`, `McASP error, abort` — and ends the process from
    /// inside libbela with nothing returned to anyone.
    ///
    /// # What is passed through
    ///
    /// Everything else the C runtime accepts, including the options
    /// this crate has no other way of asking for.
    ///
    /// `--mux-channels 2`, `4` and `8` are among them: a Gem brings the
    /// multiplexer up and the PRU fills the demultiplexed buffer, and
    /// no accessor here reaches it — `multiplexerAnalogRead` and
    /// `multiplexerChannelForFrame` are left out on purpose, because
    /// the Capelet cannot be attached to the board this crate is
    /// measured against. So the option takes effect and its readings
    /// stay out of reach. That is a gap in what this crate offers
    /// rather than a reason to refuse the flag.
    ///
    /// libbela also reshapes several values rather than refusing them,
    /// and this crate does not copy those rules — a copy would drift
    /// from the library it describes:
    ///
    /// - `--analog-channels` snaps to 8, 4 or 2. `-C 0` and `-C 3` both
    ///   give **2** analog inputs, not none and not three; `--use-analog 0`
    ///   is how to have none.
    /// - `--digital-channels` clamps to 16, and 0 turns the digital
    ///   channels off altogether.
    /// - `--board` naming hardware that is not there is ignored, an
    ///   unrecognised name included.
    /// - `--stop-button-pin` out of range runs on without a working
    ///   stop button, and `--codec-mode`, `--disabled-digital-channels`
    ///   and the audio expander options are accepted and do nothing
    ///   visible on a Gem.
    ///
    /// Two period sizes have no check either, in neither direction: a
    /// Gem Stereo with its eight analog inputs cannot keep up with
    /// `--period 1` or `--period 3` and dies in the PRU as above, while
    /// 2 and everything from 4 up run — and both failures move as soon
    /// as the analog configuration does, so there is no floor a check
    /// could hold. At the other end, a period of 256 frames or more
    /// leaves the digital pins dead while everything reports success;
    /// see [`Settings::period_size`].
    ///
    /// # A malformed `--json-string` ends the process
    ///
    /// `--json-string {` throws an uncaught `nlohmann::json` exception
    /// and the process ends on `SIGABRT`. It happens inside the parse —
    /// `Bela_getopt_long`, which this method calls — and so before
    /// `Bela_initAudio` and before the checks above. There is nothing
    /// to return and nothing to catch: the call this crate made does
    /// not come back.
    ///
    /// Refusing the option here would not close that path and would
    /// take away valid JSON, which works: `Bela_defaultSettings` runs
    /// the board's own `CL=` line from `~/.bela/belaconfig` through the
    /// same parser, so the same abort is reachable from
    /// [`new`](Bela::new), which looks at no arguments at all.
    /// The neighbouring option is better behaved — `--json-file`
    /// naming a missing file warns and carries on with the settings it
    /// already had.
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
        let mut claim = Claim::take()?;
        // Checked before anything is allocated, so a cycle libbela
        // cannot take costs nothing to reject.
        let monitoring = settings
            .cpu_monitoring_cycle()
            .map(cpu::check_cycle)
            .transpose()?;
        let (ret, runtime) = unsafe {
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
            // `SetupContext::cpu_usage` will give for the rest of the
            // run. The priming tic it takes is what the audio thread's
            // first reading is measured from, so everything from here
            // to `start` is startup time that reading includes.
            let prepared = arguments
                .as_mut()
                .map_or(Ok(()), |arguments| cmdline::parse(arguments, &mut *raw))
                .and_then(|()| Self::check_supported(&*raw, monitoring))
                .and_then(|()| cpu::apply_monitoring(monitoring));
            if let Err(error) = prepared {
                bela_sys::Bela_InitSettings_free(raw);
                return Err(error);
            }
            // Built here rather than earlier, because how many render
            // states it needs is a resolved setting like any other:
            // `--thread-count` on the command line has just had its
            // say.
            let runtime = Box::into_raw(Box::new(Runtime::new(
                application,
                settings::render_threads(&*raw),
            )));
            (*raw).setup = Some(trampoline::setup::<T>);
            (*raw).render_pre = Some(trampoline::render_pre::<T>);
            (*raw).render = Some(trampoline::render::<T>);
            (*raw).render_post = Some(trampoline::render_post::<T>);
            (*raw).cleanup = Some(trampoline::cleanup::<T>);
            let ret = bela_sys::Bela_initAudio(raw, user_data(runtime));
            bela_sys::Bela_InitSettings_free(raw);
            (ret, runtime)
        };
        if ret != 0 {
            // The audio system never took ownership of the callbacks.
            drop(unsafe { Box::from_raw(runtime) });
            // Every failure above this point leaves libbela in a state
            // the next attempt can resolve — CPU monitoring can be left
            // initialised, but the next `new` applies or disables it
            // either way. This one cannot be resolved: `Bela_initAudio`
            // got partway and no call undoes it, so the claim is
            // released as unusable rather than free and the next `new`
            // is refused instead of segfaulting.
            claim.poison();
            return Err(Error::Init(ret));
        }
        Ok(Self {
            runtime,
            started: false,
            _claim: claim,
            _marker: PhantomData,
        })
    }

    /// Checks the resolved settings against what this crate can serve.
    fn check_supported(raw: &BelaInitSettings, monitoring: Option<c_int>) -> Result<(), Error> {
        // First, because these are the settings that cost the process
        // rather than the attempt: they fail inside `Bela_initAudio`,
        // or after it in the PRU firmware, and neither leaves anything
        // to report with.
        settings::check_resolved(raw)?;
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

    /// How many callbacks this audio system has refused, while it was
    /// running, for breaking the protocol its render states rely on.
    ///
    /// Zero for every run that behaved. Anything else means libbela
    /// made a callback somewhere the crate could not hand out the
    /// references [`BelaApplication`] promises — several `render` calls
    /// with the same thread number, say. The callback was skipped and a
    /// stop requested, so this is a reason a run ended, not damage that
    /// can be undone.
    ///
    /// Refusals *during* a shutdown are not counted here, and are not a
    /// fault in the same sense: libbela abandons the block it is in
    /// when a stop arrives, which can leave a `render_post` overlapping
    /// a `render` that has not finished. Refusing that is the guard
    /// working. Keeping the two apart is what lets an ordinary Ctrl-C
    /// stay an ordinary Ctrl-C, and
    /// [`until_stopped`](Bela::until_stopped) reports the count for
    /// those separately rather than failing on it.
    ///
    /// [`until_stopped`](Bela::until_stopped) reads this for you and
    /// fails with [`Error::CallbackFaults`]; this is for a program
    /// driving [`start`](Bela::start) and [`stop`](Bela::stop) itself.
    #[must_use]
    pub fn callback_faults(&self) -> u32 {
        // Safety: the runtime is alive for as long as `self` is, and
        // reading the counter is an atomic load.
        unsafe { &*self.runtime }.faults()
    }

    /// How many callbacks this audio system has refused *during* a
    /// shutdown, which is a different thing from
    /// [`callback_faults`](Bela::callback_faults) and not a failure.
    ///
    /// libbela abandons the block it is in when a stop arrives: the
    /// secondary render threads check the stop flag just before calling
    /// `render`, and the main thread stops waiting for them on the same
    /// flag, so a `render_post` can arrive while a `render` is still
    /// finishing. Refusing that is the guard doing its job.
    ///
    /// Non-zero means the last block was cut short — some of its frames
    /// were never rendered, and its `render_post` may not have run.
    /// That is worth knowing for a program that counts frames rather
    /// than blocks, which is why it can be asked.
    /// [`until_stopped`](Bela::until_stopped) reports it on the console
    /// instead; this is for a program driving [`start`](Bela::start) and
    /// [`stop`](Bela::stop) itself, which has no other way to find out.
    #[must_use]
    pub fn callback_faults_while_stopping(&self) -> u32 {
        // Safety: the runtime is alive for as long as `self` is, and
        // reading the counter is an atomic load.
        unsafe { &*self.runtime }.faults_while_stopping()
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
    /// the process here too. Returns [`Error::CallbackFaults`] when the
    /// run ended because a callback was refused, so `Ok(())` means the
    /// run was stopped by someone asking it to.
    pub fn run(application: T, settings: &Settings) -> Result<(), Error> {
        Self::new(application, settings)?.until_stopped()
    }

    /// Runs like [`run`](Bela::run), with Bela's standard command-line
    /// options applied on top of `settings`.
    ///
    /// See [`new_with_args`](Bela::new_with_args) for what they are,
    /// where they sit, which of them are checked before the audio
    /// system is built and which are passed on to libbela as they
    /// stand.
    ///
    /// # Errors
    /// The errors of [`new_with_args`](Bela::new_with_args), plus the
    /// [`Error::Start`] and [`Error::CallbackFaults`] of
    /// [`run`](Bela::run).
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
    /// # use bela::{BelaApplication, RenderContext, SetupContext, ThreadInfo};
    /// # struct App;
    /// # impl BelaApplication for App {
    /// #     type RenderState = ();
    /// #     fn create_render_state(&mut self, _t: ThreadInfo, _c: &SetupContext) {}
    /// #     fn render(&self, _s: &mut (), _c: &mut RenderContext) {}
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
    /// Returns [`Error::Start`] when the audio system fails to start,
    /// and [`Error::CallbackFaults`] when the run ended because a
    /// callback was refused *while it was running* — a stop asked for
    /// by the crate rather than by anyone else, which `Ok(())` would
    /// otherwise hide.
    ///
    /// A callback refused during the shutdown itself is not that, and
    /// does not fail this: libbela abandons the block it is in when a
    /// stop arrives, and refusing a `render_post` that overlaps a
    /// `render` still finishing is the guard doing its job on an
    /// ordinary Ctrl-C. Those are reported on the console instead, and
    /// mean the last block may be short. See
    /// [`callback_faults`](Bela::callback_faults).
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
        // Read once audio has stopped, so that every render, render_pre
        // and render_post of the run has been counted. The `cleanup`
        // callback runs later, in the drop below, and so is not covered
        // by either number — nothing here could report it, since the
        // counters go with the runtime it is dropped along with. What
        // it could be refused for is a claim it cannot take, a thread
        // count that disagrees, or states that were never built; the
        // render threads are joined by now and the other two were
        // settled in `setup`, so none of them can happen on this path.
        //
        let while_stopping = self.callback_faults_while_stopping();
        if while_stopping != 0 {
            // Not a failure: this is libbela abandoning the block it
            // was in when the stop arrived, and the guard declining to
            // hand out references over the top of it. Said out loud all
            // the same, because an application that counts frames will
            // see the block go missing.
            crate::rt_println!(
                "bela: {while_stopping} callback(s) were refused while stopping, which is how a \
                 block in flight is abandoned; the last block may be short"
            );
        }
        match self.callback_faults() {
            0 => Ok(()),
            faults => Err(Error::CallbackFaults(faults)),
        }
    }
}

/// What can be said about an audio system without disturbing it:
/// whether it is running, and the two callback fault counts.
///
/// Written by hand rather than derived so that an application type
/// does not have to be [`Debug`] for the handle that owns it to be —
/// nothing of the application is printed. The rest of the state is the
/// pointer to the runtime and the process-wide claim, neither of which
/// says anything a reader of a debug line could use.
impl<T: BelaApplication> fmt::Debug for Bela<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bela")
            .field("started", &self.started)
            .field("callback_faults", &self.callback_faults())
            .field(
                "callback_faults_while_stopping",
                &self.callback_faults_while_stopping(),
            )
            .finish_non_exhaustive()
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
        drop(unsafe { Box::from_raw(self.runtime) });
    }
}
