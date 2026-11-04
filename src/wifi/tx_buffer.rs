//! Initialized native lookahead storage without changing a frame's logical length.

const ETHERNET_HEADER_BYTES: usize = 14;
const MIN_NATIVE_PAYLOAD_BYTES: usize = 60;

pub(super) const fn payload_capacity(len: usize) -> Option<usize> {
    if len < ETHERNET_HEADER_BYTES {
        None
    } else if len < MIN_NATIVE_PAYLOAD_BYTES {
        Some(MIN_NATIVE_PAYLOAD_BYTES)
    } else {
        Some(len)
    }
}

/// Copy a logical Ethernet frame and initialize the native parser's lookahead.
///
/// # Safety
/// `source` must point to `len` initialized bytes, where `len >= 14`.
/// `destination` must point to writable storage of at least `max(len, 60)`
/// bytes, disjoint from `source`. The destination may
/// be uninitialized; no reference to its bytes is created before writing them.
pub(super) unsafe fn copy_frame(source: *const u8, destination: *mut u8, len: usize) {
    // Pinned SDK da7ce37be424d71874241ca9f17cf8da0e3cdb70:
    // ieee80211_output_pbuf peeks at byte 23 and conditionally bytes 34..37,
    // including for valid short EAPOL frames. Keep aio.len unchanged; this is
    // parser storage, not additional on-air payload. Reset reused fallback
    // storage too, so an earlier frame cannot supply those lookahead bytes.
    debug_assert!(len >= ETHERNET_HEADER_BYTES);
    let capacity = len.max(MIN_NATIVE_PAYLOAD_BYTES);
    unsafe {
        core::ptr::copy_nonoverlapping(source, destination, len);
        core::ptr::write_bytes(destination.add(len), 0, capacity - len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_frames_have_initialized_lookahead_without_extending_logical_bytes() {
        let source = [0x5a; crate::wifi_core::MAX_ETHERNET_FRAME_SIZE];
        for len in [14, 18, 37, 38, 42, 59, 60, source.len()] {
            let capacity = payload_capacity(len).unwrap();
            let mut destination = [0xa5; crate::wifi_core::MAX_ETHERNET_FRAME_SIZE + 1];
            // SAFETY: source contains len initialized bytes; the distinct
            // destination has capacity plus one guard byte.
            unsafe { copy_frame(source.as_ptr(), destination.as_mut_ptr(), len) };
            assert_eq!(&destination[..len], &source[..len]);
            assert!(destination[len..capacity].iter().all(|&byte| byte == 0));
            assert!(destination[capacity..].iter().all(|&byte| byte == 0xa5));
            assert_eq!(capacity, len.max(60));
        }
        for len in [0, 1, 13] {
            assert_eq!(payload_capacity(len), None);
        }
    }

    #[test]
    fn reused_fallback_does_not_expose_the_previous_frame_to_native_lookahead() {
        let long_frame = [0x7e; 256];
        let short_frame = [0x42; 18];
        let mut fallback = [0; 256];
        // SAFETY: both sources are initialized and disjoint from the fallback,
        // which fits each frame's required native capacity.
        unsafe {
            copy_frame(long_frame.as_ptr(), fallback.as_mut_ptr(), long_frame.len());
            copy_frame(
                short_frame.as_ptr(),
                fallback.as_mut_ptr(),
                short_frame.len(),
            );
        }
        assert_eq!(&fallback[..short_frame.len()], &short_frame);
        assert!(fallback[18..60].iter().all(|&byte| byte == 0));
        assert!(fallback[60..].iter().all(|&byte| byte == 0x7e));
    }
}
