//! Newlib's retained formatting and string routines use the driver allocator,
//! monotonic clock and permanent reentrancy storage. The driver exposes no C
//! file descriptors; descriptor syscalls preserve the SDK's `EBADF` result
//! without maintaining a VFS registry that can never contain an entry.
//!
//! ABI contracts follow ESP8266_RTOS_SDK's newlib and VFS adapters, Copyright
//! 2015-2019 Espressif Systems (Shanghai) PTE LTD, Apache-2.0. Their license is
//! retained in esp8266-wifi-sys/vendor/licenses/.

use core::ffi::c_void;

#[cfg(target_arch = "xtensa")]
extern "C" {
    static mut _global_impure_ptr: *mut u8;
}

/// Minimal newlib reent struct. `struct _reent` is 240 bytes on ESP8266's
/// newlib. Key fields:
///   offset 0x04: _stdin  (FILE*)
///   offset 0x08: _stdout (FILE*)
///   offset 0x0C: _stderr (FILE*)
///   offset 0x18: __sdidinit (int, 1 after installing complete streams)
///
/// The blob writes 32-bit pointers and ints into this storage. Keep it at
/// least 4-byte aligned so those raw stores cannot fault on layouts where a
/// plain `[u8; 240]` would land at an odd address.
#[repr(C, align(4))]
#[cfg(target_arch = "xtensa")]
struct ReentStorage([u8; 240]);

#[cfg(target_arch = "xtensa")]
static mut REENT_STRUCT: ReentStorage = ReentStorage([0; 240]);

/// Complete pinned-newlib FILE storage. Zero access flags make stdio return
/// EBADF before using buffers or callbacks. The 32-byte __sf_fake_* prefixes
/// are insufficient once __sdidinit is set: stdio accesses flags2 at byte100.
#[repr(C, align(4))]
#[cfg(target_arch = "xtensa")]
struct ClosedStream([u8; 104]);

#[cfg(target_arch = "xtensa")]
static mut CLOSED_STREAMS: [ClosedStream; 3] = [const { ClosedStream([0; 104]) }; 3];

/// Initialize `_global_impure_ptr` so that `__getreent()` returns a valid
/// (if minimal) reent struct. Must be called before any WiFi blob function.
#[cfg(target_arch = "xtensa")]
pub(crate) unsafe fn init_newlib_reent() {
    unsafe {
        REENT_STRUCT = ReentStorage([0; 240]);
        CLOSED_STREAMS = [const { ClosedStream([0; 104]) }; 3];
        let base = REENT_STRUCT.0.as_mut_ptr();
        let streams = core::ptr::addr_of_mut!(CLOSED_STREAMS).cast::<ClosedStream>();

        // _stdin at offset 0x04
        *(base.add(0x04) as *mut *mut ClosedStream) = streams;
        // _stdout at offset 0x08
        *(base.add(0x08) as *mut *mut ClosedStream) = streams.add(1);
        // _stderr at offset 0x0C
        *(base.add(0x0C) as *mut *mut ClosedStream) = streams.add(2);
        // Full streams are installed; prevent newlib's allocating __sinit.
        *(base.add(0x18) as *mut i32) = 1;

        _global_impure_ptr = base;
    }
}

#[cfg(target_arch = "xtensa")]
#[no_mangle]
pub unsafe extern "C" fn __getreent() -> *mut u8 {
    // SAFETY: runtime initialization installs permanent storage before native
    // calls. The retained newlib ABI uses this same pointer throughout.
    unsafe { _global_impure_ptr }
}

// The pinned Xtensa newlib uses 32-bit time_t and suseconds_t. Independent
// target-header assertions in tests/abi_layout.c verify this representation.
#[repr(C)]
struct Timeval {
    seconds: i32,
    microseconds: i32,
}

impl Timeval {
    fn from_micros(micros: u64) -> Self {
        Self {
            seconds: (micros / 1_000_000) as i32,
            microseconds: (micros % 1_000_000) as i32,
        }
    }
}

