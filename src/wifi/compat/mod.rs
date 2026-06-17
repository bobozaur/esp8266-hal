//! FreeRTOS compatibility layer — pure Rust replacements for the FreeRTOS
//! kernel symbols expected by the ESP8266 WiFi blob libraries.
//!
//! # Architecture
//!
//! The WiFi blobs (`libcore.a`, `libnet80211.a`, `libpp.a`, `libphy.a`) call
//! FreeRTOS functions through two layers:
//!
//! 1. **`__wifi_*` shim functions** (in [`wifi_os_adapter`]) — clean adapter
//!    matching the `esp_wifi_osi.h` interface.
//! 2. **Direct FreeRTOS symbols** — `vPortEnterCritical`, `pxCurrentTCB`,
//!    `vTaskSwitchContext`, `_xt_isr_handler`, etc.
//!
//! This module provides both layers entirely in Rust. The Xtensa assembly
//! files for context save/restore (`xtensa_vectors.S`, `xtensa_context.S`,
//! `os_cpu_a.S`) remain compiled from C and reference our `#[no_mangle]`
//! exports at link time.

pub mod critical;
pub mod entry;
pub mod event_group;
pub mod memory;
pub mod port;
pub mod queue;
pub mod semphr;
pub mod task;
pub mod timer;
pub mod wifi_os_adapter;

#[cfg(feature = "wifi-debug-trace")]
pub mod pp_stage;

#[cfg(not(feature = "wifi-debug-trace"))]
pub mod pp_stage {
    #[no_mangle]
    pub static mut g_pp_stage: u32 = 0;
    #[no_mangle]
    pub static mut g_pp_stage_hits: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_decap_calls: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_decap_zero_returns: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_ccmp_decrypt_calls: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_ccmp_decrypt_ok: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_ccmp_decrypt_fail: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_crypto_decap_calls: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_crypto_decap_ok: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_crypto_decap_fail: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_deliver_data_calls: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_deliver_data_if0: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_deliver_data_if1: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_deliver_data_other_if: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_deliver_data_last_if: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_deliver_data_last_len: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_deliver_data_last_sta_slot: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_deliver_data_last_ap_slot: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_deliver_data_cb_hit: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_deliver_data_cb_miss: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_deliver_data_last_cb_delta: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_deliver_data_heap_checks: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_deliver_data_last_free_heap: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_deliver_data_alloc_attempts: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_deliver_data_alloc_failures: u32 = 0;
    #[no_mangle]
    pub static mut g_ieee80211_deliver_data_last_alloc_size: u32 = 0;

    extern "C" {
        fn __real_ieee80211_deliver_data(param_1: u32, param_2: u32);
    }

    static mut DELIVER_PROBE_ACTIVE: bool = false;
    static mut DELIVER_CURRENT_LEN: u16 = 0;
    const DELIVER_HEAP_GATE: u32 = 0x1800;
    const DELIVER_HEAP_MARGIN: u32 = 96;
    const DELIVER_HEAP_HEADER_BYTES: u32 = 4;

    #[inline(always)]
    unsafe fn packet_len(packet: u32) -> u16 {
        if packet == 0 {
            return 0;
        }

        unsafe { core::ptr::read_unaligned((packet.wrapping_add(0x1a)) as *const u16) }
    }

    #[inline(always)]
    pub(crate) unsafe fn adjust_deliver_free_heap(free_heap: u32) -> u32 {
        let probe_active = unsafe { DELIVER_PROBE_ACTIVE };
        if !probe_active || free_heap >= DELIVER_HEAP_GATE {
            return free_heap;
        }

        let current_len = unsafe { DELIVER_CURRENT_LEN };
        let needed = current_len as u32 + DELIVER_HEAP_HEADER_BYTES + DELIVER_HEAP_MARGIN;
        if free_heap >= needed {
            DELIVER_HEAP_GATE
        } else {
            free_heap
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn __wrap_ieee80211_deliver_data(param_1: u32, param_2: u32) {
        unsafe {
            DELIVER_CURRENT_LEN = packet_len(param_2);
            g_ieee80211_deliver_data_calls = g_ieee80211_deliver_data_calls.wrapping_add(1);
            g_ieee80211_deliver_data_last_if = param_1;
            g_ieee80211_deliver_data_last_len = DELIVER_CURRENT_LEN as u32;
            match param_1 {
                0 => g_ieee80211_deliver_data_if0 = g_ieee80211_deliver_data_if0.wrapping_add(1),
                1 => g_ieee80211_deliver_data_if1 = g_ieee80211_deliver_data_if1.wrapping_add(1),
                _ => {
                    g_ieee80211_deliver_data_other_if =
                        g_ieee80211_deliver_data_other_if.wrapping_add(1);
                }
            }
            DELIVER_PROBE_ACTIVE = true;
            __real_ieee80211_deliver_data(param_1, param_2);
            DELIVER_PROBE_ACTIVE = false;
            DELIVER_CURRENT_LEN = 0;
        }
    }

    #[inline(always)]
    pub(crate) unsafe fn pm_bcn_mark_helper(mask: u32, stage: u8) {
        let _ = (mask, stage);
    }

    #[inline(always)]
    pub(crate) unsafe fn reset_sig10_scan_trace() {}

    #[inline(always)]
    pub(crate) unsafe fn reset_sig10_timer_trace() {}

    #[inline(always)]
    pub(crate) unsafe fn reset_pp_stage_trace() {
        unsafe {
            core::ptr::write_volatile(&raw mut g_pp_stage, 0);
            core::ptr::write_volatile(&raw mut g_pp_stage_hits, 0);
        }
    }

    #[inline(always)]
    pub(crate) unsafe fn note_deliver_probe_free_heap(_free_heap: u32) {}

    #[inline(always)]
    pub(crate) unsafe fn note_deliver_probe_alloc(_size: usize, _success: bool) {}

    #[inline(always)]
    pub fn retain_timer_probe_symbols() -> usize {
        0
    }
}
