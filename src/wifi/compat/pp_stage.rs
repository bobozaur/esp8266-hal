use core::arch::asm;
use core::ffi::c_void;
use core::hint::black_box;

#[no_mangle]
pub static mut g_pp_stage: u32 = 0;
#[no_mangle]
pub static mut g_pp_stage_hits: u32 = 0;
#[no_mangle]
pub static mut g_pm_bcn_helper_hits: u32 = 0;
#[no_mangle]
pub static mut g_pm_on_bcn_ie_len: u32 = 0;
#[no_mangle]
pub static mut g_pm_last_change_to_state_arg: u32 = 0;
#[no_mangle]
pub static mut g_pm_sleep_for_arg: u32 = 0;
#[no_mangle]
pub static mut g_pm_sleep_for_ret: u32 = 0;
static mut g_pm_bcn_active: u32 = 0;
#[no_mangle]
pub static mut g_scan_parse_beacon_calls: u32 = 0;
#[no_mangle]
pub static mut g_scan_parse_beacon_returns: u32 = 0;
#[no_mangle]
pub static mut g_scan_parse_beacon_bssid_match_calls: u32 = 0;
#[no_mangle]
pub static mut g_ieee80211_match_security_calls: u32 = 0;
#[no_mangle]
pub static mut g_ieee80211_send_probereq_calls: u32 = 0;
#[no_mangle]
pub static mut g_scan_remove_bssid_calls: u32 = 0;
#[no_mangle]
pub static mut g_scan_add_probe_ssid_calls: u32 = 0;
#[no_mangle]
pub static mut g_scan_add_probe_ssid_len: u32 = 0;
#[no_mangle]
pub static mut g_scan_add_probe_ssid_flag: u32 = 0;
#[no_mangle]
pub static mut g_scan_prefer_chan_calls: u32 = 0;
#[no_mangle]
pub static mut g_scan_prefer_chan_last_ret: u32 = 0;
#[no_mangle]
pub static mut g_scan_build_chan_list_calls: u32 = 0;
#[no_mangle]
pub static mut g_scan_build_chan_list_last_mode: u32 = 0;
#[no_mangle]
pub static mut g_scan_build_chan_list_last_chan: u32 = 0;
#[no_mangle]
pub static mut g_scan_hidden_ssid_calls: u32 = 0;
#[no_mangle]
pub static mut g_cnx_bss_alloc_calls: u32 = 0;
#[no_mangle]
pub static mut g_cnx_bss_alloc_accepts: u32 = 0;
#[no_mangle]
pub static mut g_cnx_add_rc_calls: u32 = 0;
#[no_mangle]
pub static mut g_cnx_rc_search_calls: u32 = 0;
#[no_mangle]
pub static mut g_cnx_ctx_ptr: u32 = 0;
#[no_mangle]
pub static mut g_sta_state_timer_ptr: u32 = 0;
#[no_mangle]
pub static mut g_sta_retry_timer_ptr: u32 = 0;
#[no_mangle]
pub static mut g_last_ets_timer_arm_timer: u32 = 0;
#[no_mangle]
pub static mut g_last_ets_timer_arm_msec: u32 = 0;
#[no_mangle]
pub static mut g_last_ets_timer_arm_repeat: u32 = 0;
#[no_mangle]
pub static mut g_last_ets_timer_arm_kind: u32 = 0;
#[no_mangle]
pub static mut g_last_ets_timer_arm_fn: u32 = 0;
#[no_mangle]
pub static mut g_last_ets_timer_arm_arg: u32 = 0;
#[no_mangle]
pub static mut g_last_timer_setfn_timer: u32 = 0;
#[no_mangle]
pub static mut g_last_timer_setfn_fn: u32 = 0;
#[no_mangle]
pub static mut g_last_timer_setfn_kind: u32 = 0;
#[no_mangle]
pub static mut g_sta_state_timer_cb_target: u32 = 0;
#[no_mangle]
pub static mut g_sta_retry_timer_cb_target: u32 = 0;
#[no_mangle]
pub static mut g_sta_retry_timer_cb_first: u32 = 0;
#[no_mangle]
pub static mut g_sta_retry_timer_cb_second: u32 = 0;
#[no_mangle]
pub static mut g_last_ets_timer_proc_head_timer: u32 = 0;
#[no_mangle]
pub static mut g_last_ets_timer_proc_head_fn: u32 = 0;
#[no_mangle]
pub static mut g_last_ets_timer_proc_head_kind: u32 = 0;
#[no_mangle]
pub static mut g_last_ets_timer_proc_head_arg: u32 = 0;
#[no_mangle]
pub static mut g_last_ets_timer_proc_head_expire: u32 = 0;
#[no_mangle]
pub static mut g_last_ets_timer_proc_head_period: u32 = 0;
#[no_mangle]
pub static mut g_last_ets_timer_head_cell_addr: u32 = 0;
#[no_mangle]
pub static mut g_last_timer_callback_kind: u32 = 0;
#[no_mangle]
pub static mut g_last_timer_callback_timer: u32 = 0;
#[no_mangle]
pub static mut g_last_timer_callback_fn: u32 = 0;
#[no_mangle]
pub static mut g_scan_start_stage: u32 = 0;
#[no_mangle]
pub static mut g_scan_start_calls: u32 = 0;
#[no_mangle]
pub static mut g_scan_start_ret: u32 = 0;
#[no_mangle]
pub static mut g_scan_start_chan: u32 = 0;
#[no_mangle]
pub static mut g_cnx_start_handoff_calls: u32 = 0;
#[no_mangle]
pub static mut g_cnx_start_handoff_status: u32 = 0;
#[no_mangle]
pub static mut g_last_pp_post_sig: u32 = 0;
#[no_mangle]
pub static mut g_last_pp_post_stage: u32 = 0;
#[no_mangle]
pub static mut g_sig4_source_stage: u32 = 0;
#[no_mangle]
pub static mut g_sig4_enqueue_calls: u32 = 0;
#[no_mangle]
pub static mut g_sig4_rcupdate_calls: u32 = 0;
#[no_mangle]
pub static mut g_sig4_post_calls: u32 = 0;
#[no_mangle]
pub static mut g_sig4_post_enqueue_snapshot: u32 = 0;
#[no_mangle]
pub static mut g_sig4_post_rcupdate_snapshot: u32 = 0;
#[no_mangle]
pub static mut g_sig5_source_stage: u32 = 0;
#[no_mangle]
pub static mut g_sig5_lmac_rxdone_calls: u32 = 0;
#[no_mangle]
pub static mut g_sig5_enqueue_calls: u32 = 0;
#[no_mangle]
pub static mut g_sig5_post_calls: u32 = 0;
#[no_mangle]
pub static mut g_sig5_post_lmac_snapshot: u32 = 0;
#[no_mangle]
pub static mut g_sig5_post_enqueue_snapshot: u32 = 0;

