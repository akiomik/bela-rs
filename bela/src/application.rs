use crate::context::Context;

/// A Bela application: user code driven by the audio system callbacks.
///
/// `setup` runs once before audio starts, `render` runs for every block
/// of frames on the real-time audio thread, and `cleanup` runs once
/// after audio stops.
///
/// # Safety
///
/// Implementing this trait is a promise that `render` is real-time
/// safe. On the audio thread it must not:
///
/// - allocate or free heap memory,
/// - block (locks, channels, sleeping) or make system calls (including
///   I/O; use `rt_printf` from `bela_sys` for debugging),
/// - panic in code paths that can actually be hit — a panic crossing
///   the callback boundary aborts the whole process.
///
/// `setup` and `cleanup` run outside the real-time context and are not
/// subject to these restrictions (panics still abort the process).
///
/// Implementors must be [`Send`]: the application is moved to the audio
/// thread after construction.
pub unsafe trait BelaApplication: Send {
    /// Called once before audio rendering starts. Return `false` to
    /// abort startup.
    fn setup(&mut self, _context: &mut Context) -> bool {
        true
    }

    /// Called once per block of frames on the real-time audio thread.
    fn render(&mut self, context: &mut Context);

    /// Called once after audio rendering stops.
    fn cleanup(&mut self, _context: &mut Context) {}
}

/// `extern "C"` shims installed into `BelaInitSettings`, bridging the C
/// callbacks to a `T: BelaApplication` reached through `userData`.
///
/// Safety contract shared by all three: `context` must satisfy
/// [`Context::from_mut_ptr`], and `user_data` must point to a live `T`
/// not accessed through any other reference during the call.
#[cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only called by the device-gated system module; still unit-tested on the host"
    )
)]
pub mod trampoline {
    use core::ffi::c_void;

    use bela_sys::BelaContext;

    use super::BelaApplication;
    use crate::context::Context;

    pub unsafe extern "C" fn setup<T: BelaApplication>(
        context: *mut BelaContext,
        user_data: *mut c_void,
    ) -> bool {
        let app = unsafe { &mut *user_data.cast::<T>() };
        let context = unsafe { Context::from_mut_ptr(context) };
        app.setup(context)
    }

    pub unsafe extern "C" fn render<T: BelaApplication>(
        context: *mut BelaContext,
        user_data: *mut c_void,
    ) {
        let app = unsafe { &mut *user_data.cast::<T>() };
        let context = unsafe { Context::from_mut_ptr(context) };
        app.render(context);
    }

    pub unsafe extern "C" fn cleanup<T: BelaApplication>(
        context: *mut BelaContext,
        user_data: *mut c_void,
    ) {
        let app = unsafe { &mut *user_data.cast::<T>() };
        let context = unsafe { Context::from_mut_ptr(context) };
        app.cleanup(context);
    }
}

#[cfg(test)]
mod tests {
    use core::ffi::c_void;
    use core::mem;

    use bela_sys::BelaContext;

    use super::*;

    #[derive(Default)]
    struct TestApp {
        setup_ok: bool,
        setup_calls: u32,
        render_calls: u32,
        cleanup_calls: u32,
        frames_seen: u32,
    }

    unsafe impl BelaApplication for TestApp {
        fn setup(&mut self, _context: &mut Context) -> bool {
            self.setup_calls += 1;
            self.setup_ok
        }

        fn render(&mut self, context: &mut Context) {
            self.render_calls += 1;
            self.frames_seen = context.as_sys().audioFrames;
        }

        fn cleanup(&mut self, _context: &mut Context) {
            self.cleanup_calls += 1;
        }
    }

    fn test_context() -> BelaContext {
        // A hand-built context standing in for the one libbela provides.
        let mut context: BelaContext = unsafe { mem::zeroed() };
        context.audioFrames = 64;
        context
    }

    #[test]
    fn trampolines_forward_to_the_application() {
        let mut context = test_context();
        let mut app = TestApp {
            setup_ok: true,
            ..TestApp::default()
        };
        let user_data = (&raw mut app).cast::<c_void>();

        unsafe {
            assert!(trampoline::setup::<TestApp>(&raw mut context, user_data));
            trampoline::render::<TestApp>(&raw mut context, user_data);
            trampoline::render::<TestApp>(&raw mut context, user_data);
            trampoline::cleanup::<TestApp>(&raw mut context, user_data);
        }

        assert_eq!(app.setup_calls, 1);
        assert_eq!(app.render_calls, 2);
        assert_eq!(app.cleanup_calls, 1);
        assert_eq!(app.frames_seen, 64);
    }

    #[test]
    fn setup_failure_is_reported() {
        let mut context = test_context();
        let mut app = TestApp::default();
        let user_data = (&raw mut app).cast::<c_void>();

        let ok = unsafe { trampoline::setup::<TestApp>(&raw mut context, user_data) };

        assert!(!ok);
        assert_eq!(app.setup_calls, 1);
    }

    #[test]
    fn default_setup_and_cleanup_are_provided() {
        struct RenderOnly;
        unsafe impl BelaApplication for RenderOnly {
            fn render(&mut self, _context: &mut Context) {}
        }

        let mut context = test_context();
        let mut app = RenderOnly;
        let user_data = (&raw mut app).cast::<c_void>();

        unsafe {
            assert!(trampoline::setup::<RenderOnly>(&raw mut context, user_data));
            trampoline::cleanup::<RenderOnly>(&raw mut context, user_data);
        }
    }
}
