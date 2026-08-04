use core::ffi::{c_int, c_void};
use core::marker::PhantomData;
use core::time::Duration;
use std::thread;

use crate::application::{BelaApplication, trampoline};
use crate::cpu;
use crate::error::Error;
use crate::settings::{self, Settings};
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
/// process-wide singleton.
///
/// Only available on the device target (`aarch64-unknown-linux-gnu`).
pub struct Bela<T: BelaApplication> {
    // Owned; boxed so the address handed to libbela stays stable, kept
    // raw so the audio thread's access is never aliased by a &mut.
    app: *mut T,
    started: bool,
    _marker: PhantomData<T>,
}

impl<T: BelaApplication> Bela<T> {
    /// Initialises the audio system with `application` and `settings`
    /// applied on top of `Bela_defaultSettings()`.
    ///
    /// # Errors
    /// Returns [`Error::Init`] when `Bela_initAudio` fails, e.g. when
    /// the audio hardware is unavailable or already in use,
    /// [`Error::ThreadCountUnsupported`] when more than one render
    /// thread is requested, and [`Error::CpuMonitoringCycle`] or
    /// [`Error::CpuMonitoring`] when
    /// [`Settings::cpu_monitoring`] asks for something libbela cannot
    /// serve.
    pub fn new(application: T, settings: &Settings) -> Result<Self, Error> {
        // Checked before anything is allocated or initialised, so a
        // cycle libbela cannot take costs nothing to reject.
        let monitoring = settings
            .cpu_monitoring_cycle()
            .map(cpu::check_cycle)
            .transpose()?;
        // Before Bela_initAudio, because the `setup` callback runs
        // inside that call and should already see the answer that
        // `Context::cpu_usage` will give for the rest of the run. The
        // priming tic this takes is what the audio thread's first
        // reading is measured from, so everything from here to
        // `start` is startup time that reading includes.
        cpu::apply_monitoring(monitoring)?;
        let app = Box::into_raw(Box::new(application));
        let ret = unsafe {
            let raw = bela_sys::Bela_InitSettings_alloc();
            bela_sys::Bela_defaultSettings(raw);
            settings.apply_to(&mut *raw);
            // Refuse what the safe API cannot serve rather than
            // initialising something unsound.
            if let Err(error) = settings::check_supported(&*raw) {
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
            _marker: PhantomData,
        })
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
    /// system fails to initialise or start.
    pub fn run(application: T, settings: &Settings) -> Result<(), Error> {
        let mut bela = Self::new(application, settings)?;
        let handler = request_stop_on_signal as extern "C" fn(c_int);
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            unsafe { libc::signal(signal, handler as libc::sighandler_t) };
        }
        bela.start()?;
        while !Self::stop_requested() {
            thread::sleep(Duration::from_millis(10));
        }
        bela.stop();
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