#[repr(C)]
struct RomOsTimer {
    timer_next: *mut RomOsTimer,
    timer_handle: *mut c_void,
    timer_expire: u32,
    timer_period: u32,
    timer_func: *mut c_void,
    timer_repeat_flag: bool,
    _pad: [u8; 3],
    timer_arg: *mut c_void,
}

#[derive(Clone, Copy)]
struct TimerMirrorSlot {
    timer: u32,
    callback: u32,
    arg: u32,
    period: u32,
    repeat_flag: u32,
    armed: u32,
    kind: u32,
}

const TIMER_MIRROR_SLOTS: usize = 8;
const TIMER_MIRROR_SLOT_INIT: TimerMirrorSlot = TimerMirrorSlot {
    timer: 0,
    callback: 0,
    arg: 0,
    period: 0,
    repeat_flag: 0,
    armed: 0,
    kind: 0,
};
static mut TIMER_MIRROR: [TimerMirrorSlot; TIMER_MIRROR_SLOTS] =
    [TIMER_MIRROR_SLOT_INIT; TIMER_MIRROR_SLOTS];
const TRACE_ACTIVE: bool = true;

#[inline(always)]
unsafe fn set_stage(marker: u8) {
    unsafe {
        if !TRACE_ACTIVE {
            let _ = marker;
            return;
        }
        core::ptr::write_volatile(&raw mut g_pp_stage, marker as u32);
    }
}

#[inline(always)]
unsafe fn classify_named_timer_callback(callback: u32) -> u8 {
    if callback == 0 {
        return b'0';
    }
    if callback == (__wrap_cnx_handshake_timeout as usize as u32)
        || callback == (__real_cnx_handshake_timeout as usize as u32)
    {
        return b'H';
    }
    if callback == (__wrap_dhcp_bind_check as usize as u32)
        || callback == (__real_dhcp_bind_check as usize as u32)
    {
        return b'B';
    }
    if callback == (__wrap_cnx_assoc_timeout as usize as u32)
        || callback == (__real_cnx_assoc_timeout as usize as u32)
    {
        return b'Q';
    }
    if callback == (__wrap_cnx_auth_timeout as usize as u32)
        || callback == (__real_cnx_auth_timeout as usize as u32)
    {
        return b'N';
    }

    b'?'
}

#[inline(always)]
unsafe fn classify_retry_timer_callback(callback: u32, learn: bool) -> u8 {
    unsafe {
        if callback == g_sta_retry_timer_cb_first {
            return b'V';
        }
        if callback == g_sta_retry_timer_cb_second {
            return b'W';
        }
        if learn && g_sta_retry_timer_cb_first == 0 {
            g_sta_retry_timer_cb_first = callback;
            return b'V';
        }
        if learn && g_sta_retry_timer_cb_second == 0 {
            g_sta_retry_timer_cb_second = callback;
            return b'W';
        }

        b'R'
    }
}

#[inline(always)]
unsafe fn classify_timer(timer: u32, callback: u32, learn_retry: bool) -> u8 {
    unsafe {
        let named = classify_named_timer_callback(callback);
        if named != b'?' {
            return named;
        }
        if timer == g_sta_retry_timer_ptr || callback == g_sta_retry_timer_cb_target {
            return classify_retry_timer_callback(callback, learn_retry);
        }
        if timer == g_sta_state_timer_ptr || callback == g_sta_state_timer_cb_target {
            return b'S';
        }
        if callback == 0 {
            b'0'
        } else {
            b'?'
        }
    }
}

#[inline(always)]
unsafe fn is_state_timer_kind(kind: u8) -> bool {
    matches!(kind, b'H' | b'B' | b'Q' | b'N')
}

#[inline(always)]
unsafe fn reset_timer_mirror() {
    unsafe {
        let slots = &raw mut TIMER_MIRROR;
        let mut idx = 0usize;
        while idx < TIMER_MIRROR_SLOTS {
            (*slots)[idx] = TIMER_MIRROR_SLOT_INIT;
            idx += 1;
        }
    }
}

#[inline(always)]
unsafe fn timer_mirror_slot_mut(timer: u32) -> *mut TimerMirrorSlot {
    unsafe {
        let slots = &raw mut TIMER_MIRROR;
        let mut empty: *mut TimerMirrorSlot = core::ptr::null_mut();
        let mut idx = 0usize;
        while idx < TIMER_MIRROR_SLOTS {
            let slot = &mut (*slots)[idx] as *mut TimerMirrorSlot;
            if (*slot).timer == timer {
                return slot;
            }
            if empty.is_null() && (*slot).timer == 0 {
                empty = slot;
            }
            idx += 1;
        }

        let slot = if empty.is_null() {
            &mut (*slots)[TIMER_MIRROR_SLOTS - 1] as *mut TimerMirrorSlot
        } else {
            empty
        };
        (*slot).timer = timer;
        slot
    }
}

#[inline(always)]
unsafe fn timer_mirror_lookup(timer: u32) -> Option<TimerMirrorSlot> {
    unsafe {
        let slots = &raw const TIMER_MIRROR;
        let mut idx = 0usize;
        while idx < TIMER_MIRROR_SLOTS {
            let slot = (*slots)[idx];
            if slot.timer == timer {
                return Some(slot);
            }
            idx += 1;
        }
        None
    }
}

