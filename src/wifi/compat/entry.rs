//! One-time newlib initialization for the pinned SDK ABI.

extern "C" {
    static mut _global_impure_ptr: *mut u8;
}

/// Minimal newlib reent struct. `struct _reent` is 240 bytes on ESP8266's
/// newlib. Key fields:
///   offset 0x04: _stdin  (FILE*)
///   offset 0x08: _stdout (FILE*)
///   offset 0x0C: _stderr (FILE*)
///   offset 0x18: __sdidinit (int, must be 1)
///
/// The blob writes 32-bit pointers and ints into this storage. Keep it at
/// least 4-byte aligned so those raw stores cannot fault on layouts where a
/// plain `[u8; 240]` would land at an odd address.
#[repr(C, align(4))]
struct ReentStorage([u8; 240]);

static mut REENT_STRUCT: ReentStorage = ReentStorage([0; 240]);

/// Initialize `_global_impure_ptr` so that `__getreent()` returns a valid
/// (if minimal) reent struct. Must be called before any WiFi blob function.
pub(crate) unsafe fn init_newlib_reent() {
    unsafe {
        extern "C" {
            static __sf_fake_stdin: u8;
            static __sf_fake_stdout: u8;
            static __sf_fake_stderr: u8;
        }

        REENT_STRUCT = ReentStorage([0; 240]);
        let base = REENT_STRUCT.0.as_mut_ptr();

        // _stdin at offset 0x04
        *(base.add(0x04) as *mut *const u8) = &__sf_fake_stdin;
        // _stdout at offset 0x08
        *(base.add(0x08) as *mut *const u8) = &__sf_fake_stdout;
        // _stderr at offset 0x0C
        *(base.add(0x0C) as *mut *const u8) = &__sf_fake_stderr;
        // __sdidinit at offset 0x18 — must be 1
        *(base.add(0x18) as *mut i32) = 1;

        _global_impure_ptr = base;
    }
}