#[cfg(target_arch = "xtensa")]
#[no_mangle]
unsafe extern "C" fn _gettimeofday_r(
    _reent: *mut c_void,
    time: *mut Timeval,
    _timezone: *mut c_void,
) -> i32 {
    if !time.is_null() {
        let micros = super::port::esp_timer_get_time() as u64;
        // SAFETY: newlib supplies one writable timeval, or a null pointer.
        unsafe { time.write(Timeval::from_micros(micros)) };
    }
    0
}

unsafe fn bad_descriptor(reent: *mut c_void) -> i32 {
    // SAFETY: newlib passes its writable reentrancy object; errno is the first
    // i32, verified against the pinned target headers in tests/abi_layout.c.
    unsafe { reent.cast::<i32>().write(9) }; // EBADF
    -1
}

#[cfg_attr(target_arch = "xtensa", no_mangle)]
unsafe extern "C" fn _read_r(
    reent: *mut c_void,
    _fd: i32,
    _buffer: *mut c_void,
    _length: usize,
) -> isize {
    unsafe { bad_descriptor(reent) as isize }
}

#[cfg_attr(target_arch = "xtensa", no_mangle)]
unsafe extern "C" fn _write_r(
    reent: *mut c_void,
    _fd: i32,
    _buffer: *const c_void,
    _length: usize,
) -> isize {
    unsafe { bad_descriptor(reent) as isize }
}

#[cfg_attr(target_arch = "xtensa", no_mangle)]
unsafe extern "C" fn _close_r(reent: *mut c_void, _fd: i32) -> i32 {
    unsafe { bad_descriptor(reent) }
}

#[cfg_attr(target_arch = "xtensa", no_mangle)]
unsafe extern "C" fn _fstat_r(reent: *mut c_void, _fd: i32, _stat: *mut c_void) -> i32 {
    unsafe { bad_descriptor(reent) }
}

#[cfg_attr(target_arch = "xtensa", no_mangle)]
unsafe extern "C" fn _lseek_r(reent: *mut c_void, _fd: i32, _offset: i32, _whence: i32) -> i32 {
    unsafe { bad_descriptor(reent) }
}

#[cfg(target_arch = "xtensa")]
#[no_mangle]
extern "C" fn abort() -> ! {
    crate::wifi::sdk::runtime::panicHandler(core::ptr::null_mut(), 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_timeval_preserves_subseconds_and_long_uptime() {
        for (micros, seconds, remainder) in [
            (0, 0, 0),
            (999_999, 0, 999_999),
            (1_000_001, 1, 1),
            (4_294_967_296, 4294, 967_296),
            (2_147_483_647_999_999, i32::MAX, 999_999),
        ] {
            let time = Timeval::from_micros(micros);
            assert_eq!((time.seconds, time.microseconds), (seconds, remainder));
        }
    }

    #[test]
    fn native_descriptor_calls_fail_without_touching_io_buffers() {
        let mut storage = [0_i32, 0x1234_5678];
        let reent = storage.as_mut_ptr().cast();
        let mut buffer = [0x5a_u8; 8];
        for fd in [-1, 0, 1, 2, i32::MAX] {
            unsafe {
                assert_eq!(_read_r(reent, fd, buffer.as_mut_ptr().cast(), 8), -1);
                assert_eq!(storage[0], 9);
                storage[0] = 0;
                assert_eq!(_write_r(reent, fd, buffer.as_ptr().cast(), 8), -1);
                assert_eq!(storage[0], 9);
                storage[0] = 0;
                assert_eq!(_close_r(reent, fd), -1);
                assert_eq!(storage[0], 9);
                storage[0] = 0;
                assert_eq!(_fstat_r(reent, fd, buffer.as_mut_ptr().cast()), -1);
                assert_eq!(storage[0], 9);
                storage[0] = 0;
                assert_eq!(_lseek_r(reent, fd, 0, 0), -1);
                assert_eq!(storage[0], 9);
            }
            assert_eq!(storage[1], 0x1234_5678);
            assert_eq!(buffer, [0x5a; 8]);
        }
    }
}