#[inline(always)]
unsafe fn timer_mirror_update_setfn(timer: u32, callback: u32, arg: u32, kind: u8) {
    unsafe {
        let slot = timer_mirror_slot_mut(timer);
        (*slot).callback = callback;
        (*slot).arg = arg;
        (*slot).kind = kind as u32;
    }
}

#[inline(always)]
unsafe fn timer_mirror_update_arm(timer: u32, msec: u32, repeat_flag: u32, kind: u8) {
    unsafe {
        let slot = timer_mirror_slot_mut(timer);
        (*slot).period = msec;
        (*slot).repeat_flag = repeat_flag;
        (*slot).armed = 1;
        if kind != b'?' {
            (*slot).kind = kind as u32;
        }
    }
}

#[inline(always)]
unsafe fn timer_mirror_update_disarm(timer: u32) {
    unsafe {
        let slot = timer_mirror_slot_mut(timer);
        (*slot).armed = 0;
    }
}

#[inline(always)]
unsafe fn record_timer_callback(kind: u8, timer: u32, callback: u32) {
    unsafe {
        g_last_timer_callback_kind = kind as u32;
        g_last_timer_callback_timer = timer;
        g_last_timer_callback_fn = callback;
    }
}

#[inline(always)]
unsafe fn record_timer_arm_call(timer: *mut c_void, msec: u32, repeat_flag: u32) {
    unsafe {
        if !TRACE_ACTIVE {
            return;
        }
        let timer = timer as u32;
        let mirror = timer_mirror_lookup(timer);
        let callback = mirror.map(|slot| slot.callback).unwrap_or(0);
        let arg = mirror.map(|slot| slot.arg).unwrap_or(0);
        let kind = classify_timer(timer, callback, false);
        g_last_ets_timer_arm_timer = timer;
        g_last_ets_timer_arm_msec = msec;
        g_last_ets_timer_arm_repeat = repeat_flag;
        g_last_ets_timer_arm_kind = kind as u32;
        g_last_ets_timer_arm_fn = callback;
        g_last_ets_timer_arm_arg = arg;
        timer_mirror_update_arm(timer, msec, repeat_flag, kind);
    }
}

#[inline(always)]
unsafe fn record_timer_setfn_call(timer: *mut c_void, callback: *mut c_void, arg: *mut c_void) {
    unsafe {
        if !TRACE_ACTIVE {
            return;
        }
        let timer = timer as u32;
        let callback = callback as u32;
        let arg_u32 = arg as u32;
        let kind = classify_timer(timer, callback, true);

        g_last_timer_setfn_timer = timer;
        g_last_timer_setfn_fn = callback;
        g_last_timer_setfn_kind = kind as u32;
        if is_state_timer_kind(kind) {
            g_sta_state_timer_ptr = timer;
            g_sta_state_timer_cb_target = callback;
        } else if timer == g_sta_state_timer_ptr {
            g_sta_state_timer_cb_target = callback;
        } else if timer == g_sta_retry_timer_ptr {
            g_sta_retry_timer_cb_target = callback;
        }
        timer_mirror_update_setfn(timer, callback, arg_u32, kind);
    }
}

#[inline(always)]
unsafe fn load_code_byte(addr: *const u8) -> u8 {
    unsafe {
        let aligned = ((addr as usize) & !0x3) as *const u32;
        let shift = ((addr as usize) & 0x3) * 8;
        let word = aligned.read_volatile();
        ((word >> shift) & 0xff) as u8
    }
}

#[inline(always)]
unsafe fn derive_ets_timer_head_cell_ptr() -> *const *const RomOsTimer {
    unsafe {
        let code = __real_ets_timer_proc as usize as *const u8;
        let mut offset = 0usize;
        while offset < 24 {
            let insn = code.add(offset);
            let inst = (load_code_byte(insn) as u32)
                | ((load_code_byte(insn.add(1)) as u32) << 8)
                | ((load_code_byte(insn.add(2)) as u32) << 16);

            if (inst & 0x0f) == 0x01 {
                // Xtensa LX106 `l32r` uses FIELD_imm16 in instruction bits 8..23
                // and binutils decodes `uimm16x4` as `(((0xffff << 16) | imm16) << 2)`,
                // then adds `((pc + 3) & !0x3)`.
                let imm16 = (inst >> 8) & 0xffff;
                let rel = (0xffff_0000u32 | imm16) << 2;
                let literal_addr =
                    rel.wrapping_add(((insn as usize as u32).wrapping_add(3)) & !0x3) as usize;

                if !(0x4010_0000..0x4030_0000).contains(&literal_addr) {
                    offset += 1;
                    continue;
                }

                let cell = (literal_addr as *const u32).read_volatile();
                if (0x3fff_0000..0x4000_0000).contains(&cell) {
                    return literal_addr as *const *const RomOsTimer;
                }
            }
            offset += 1;
        }

        core::ptr::null()
    }
}

#[inline(always)]
fn hex_nibble(n: u8) -> u8 {
    match n & 0x0f {
        0..=9 => b'0' + (n & 0x0f),
        _ => b'a' + ((n & 0x0f) - 10),
    }
}

#[inline(always)]
unsafe fn trace_hex32(value: u32) {
    unsafe {
        crate::logger::write_byte(hex_nibble((value >> 28) as u8));
        crate::logger::write_byte(hex_nibble((value >> 24) as u8));
        crate::logger::write_byte(hex_nibble((value >> 20) as u8));
        crate::logger::write_byte(hex_nibble((value >> 16) as u8));
        crate::logger::write_byte(hex_nibble((value >> 12) as u8));
        crate::logger::write_byte(hex_nibble((value >> 8) as u8));
        crate::logger::write_byte(hex_nibble((value >> 4) as u8));
        crate::logger::write_byte(hex_nibble(value as u8));
    }
}

#[inline(always)]
unsafe fn mark_stage_hit(mask: u32) {
    unsafe {
        if !TRACE_ACTIVE {
            let _ = mask;
            return;
        }
        let hits = core::ptr::read_volatile(&raw const g_pp_stage_hits);
        core::ptr::write_volatile(&raw mut g_pp_stage_hits, hits | mask);
    }
}

