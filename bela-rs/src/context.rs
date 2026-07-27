use bela_sys::BelaContext;

/// View over the [`BelaContext`] passed to the application callbacks.
///
/// Safe, real-time-friendly accessors (audio/analog/digital buffers,
/// frame counts, sample rates) will be added here; until then the
/// underlying struct is reachable through [`Context::as_sys`] and
/// [`Context::as_sys_mut`].
#[repr(transparent)]
pub struct Context(BelaContext);

impl Context {
    /// Reborrows a raw `BelaContext` pointer as a [`Context`].
    ///
    /// # Safety
    ///
    /// `ptr` must be non-null, properly aligned, and point to a live
    /// `BelaContext` that is not accessed through any other reference
    /// for the duration of `'a`.
    pub unsafe fn from_mut_ptr<'a>(ptr: *mut BelaContext) -> &'a mut Context {
        // repr(transparent) makes the cast sound.
        unsafe { &mut *ptr.cast::<Context>() }
    }

    /// Read access to the underlying `BelaContext`.
    pub fn as_sys(&self) -> &BelaContext {
        &self.0
    }

    /// Mutable access to the underlying `BelaContext`.
    ///
    /// # Safety
    ///
    /// The caller must not invalidate data the audio system relies on,
    /// e.g. by overwriting buffer pointers, frame counts or channel
    /// counts. Writing *through* the output buffer pointers is fine.
    pub unsafe fn as_sys_mut(&mut self) -> &mut BelaContext {
        &mut self.0
    }
}
