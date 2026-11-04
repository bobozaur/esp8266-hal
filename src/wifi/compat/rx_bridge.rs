//! Rust implementation of the SDK receive-buffer delivery boundary.
//!
//! The original ESP8266 object rejects all copied frames below a fixed free-
//! heap threshold. Static driver storage makes that threshold unnecessarily
//! conservative, so this implementation admits a copy whenever the concrete
//! frame plus a small allocator margin fits. The SDK's zero-copy fast path and
//! buffer-ownership contract remain unchanged.

use core::{ffi::c_void, ptr};

const PACKET_DATA_OFFSET: usize = 12;
const PACKET_LENGTH_OFFSET: usize = 26;
const RX_BUFFER_LIMIT_OFFSET: usize = 16;
const SDK_HEAP_ADMISSION_BYTES: u32 = 0x1800;
const ALLOCATOR_HEADER_BYTES: u32 = 4;
const DELIVERY_MARGIN_BYTES: u32 = 96;
const MALLOC_CAP_8BIT: u32 = 2;
const MALLOC_CAP_DMA: u32 = 4;
const SOURCE_FILE: &[u8] = b"ieee80211_input.c\0";

type RxCallback = unsafe extern "C" fn(*mut c_void, u16, *mut c_void) -> i32;

extern "C" {
    static sta_rxcb: [Option<RxCallback>; 2];
    static g_wifi_menuconfig: u8;

    fn wDev_hwbuf_cnt() -> u32;
    fn esp_get_free_heap_size() -> u32;
    fn esp_wifi_get_rx_pbuf_mem_type() -> u32;
    fn _heap_caps_malloc(size: usize, caps: u32, file: *const u8, line: usize) -> *mut c_void;
    fn _heap_caps_free(buffer: *mut c_void, file: *const u8, line: usize);
    fn ppRecycleRxPkt(packet: *mut c_void);
}

#[inline(always)]
fn copy_has_capacity(free: u32, frame_bytes: u32) -> bool {
    free >= SDK_HEAP_ADMISSION_BYTES
        || free >= frame_bytes.saturating_add(ALLOCATOR_HEADER_BYTES + DELIVERY_MARGIN_BYTES)
}

/// Delivers one SDK RX packet to the registered STA/AP callback.
#[export_name = "__esp8266_hal_ieee80211_deliver_data"]
pub unsafe extern "C" fn ieee80211_deliver_data(interface: u32, packet: *mut c_void) {
    let callback = if interface < 2 {
        unsafe { ptr::read_volatile(ptr::addr_of!(sta_rxcb[interface as usize])) }
    } else {
        None
    };
    let Some(callback) = callback else {
        if !packet.is_null() {
            unsafe { ppRecycleRxPkt(packet) };
        }
        return;
    };

    if packet.is_null() {
        return;
    }

    let frame_bytes = u32::from(unsafe {
        ptr::read_unaligned(packet.cast::<u8>().add(PACKET_LENGTH_OFFSET).cast::<u16>())
    });
    let configured_buffers =
        unsafe { ptr::read_volatile(ptr::addr_of!(g_wifi_menuconfig).add(RX_BUFFER_LIMIT_OFFSET)) };

    let (buffer, owner) = if unsafe { wDev_hwbuf_cnt() } > u32::from(configured_buffers) {
        let buffer = unsafe {
            ptr::read_unaligned(
                packet
                    .cast::<u8>()
                    .add(PACKET_DATA_OFFSET)
                    .cast::<*mut c_void>(),
            )
        };
        (buffer, packet)
    } else {
        let free = unsafe { esp_get_free_heap_size() };
        if !copy_has_capacity(free, frame_bytes) {
            unsafe { ppRecycleRxPkt(packet) };
            return;
        }

        let mem_type = unsafe { esp_wifi_get_rx_pbuf_mem_type() };
        let caps = if mem_type == 1 {
            MALLOC_CAP_DMA
        } else {
            MALLOC_CAP_8BIT
        };
        let line = if mem_type == 1 { 211 } else { 213 };
        let buffer =
            unsafe { _heap_caps_malloc(frame_bytes as usize, caps, SOURCE_FILE.as_ptr(), line) };
        if buffer.is_null() {
            unsafe { ppRecycleRxPkt(packet) };
            return;
        }

        let source = unsafe {
            ptr::read_unaligned(
                packet
                    .cast::<u8>()
                    .add(PACKET_DATA_OFFSET)
                    .cast::<*const u8>(),
            )
        };
        unsafe { ptr::copy_nonoverlapping(source, buffer.cast::<u8>(), frame_bytes as usize) };
        unsafe { ppRecycleRxPkt(packet) };
        (buffer, ptr::null_mut())
    };

    if unsafe { callback(buffer, frame_bytes as u16, owner) } != esp8266_wifi_sys::ESP_OK {
        if owner.is_null() {
            unsafe { _heap_caps_free(buffer, SOURCE_FILE.as_ptr(), 241) };
        } else {
            unsafe { ppRecycleRxPkt(owner) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_admission_uses_the_concrete_frame_size() {
        assert!(copy_has_capacity(200, 100));
        assert!(!copy_has_capacity(199, 100));
        assert!(copy_has_capacity(SDK_HEAP_ADMISSION_BYTES, u32::MAX));
    }
}