#[inline(always)]
pub(crate) unsafe fn pm_bcn_mark_helper(mask: u32, stage: u8) {
    unsafe {
        if !TRACE_ACTIVE {
            let _ = mask;
            let _ = stage;
            return;
        }
        if core::ptr::read_volatile(&raw const g_pm_bcn_active) == 0 {
            return;
        }

        let hits = core::ptr::read_volatile(&raw const g_pm_bcn_helper_hits);
        core::ptr::write_volatile(&raw mut g_pm_bcn_helper_hits, hits | mask);
        set_stage(stage);
    }
}

#[inline(always)]
unsafe fn reset_pm_bcn_trace() {
    unsafe {
        core::ptr::write_volatile(&raw mut g_pm_bcn_helper_hits, 0);
        core::ptr::write_volatile(&raw mut g_pm_on_bcn_ie_len, 0);
        core::ptr::write_volatile(&raw mut g_pm_last_change_to_state_arg, 0);
        core::ptr::write_volatile(&raw mut g_pm_sleep_for_arg, 0);
        core::ptr::write_volatile(&raw mut g_pm_sleep_for_ret, 0);
    }
}

#[inline(always)]
pub(crate) unsafe fn reset_sig10_scan_trace() {
    unsafe {
        core::ptr::write_volatile(&raw mut g_scan_start_stage, 0);
        core::ptr::write_volatile(&raw mut g_scan_start_calls, 0);
        core::ptr::write_volatile(&raw mut g_scan_start_ret, 0);
        core::ptr::write_volatile(&raw mut g_scan_start_chan, 0);
        core::ptr::write_volatile(&raw mut g_cnx_start_handoff_calls, 0);
        core::ptr::write_volatile(&raw mut g_cnx_start_handoff_status, 0);
    }
}

#[inline(always)]
pub(crate) unsafe fn reset_sig10_timer_trace() {
    unsafe {
        core::ptr::write_volatile(&raw mut g_last_ets_timer_arm_timer, 0);
        core::ptr::write_volatile(&raw mut g_last_ets_timer_arm_msec, 0);
        core::ptr::write_volatile(&raw mut g_last_ets_timer_arm_repeat, 0);
        core::ptr::write_volatile(&raw mut g_last_ets_timer_arm_kind, 0);
        core::ptr::write_volatile(&raw mut g_last_ets_timer_arm_fn, 0);
        core::ptr::write_volatile(&raw mut g_last_ets_timer_arm_arg, 0);
        core::ptr::write_volatile(&raw mut g_last_timer_setfn_timer, 0);
        core::ptr::write_volatile(&raw mut g_last_timer_setfn_fn, 0);
        core::ptr::write_volatile(&raw mut g_last_timer_setfn_kind, 0);
        core::ptr::write_volatile(&raw mut g_last_ets_timer_proc_head_timer, 0);
        core::ptr::write_volatile(&raw mut g_last_ets_timer_proc_head_fn, 0);
        core::ptr::write_volatile(&raw mut g_last_ets_timer_proc_head_kind, 0);
        core::ptr::write_volatile(&raw mut g_last_ets_timer_proc_head_arg, 0);
        core::ptr::write_volatile(&raw mut g_last_ets_timer_proc_head_expire, 0);
        core::ptr::write_volatile(&raw mut g_last_ets_timer_proc_head_period, 0);
        core::ptr::write_volatile(&raw mut g_last_ets_timer_head_cell_addr, 0);
        core::ptr::write_volatile(&raw mut g_last_timer_callback_kind, 0);
        core::ptr::write_volatile(&raw mut g_last_timer_callback_timer, 0);
        core::ptr::write_volatile(&raw mut g_last_timer_callback_fn, 0);
    }
}

#[inline(always)]
pub(crate) unsafe fn reset_pp_stage_trace() {
    unsafe {
        core::ptr::write_volatile(&raw mut g_pp_stage, 0);
        core::ptr::write_volatile(&raw mut g_pp_stage_hits, 0);
    }
}

extern "C" {
    static g_wifi_nvs: *const u8;
    fn __real_pm_sleep_for(param: *const c_void) -> bool;
    fn __real_pm_onBcnRx(param_1: *mut c_void, param_2: *mut c_void, param_3: *mut c_void);
    fn __real_os_timer_disarm(timer: *mut c_void);
    fn __real_pm_send_keepalive_pkt() -> i32;
    fn __real_pm_set_idle_timer();
    fn __real_pm_wait_tx_rx_idle_fun();
    fn __real_pm_change_to_state(state: u32);
    fn __real_scan_start(param_1: u32, param_2: u32, param_3: u32, param_4: u32) -> i32;
    fn __real_sta_input(param_1: *mut c_void, param_2: *mut c_void, param_3: i32) -> u32;
    fn __real_hostap_input(param_1: *mut c_void, param_2: *mut c_void, param_3: i32) -> *mut u8;
    fn __real_ieee80211_recv_action(
        param_1: *mut c_void,
        param_2: *mut c_void,
        param_3: *mut u8,
        param_4: *mut u8,
    );
    fn __real_ieee80211_parse_beacon(
        param_1: *mut c_void,
        param_2: *mut c_void,
        param_3: u32,
        param_4: i32,
    ) -> u32;
    fn __real_scan_parse_beacon(param_1: *mut c_void, param_2: *mut c_void, param_3: u32) -> u32;
    fn __real_ieee80211_match_security(param_1: *mut c_void) -> u16;
    fn __real_ieee80211_send_probereq(
        param_1: u32,
        param_2: u32,
        param_3: u32,
        param_4: u32,
        param_5: *const u8,
        param_6: u32,
        param_7: u8,
    ) -> u32;
    fn __real_scan_remove_bssid();
    fn __real_scan_add_probe_ssid(param_1: u32, param_2: *const u8, param_3: u32, param_4: i8);
    fn __real_scan_prefer_chan(param_1: *const u8) -> u8;
    fn __real_scan_build_chan_list(param_1: i8, param_2: u8) -> u32;
    fn __real_scan_hidden_ssid(param_1: u8);
    fn __real_ic_bss_info_update(param_1: u32, param_2: *mut c_void, param_3: u32, param_4: i32);
    fn __real_ppProcessTxQ(param_1: u32) -> u32;
    fn __real_ppProcessWaitQ();
    fn __real_tx_pwctrl_background(param_1: u32, param_2: u32);
    fn __real_pm_reset_keepalive_time();
    fn __real_ppCheckTxIdle(param_1: u32) -> u32;
    fn __real_ieee80211_decap(param_1: u32, param_2: u32, param_3: u32, param_4: u32) -> u32;
    fn __real_ieee80211_setup_rates(param_1: u32, param_2: u32, param_3: u32, param_4: u32);
    fn __real_ieee80211_sta_new_state(param_1: u32, param_2: u32, param_3: u32) -> u32;
    fn __real_ieee80211_ccmp_decrypt(param_1: u32, param_2: u32) -> u32;
    fn __real_ieee80211_send_mgmt(param_1: u32, param_2: u32, param_3: u32) -> u32;
    fn __real_ets_timer_proc();
    fn __real_ets_timer_setfn(timer: *mut c_void, callback: *mut c_void, arg: *mut c_void);
    fn __real_ets_timer_arm(timer: *mut c_void, msec: u32, repeat_flag: u32);
    fn __real_os_timer_setfn(timer: *mut c_void, callback: *mut c_void, arg: *mut c_void);
    fn __real_os_timer_arm(timer: *mut c_void, msec: u32, repeat_flag: u32);
    fn __real_cnx_auth_timeout();
    fn __real_cnx_assoc_timeout();
    fn __real_cnx_handshake_timeout();
    fn __real_cnx_sta_connect_cmd();
    fn __real_cnx_start_handoff_cb(param_1: u32, param_2: i32);
    fn __real_cnx_sta_associated(param_1: u32, param_2: u32);
    fn __real_dhcp_bind_check();
    fn __real_pp_post(param_1: u32, param_2: *mut c_void) -> i32;
    fn __real_ppEnqueueTxDone();
    fn __real_rcUpdateTxDone(param_1: u32);
    fn __real_ppEnqueueRxq();
    fn __real_lmacRxDone();
    fn __real_ieee80211_mlme_connect_bss(param_1: u32, param_2: u32, param_3: u32);
    fn __real_cnx_rc_search(param_1: u32) -> u32;
    fn __real_cnx_bss_alloc(param_1: u32, param_2: i32, param_3: u32) -> *mut c_void;
    fn __real_cnx_add_rc(param_1: u32) -> u32;
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_pm_sleep_for(param: *const c_void) -> bool {
    unsafe {
        g_pm_sleep_for_arg = param as u32;
        set_stage(b'p');
        let ret = __real_pm_sleep_for(param);
        g_pm_sleep_for_ret = ret as u32;
        ret
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_pm_onBcnRx(
    param_1: *mut c_void,
    param_2: *mut c_void,
    param_3: *mut c_void,
) {
    unsafe {
        core::ptr::write_volatile(&raw mut g_pp_stage_hits, 0);
        core::ptr::write_volatile(&raw mut g_pm_bcn_helper_hits, 0);
        core::ptr::write_volatile(
            &raw mut g_pm_on_bcn_ie_len,
            (param_3 as usize).wrapping_sub(param_2 as usize) as u32,
        );
        core::ptr::write_volatile(&raw mut g_pm_last_change_to_state_arg, 0);
        core::ptr::write_volatile(&raw mut g_pm_bcn_active, 1);
        mark_stage_hit(1 << 17);
        set_stage(b'b');
        __real_pm_onBcnRx(param_1, param_2, param_3);
        mark_stage_hit(1 << 28);
        core::ptr::write_volatile(&raw mut g_pm_bcn_active, 0);
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_os_timer_disarm(timer: *mut c_void) {
    unsafe {
        let timer = timer as u32;
        timer_mirror_update_disarm(timer);
        pm_bcn_mark_helper(1 << 0, b'o');
        __real_os_timer_disarm(timer as *mut c_void);
        pm_bcn_mark_helper(1 << 16, b'o');
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_pm_send_keepalive_pkt() -> i32 {
    unsafe {
        pm_bcn_mark_helper(1 << 1, b'k');
        let ret = __real_pm_send_keepalive_pkt();
        pm_bcn_mark_helper(1 << 17, b'k');
        ret
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_pm_set_idle_timer() {
    unsafe {
        pm_bcn_mark_helper(1 << 2, b'i');
        __real_pm_set_idle_timer();
        pm_bcn_mark_helper(1 << 18, b'i');
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_pm_wait_tx_rx_idle_fun() {
    unsafe {
        pm_bcn_mark_helper(1 << 3, b'w');
        __real_pm_wait_tx_rx_idle_fun();
        pm_bcn_mark_helper(1 << 19, b'w');
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_pm_change_to_state(state: u32) {
    unsafe {
        core::ptr::write_volatile(&raw mut g_pm_last_change_to_state_arg, state);
        pm_bcn_mark_helper(1 << 4, b'm');
        __real_pm_change_to_state(state);
        pm_bcn_mark_helper(1 << 20, b'm');
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_scan_start(
    param_1: u32,
    param_2: u32,
    param_3: u32,
    param_4: u32,
) -> i32 {
    unsafe {
        core::ptr::write_volatile(&raw mut g_scan_start_stage, b's' as u32);
        core::ptr::write_volatile(
            &raw mut g_scan_start_calls,
            core::ptr::read_volatile(&raw const g_scan_start_calls).wrapping_add(1),
        );
        core::ptr::write_volatile(&raw mut g_scan_start_chan, param_2 & 0xff);
        let ret = __real_scan_start(param_1, param_2, param_3, param_4);
        core::ptr::write_volatile(&raw mut g_scan_start_ret, ret as u32);
        core::ptr::write_volatile(&raw mut g_scan_start_stage, b'S' as u32);
        ret
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_sta_input(
    param_1: *mut c_void,
    param_2: *mut c_void,
    param_3: i32,
) -> u32 {
    unsafe {
        reset_pm_bcn_trace();
        core::ptr::write_volatile(&raw mut g_pp_stage_hits, 0);
        mark_stage_hit(1 << 18);
        crate::wifi::device::debug_reset_sta_rx_cb_trace();
        set_stage(b's');
        let ret = __real_sta_input(param_1, param_2, param_3);
        mark_stage_hit(1 << 27);
        set_stage(b't');
        ret
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_hostap_input(
    param_1: *mut c_void,
    param_2: *mut c_void,
    param_3: i32,
) -> *mut u8 {
    unsafe {
        reset_pm_bcn_trace();
        mark_stage_hit(1 << 19);
        set_stage(b'h');
        __real_hostap_input(param_1, param_2, param_3)
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_ieee80211_recv_action(
    param_1: *mut c_void,
    param_2: *mut c_void,
    param_3: *mut u8,
    param_4: *mut u8,
) {
    unsafe {
        reset_pm_bcn_trace();
        mark_stage_hit(1 << 20);
        set_stage(b'a');
        __real_ieee80211_recv_action(param_1, param_2, param_3, param_4);
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_ieee80211_parse_beacon(
    param_1: *mut c_void,
    param_2: *mut c_void,
    param_3: u32,
    param_4: i32,
) -> u32 {
    unsafe {
        mark_stage_hit(1 << 21);
        set_stage(b'P');
        let ret = __real_ieee80211_parse_beacon(param_1, param_2, param_3, param_4);
        mark_stage_hit(1 << 29);
        ret
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_scan_parse_beacon(
    param_1: *mut c_void,
    param_2: *mut c_void,
    param_3: u32,
) -> u32 {
    unsafe {
        g_scan_parse_beacon_calls = g_scan_parse_beacon_calls.wrapping_add(1);
        mark_stage_hit(1 << 8);
        set_stage(b'Y');
        let ret = __real_scan_parse_beacon(param_1, param_2, param_3);
        g_scan_parse_beacon_returns = g_scan_parse_beacon_returns.wrapping_add(1);
        mark_stage_hit(1 << 9);
        set_stage(b'y');
        ret
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_ieee80211_match_security(param_1: *mut c_void) -> u16 {
    unsafe {
        g_ieee80211_match_security_calls = g_ieee80211_match_security_calls.wrapping_add(1);
        mark_stage_hit(1 << 10);
        set_stage(b'M');
        __real_ieee80211_match_security(param_1)
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_ieee80211_send_probereq(
    param_1: u32,
    param_2: u32,
    param_3: u32,
    param_4: u32,
    param_5: *const u8,
    param_6: u32,
    param_7: u8,
) -> u32 {
    unsafe {
        g_ieee80211_send_probereq_calls = g_ieee80211_send_probereq_calls.wrapping_add(1);
        mark_stage_hit(1 << 11);
        __real_ieee80211_send_probereq(
            param_1, param_2, param_3, param_4, param_5, param_6, param_7,
        )
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_scan_remove_bssid() {
    unsafe {
        g_scan_remove_bssid_calls = g_scan_remove_bssid_calls.wrapping_add(1);
        mark_stage_hit(1 << 12);
        __real_scan_remove_bssid();
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_scan_add_probe_ssid(
    param_1: u32,
    param_2: *const u8,
    param_3: u32,
    param_4: i8,
) {
    unsafe {
        g_scan_add_probe_ssid_calls = g_scan_add_probe_ssid_calls.wrapping_add(1);
        g_scan_add_probe_ssid_len = param_3;
        g_scan_add_probe_ssid_flag = param_4 as u32;
        mark_stage_hit(1 << 13);
        __real_scan_add_probe_ssid(param_1, param_2, param_3, param_4);
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_scan_prefer_chan(param_1: *const u8) -> u8 {
    unsafe {
        g_scan_prefer_chan_calls = g_scan_prefer_chan_calls.wrapping_add(1);
        let ret = __real_scan_prefer_chan(param_1);
        g_scan_prefer_chan_last_ret = ret as u32;
        mark_stage_hit(1 << 14);
        ret
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_scan_build_chan_list(param_1: i8, param_2: u8) -> u32 {
    unsafe {
        g_scan_build_chan_list_calls = g_scan_build_chan_list_calls.wrapping_add(1);
        g_scan_build_chan_list_last_mode = param_1 as u32;
        g_scan_build_chan_list_last_chan = param_2 as u32;
        mark_stage_hit(1 << 15);
        __real_scan_build_chan_list(param_1, param_2)
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_scan_hidden_ssid(param_1: u8) {
    unsafe {
        g_scan_hidden_ssid_calls = g_scan_hidden_ssid_calls.wrapping_add(1);
        mark_stage_hit(1 << 16);
        __real_scan_hidden_ssid(param_1);
        let sp: usize;
        asm!("mov {0}, a1", out(reg) sp, options(nomem, nostack, preserves_flags));
        if ((sp + 44) as *const u32).read_volatile() != 0 {
            g_scan_hidden_ssid_calls |= 0x4000_0000;
        }
        g_scan_hidden_ssid_calls |= 0x8000_0000;
    }
}

pub unsafe extern "C" fn __wrap_ic_bss_info_update(
    param_1: u32,
    param_2: *mut c_void,
    param_3: u32,
    param_4: i32,
) {
    unsafe {
        mark_stage_hit(1 << 22);
        set_stage(b'I');
        __real_ic_bss_info_update(param_1, param_2, param_3, param_4);
        mark_stage_hit(1 << 30);
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_ppProcessTxQ(param_1: u32) -> u32 {
    unsafe {
        mark_stage_hit(1 << 23);
        set_stage(b'0');
        let ret = __real_ppProcessTxQ(param_1);
        set_stage(b'o');
        ret
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_tx_pwctrl_background(param_1: u32, param_2: u32) {
    unsafe {
        mark_stage_hit(1 << 24);
        set_stage(b'T');
        __real_tx_pwctrl_background(param_1, param_2);
        set_stage(b't');
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_pm_reset_keepalive_time() {
    unsafe {
        mark_stage_hit(1 << 25);
        set_stage(b'K');
        __real_pm_reset_keepalive_time();
        set_stage(b'k');
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_ppCheckTxIdle(param_1: u32) -> u32 {
    unsafe {
        mark_stage_hit(1 << 26);
        set_stage(b'X');
        let ret = __real_ppCheckTxIdle(param_1);
        mark_stage_hit(1 << 31);
        set_stage(b'x');
        ret
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_ppProcessWaitQ() {
    unsafe {
        set_stage(b'Q');
        __real_ppProcessWaitQ();
        set_stage(b'q');
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_ieee80211_decap(
    param_1: u32,
    param_2: u32,
    param_3: u32,
    param_4: u32,
) -> u32 {
    unsafe {
        set_stage(b'd');
        let ret = __real_ieee80211_decap(param_1, param_2, param_3, param_4);
        set_stage(b'D');
        ret
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_ieee80211_setup_rates(
    param_1: u32,
    param_2: u32,
    param_3: u32,
    param_4: u32,
) {
    unsafe {
        set_stage(b'r');
        __real_ieee80211_setup_rates(param_1, param_2, param_3, param_4);
        set_stage(b'R');
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_ieee80211_sta_new_state(
    param_1: u32,
    param_2: u32,
    param_3: u32,
) -> u32 {
    unsafe {
        set_stage(b'n');
        let ret = __real_ieee80211_sta_new_state(param_1, param_2, param_3);
        set_stage(b'N');
        ret
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_ieee80211_ccmp_decrypt(param_1: u32, param_2: u32) -> u32 {
    unsafe {
        set_stage(b'c');
        let ret = __real_ieee80211_ccmp_decrypt(param_1, param_2);
        set_stage(b'C');
        ret
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_ieee80211_send_mgmt(
    param_1: u32,
    param_2: u32,
    param_3: u32,
) -> u32 {
    unsafe {
        set_stage(b'm');
        let ret = __real_ieee80211_send_mgmt(param_1, param_2, param_3);
        set_stage(b'M');
        ret
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_ets_timer_proc() {
    unsafe {
        if TRACE_ACTIVE {
            let head_cell = derive_ets_timer_head_cell_ptr();
            g_last_ets_timer_head_cell_addr = head_cell as u32;
            let head = if head_cell.is_null() {
                core::ptr::null()
            } else {
                core::ptr::read_volatile(head_cell)
            };

            if head_cell.is_null() {
                g_last_ets_timer_proc_head_timer = 0;
                g_last_ets_timer_proc_head_fn = 0;
                g_last_ets_timer_proc_head_arg = 0;
                g_last_ets_timer_proc_head_expire = 0;
                g_last_ets_timer_proc_head_period = 0;
                g_last_ets_timer_proc_head_kind = b'X' as u32;
            } else if head.is_null() {
                g_last_ets_timer_proc_head_timer = 0;
                g_last_ets_timer_proc_head_fn = 0;
                g_last_ets_timer_proc_head_arg = 0;
                g_last_ets_timer_proc_head_expire = 0;
                g_last_ets_timer_proc_head_period = 0;
                g_last_ets_timer_proc_head_kind = b'0' as u32;
            } else {
                let head_ptr = head as u32;
                let mirror = timer_mirror_lookup(head_ptr);
                let head_fn = mirror
                    .map(|slot| {
                        if slot.callback != 0 {
                            slot.callback
                        } else {
                            (*head).timer_func as u32
                        }
                    })
                    .unwrap_or_else(|| (*head).timer_func as u32);
                let head_arg = mirror
                    .map(|slot| {
                        if slot.arg != 0 {
                            slot.arg
                        } else {
                            (*head).timer_arg as u32
                        }
                    })
                    .unwrap_or_else(|| (*head).timer_arg as u32);
                let head_period = mirror
                    .map(|slot| {
                        if slot.period != 0 {
                            slot.period
                        } else {
                            (*head).timer_period
                        }
                    })
                    .unwrap_or_else(|| (*head).timer_period);
                let head_kind = mirror
                    .map(|slot| {
                        if slot.kind != 0 {
                            slot.kind as u8
                        } else {
                            classify_timer(head_ptr, head_fn, false)
                        }
                    })
                    .unwrap_or_else(|| classify_timer(head_ptr, head_fn, false));
                g_last_ets_timer_proc_head_timer = head_ptr;
                g_last_ets_timer_proc_head_fn = head_fn;
                g_last_ets_timer_proc_head_arg = head_arg;
                g_last_ets_timer_proc_head_expire = (*head).timer_expire;
                g_last_ets_timer_proc_head_period = head_period;
                g_last_ets_timer_proc_head_kind = head_kind as u32;
            }
        }
        set_stage(b'E');
        __real_ets_timer_proc();
        set_stage(b'e');
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_ets_timer_arm(timer: *mut c_void, msec: u32, repeat_flag: u32) {
    unsafe {
        record_timer_arm_call(timer, msec, repeat_flag);
        __real_ets_timer_arm(timer as *mut c_void, msec, repeat_flag);
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_ets_timer_setfn(
    timer: *mut c_void,
    callback: *mut c_void,
    arg: *mut c_void,
) {
    unsafe {
        record_timer_setfn_call(timer, callback, arg);
        __real_ets_timer_setfn(timer as *mut c_void, callback as *mut c_void, arg);
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_os_timer_arm(timer: *mut c_void, msec: u32, repeat_flag: u32) {
    unsafe {
        record_timer_arm_call(timer, msec, repeat_flag);
        __real_os_timer_arm(timer, msec, repeat_flag);
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_os_timer_setfn(
    timer: *mut c_void,
    callback: *mut c_void,
    arg: *mut c_void,
) {
    unsafe {
        record_timer_setfn_call(timer, callback, arg);
        __real_os_timer_setfn(timer, callback, arg);
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_cnx_auth_timeout() {
    unsafe {
        record_timer_callback(
            b'N',
            g_sta_state_timer_ptr,
            __wrap_cnx_auth_timeout as usize as u32,
        );
        mark_stage_hit(1 << 3);
        set_stage(b'N');
        __real_cnx_auth_timeout();
        set_stage(b'n');
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_cnx_assoc_timeout() {
    unsafe {
        record_timer_callback(
            b'Q',
            g_sta_state_timer_ptr,
            __wrap_cnx_assoc_timeout as usize as u32,
        );
        set_stage(b'Q');
        __real_cnx_assoc_timeout();
        set_stage(b'q');
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_cnx_handshake_timeout() {
    unsafe {
        record_timer_callback(
            b'H',
            g_sta_state_timer_ptr,
            __wrap_cnx_handshake_timeout as usize as u32,
        );
        set_stage(b'H');
        __real_cnx_handshake_timeout();
        set_stage(b'h');
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_cnx_sta_connect_cmd() {
    unsafe {
        mark_stage_hit(1 << 0);
        set_stage(b'D');
        __real_cnx_sta_connect_cmd();
        set_stage(b'd');
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_cnx_start_handoff_cb(param_1: u32, param_2: i32) {
    unsafe {
        g_cnx_start_handoff_calls = g_cnx_start_handoff_calls.wrapping_add(1);
        g_cnx_start_handoff_status = param_2 as u32;
        set_stage(b'F');
        __real_cnx_start_handoff_cb(param_1, param_2);
        set_stage(b'f');
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_cnx_sta_associated(param_1: u32, param_2: u32) {
    unsafe {
        reset_timer_mirror();
        g_cnx_ctx_ptr = param_1;
        g_sta_state_timer_ptr = param_1.wrapping_add(4);
        g_sta_retry_timer_ptr = param_1.wrapping_add(0x20);
        g_sta_state_timer_cb_target = 0;
        g_sta_retry_timer_cb_target = 0;
        g_sta_retry_timer_cb_first = 0;
        g_sta_retry_timer_cb_second = 0;
        mark_stage_hit(1 << 2);
        set_stage(b'J');
        __real_cnx_sta_associated(param_1, param_2);
        set_stage(b'j');
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_dhcp_bind_check() {
    unsafe {
        record_timer_callback(
            b'B',
            g_sta_state_timer_ptr,
            __wrap_dhcp_bind_check as usize as u32,
        );
        set_stage(b'B');
        __real_dhcp_bind_check();
        set_stage(b'b');
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_ppEnqueueTxDone() {
    unsafe {
        g_sig4_source_stage = b'E' as u32;
        g_sig4_enqueue_calls = g_sig4_enqueue_calls.wrapping_add(1);
        __real_ppEnqueueTxDone();
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_rcUpdateTxDone(param_1: u32) {
    unsafe {
        g_sig4_source_stage = b'U' as u32;
        g_sig4_rcupdate_calls = g_sig4_rcupdate_calls.wrapping_add(1);
        __real_rcUpdateTxDone(param_1);
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_ppEnqueueRxq() {
    unsafe {
        g_sig5_source_stage = b'Q' as u32;
        g_sig5_enqueue_calls = g_sig5_enqueue_calls.wrapping_add(1);
        __real_ppEnqueueRxq();
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_lmacRxDone() {
    unsafe {
        g_sig5_source_stage = b'L' as u32;
        g_sig5_lmac_rxdone_calls = g_sig5_lmac_rxdone_calls.wrapping_add(1);
        __real_lmacRxDone();
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_pp_post(param_1: u32, param_2: *mut c_void) -> i32 {
    unsafe {
        g_last_pp_post_sig = param_1;

        match param_1 {
            0x04 => {
                g_sig4_post_calls = g_sig4_post_calls.wrapping_add(1);
                g_last_pp_post_stage = g_sig4_source_stage;
                g_sig4_post_enqueue_snapshot = g_sig4_enqueue_calls;
                g_sig4_post_rcupdate_snapshot = g_sig4_rcupdate_calls;
            }
            0x05 => {
                g_sig5_post_calls = g_sig5_post_calls.wrapping_add(1);
                g_last_pp_post_stage = g_sig5_source_stage;
                g_sig5_post_lmac_snapshot = g_sig5_lmac_rxdone_calls;
                g_sig5_post_enqueue_snapshot = g_sig5_enqueue_calls;
            }
            0x10 => {
                g_last_pp_post_stage = b'T' as u32;
            }
            _ => {}
        }

        __real_pp_post(param_1, param_2)
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_ieee80211_mlme_connect_bss(
    param_1: u32,
    param_2: u32,
    param_3: u32,
) {
    unsafe {
        mark_stage_hit(1 << 1);
        set_stage(b'L');
        __real_ieee80211_mlme_connect_bss(param_1, param_2, param_3);
        set_stage(b'l');
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_cnx_rc_search(param_1: u32) -> u32 {
    unsafe {
        g_cnx_rc_search_calls = g_cnx_rc_search_calls.wrapping_add(1);
        mark_stage_hit(1 << 4);
        set_stage(b'K');
        let ret = __real_cnx_rc_search(param_1);
        set_stage(b'k');
        ret
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_cnx_bss_alloc(
    param_1: u32,
    param_2: i32,
    param_3: u32,
) -> *mut c_void {
    unsafe {
        g_cnx_bss_alloc_calls = g_cnx_bss_alloc_calls.wrapping_add(1);
        mark_stage_hit(1 << 5);
        set_stage(b'R');
        let ret = __real_cnx_bss_alloc(param_1, param_2, param_3);
        if !ret.is_null() {
            g_cnx_bss_alloc_accepts = g_cnx_bss_alloc_accepts.wrapping_add(1);
            mark_stage_hit(1 << 6);
        }
        set_stage(b'r');
        ret
    }
}

#[no_mangle]
pub unsafe extern "C" fn __wrap_cnx_add_rc(param_1: u32) -> u32 {
    unsafe {
        g_cnx_add_rc_calls = g_cnx_add_rc_calls.wrapping_add(1);
        mark_stage_hit(1 << 7);
        set_stage(b'A');
        let ret = __real_cnx_add_rc(param_1);
        set_stage(b'a');
        ret
    }
}

#[inline(never)]
pub fn retain_timer_probe_symbols() -> usize {
    let sum = (__wrap_ets_timer_proc as usize)
        ^ (__wrap_ets_timer_setfn as usize)
        ^ (__wrap_ets_timer_arm as usize)
        ^ (__wrap_cnx_sta_associated as usize)
        ^ (__wrap_cnx_handshake_timeout as usize)
        ^ (__wrap_cnx_assoc_timeout as usize)
        ^ (__wrap_cnx_auth_timeout as usize)
        ^ (__wrap_dhcp_bind_check as usize);

    black_box(sum)
}
