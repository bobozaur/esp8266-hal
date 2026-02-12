//! Probe the driver-owned start/connect path with the same low-overhead crash
//! diagnostics as `wifi_connect_raw_probe`.
//!
//! Unlike the raw probe, this uses:
//! - `WifiController::start()` so the Rust event trampoline is registered
//! - `WifiController::connect_blocking()` so the wait path matches the current
//!   public driver API instead of a hand-written delay loop

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::Duration;

use esp8266_hal::println;
use esp8266_hal::wifi::controller::WifiInitConfig;
use esp8266_hal::wifi::{AuthMode, StaConfig, Storage, WifiController, WifiMode};
use xtensa_lx_rt::exception::{Context, ExceptionCause};

const WIFI_SSID: &str = include_str!("ssid");
const WIFI_PASS: &str = include_str!("pass");
// Re-verify this against the linked `ets_timer_proc()` body whenever the image
// layout changes. The current no-wrap probe build loads `0x3fff2f5c`.
const ETS_TIMER_LIST_HEAD_PTR: *const *const RomOsTimer = 0x3fff_2f5c as *const *const RomOsTimer;

#[repr(C)]
struct RomOsTimer {
    timer_next: *const RomOsTimer,
    timer_handle: *const core::ffi::c_void,
    timer_expire: u32,
    timer_period: u32,
    timer_func: *const core::ffi::c_void,
    timer_repeat_flag: bool,
    _pad: [u8; 3],
    timer_arg: *const core::ffi::c_void,
}

fn hex_nibble(n: u8) -> u8 {
    match n & 0x0f {
        0..=9 => b'0' + (n & 0x0f),
        _ => b'a' + ((n & 0x0f) - 10),
    }
}

fn write_hex16(value: u16) {
    esp8266_hal::logger::write_byte(hex_nibble((value >> 12) as u8));
    esp8266_hal::logger::write_byte(hex_nibble((value >> 8) as u8));
    esp8266_hal::logger::write_byte(hex_nibble((value >> 4) as u8));
    esp8266_hal::logger::write_byte(hex_nibble(value as u8));
}

fn write_hex32(value: u32) {
    write_hex16((value >> 16) as u16);
    write_hex16(value as u16);
}

fn write_task_marker(value: u32) {
    if value != 0 {
        esp8266_hal::logger::write_byte(value as u8);
    }
}

unsafe fn write_pp_signal4_dispatch_state() {
    extern "C" {
        static mut pTxRx: *mut u8;
        static g_pp_last_sig: u32;
        static g_pp_last_arg: u32;
    }

    let last_sig = unsafe { g_pp_last_sig };
    let last_arg = unsafe { g_pp_last_arg };
    let txrx = unsafe { pTxRx };
    let mut tx_head = 0u32;
    let mut tx_bits = 0u32;
    let mut active_mask = 0u32;
    let mut first_bit = u32::MAX;
    let mut callback = 0u32;

    if !txrx.is_null() {
        tx_head = unsafe { core::ptr::read(txrx.add(0xa8) as *const u32) };
        active_mask = unsafe { core::ptr::read(txrx.add(0xb8) as *const u32) };

        // Match the decompiled `ppTask()` signal-4 path:
        // it dequeues the queue message, then reads the live tx-head from
        // `pTxRx + 0xa8`, not from the queue message arg.
        let item = if tx_head != 0 {
            tx_head as *const u8
        } else {
            core::ptr::null()
        };

        if !item.is_null() {
            let tx_mode_ptr = unsafe { core::ptr::read(item.add(0x28) as *const *const u32) };
            if !tx_mode_ptr.is_null() {
                tx_bits = unsafe { core::ptr::read(tx_mode_ptr.add(4)) };
            }
        }

        if tx_bits != 0 {
            first_bit = tx_bits.trailing_zeros();
            callback = unsafe {
                core::ptr::read(txrx.add(0x138 + (first_bit as usize) * 4) as *const u32)
            };
        }
    }

    esp8266_hal::logger::write_byte(b's');
    write_hex32(last_sig);
    esp8266_hal::logger::write_byte(b'a');
    write_hex32(last_arg);

    let head = unsafe { core::ptr::read_volatile(ETS_TIMER_LIST_HEAD_PTR) };
    esp8266_hal::logger::write_byte(b'h');
    write_hex32(head as u32);
    if !head.is_null() {
        let timer = unsafe { &*head };
        esp8266_hal::logger::write_byte(b't');
        write_hex32(timer.timer_handle as u32);
        esp8266_hal::logger::write_byte(b'f');
        write_hex32(timer.timer_func as u32);
        esp8266_hal::logger::write_byte(b'g');
        write_hex32(timer.timer_arg as u32);
        esp8266_hal::logger::write_byte(b'e');
        write_hex32(timer.timer_expire);
        esp8266_hal::logger::write_byte(b'p');
        write_hex32(timer.timer_period);
        esp8266_hal::logger::write_byte(b'r');
        write_hex32(timer.timer_repeat_flag as u32);
    }

    if last_sig == 0x10 {
        return;
    }

    esp8266_hal::logger::write_byte(b'x');
    write_hex32(txrx as u32);
    esp8266_hal::logger::write_byte(b'y');
    write_hex32(tx_head);
    esp8266_hal::logger::write_byte(b'b');
    write_hex32(tx_bits);
    esp8266_hal::logger::write_byte(b'm');
    write_hex32(active_mask);
    esp8266_hal::logger::write_byte(b'c');
    write_hex32(first_bit);
    esp8266_hal::logger::write_byte(b'd');
    write_hex32(callback);
}

unsafe fn write_timer_trace_state() {
    const CHM_STATE_PTR: *const u8 = 0x3fff_3c24 as *const u8;

    extern "C" {
        static g_sta_state_timer_ptr: u32;
        static g_sta_retry_timer_ptr: u32;
        static g_last_timer_setfn_timer: u32;
        static g_last_timer_setfn_fn: u32;
        static g_last_timer_setfn_kind: u32;
        static g_last_ets_timer_arm_timer: u32;
        static g_last_ets_timer_arm_msec: u32;
        static g_last_ets_timer_arm_repeat: u32;
        static g_last_ets_timer_arm_kind: u32;
        static g_last_ets_timer_proc_head_timer: u32;
        static g_last_ets_timer_proc_head_fn: u32;
        static g_last_ets_timer_proc_head_kind: u32;
        static g_last_ets_timer_proc_head_arg: u32;
        static g_last_ets_timer_proc_head_expire: u32;
        static g_last_ets_timer_proc_head_period: u32;
        static g_last_ets_timer_head_cell_3234: u32;
        static g_last_ets_timer_head_cell_66c4: u32;
        static g_sta_state_timer_cb_target: u32;
        static g_sta_retry_timer_cb_target: u32;
    }

    let chm_timeout_1 = unsafe { core::ptr::read_volatile(CHM_STATE_PTR.add(4) as *const u32) };
    let chm_timeout_2 = unsafe { core::ptr::read_volatile(CHM_STATE_PTR.add(8) as *const u32) };
    let chm_start_cb = unsafe { core::ptr::read_volatile(CHM_STATE_PTR.add(16) as *const u32) };
    let chm_end_cb = unsafe { core::ptr::read_volatile(CHM_STATE_PTR.add(20) as *const u32) };
    let chm_state = unsafe { core::ptr::read_volatile(CHM_STATE_PTR.add(24) as *const u8) as u32 };
    let chm_user_cb = unsafe { core::ptr::read_volatile(CHM_STATE_PTR.add(28) as *const u32) };
    let chm_user_arg = unsafe { core::ptr::read_volatile(CHM_STATE_PTR.add(32) as *const u32) };

    esp8266_hal::logger::write_byte(b'!');
    esp8266_hal::logger::write_byte(b'i');
    write_hex32(unsafe { g_sta_state_timer_ptr });
    esp8266_hal::logger::write_byte(b'j');
    write_hex32(unsafe { g_sta_retry_timer_ptr });
    esp8266_hal::logger::write_byte(b'k');
    write_hex32(unsafe { g_last_timer_setfn_timer });
    esp8266_hal::logger::write_byte(b'l');
    write_hex32(unsafe { g_last_timer_setfn_fn });
    esp8266_hal::logger::write_byte(b'm');
    write_hex32(unsafe { g_last_timer_setfn_kind });
    esp8266_hal::logger::write_byte(b'n');
    write_hex32(unsafe { g_last_ets_timer_arm_timer });
    esp8266_hal::logger::write_byte(b'o');
    write_hex32(unsafe { g_last_ets_timer_arm_msec });
    esp8266_hal::logger::write_byte(b'p');
    write_hex32(unsafe { g_last_ets_timer_arm_repeat });
    esp8266_hal::logger::write_byte(b'q');
    write_hex32(unsafe { g_last_ets_timer_arm_kind });
    esp8266_hal::logger::write_byte(b't');
    write_hex32(unsafe { g_last_ets_timer_proc_head_timer });
    esp8266_hal::logger::write_byte(b'u');
    write_hex32(unsafe { g_last_ets_timer_proc_head_fn });
    esp8266_hal::logger::write_byte(b'v');
    write_hex32(unsafe { g_last_ets_timer_proc_head_kind });
    esp8266_hal::logger::write_byte(b'w');
    write_hex32(unsafe { g_last_ets_timer_proc_head_arg });
    esp8266_hal::logger::write_byte(b'x');
    write_hex32(unsafe { g_last_ets_timer_proc_head_expire });
    esp8266_hal::logger::write_byte(b'y');
    write_hex32(unsafe { g_last_ets_timer_proc_head_period });
    esp8266_hal::logger::write_byte(b'A');
    write_hex32(unsafe { g_last_ets_timer_head_cell_3234 });
    esp8266_hal::logger::write_byte(b'B');
    write_hex32(unsafe { g_last_ets_timer_head_cell_66c4 });
    esp8266_hal::logger::write_byte(b'C');
    write_hex32(unsafe { g_sta_state_timer_cb_target });
    esp8266_hal::logger::write_byte(b'D');
    write_hex32(unsafe { g_sta_retry_timer_cb_target });
    esp8266_hal::logger::write_byte(b'E');
    write_hex32(chm_timeout_1);
    esp8266_hal::logger::write_byte(b'F');
    write_hex32(chm_timeout_2);
    esp8266_hal::logger::write_byte(b'G');
    write_hex32(chm_start_cb);
    esp8266_hal::logger::write_byte(b'H');
    write_hex32(chm_end_cb);
    esp8266_hal::logger::write_byte(b'I');
    write_hex32(chm_state);
    esp8266_hal::logger::write_byte(b'J');
    write_hex32(chm_user_cb);
    esp8266_hal::logger::write_byte(b'K');
    write_hex32(chm_user_arg);
}

unsafe fn write_task_switch_trace() {
    extern "C" {
        static g_last_saved_task: u32;
        static g_last_saved_pc: u32;
        static g_last_saved_a0: u32;
        static g_last_saved_a1: u32;
        static g_last_restored_task: u32;
        static g_last_restored_pc: u32;
        static g_last_restored_a0: u32;
        static g_last_restored_a1: u32;
        static g_first_zero_save_task: u32;
        static g_first_zero_save_pc: u32;
        static g_first_zero_save_a0: u32;
        static g_first_zero_restore_task: u32;
        static g_first_zero_restore_a0: u32;
    }

    let last_saved_task = unsafe { g_last_saved_task };
    if last_saved_task != 0 {
        esp8266_hal::logger::write_byte(b'L');
        write_task_marker(last_saved_task);
        write_hex32(unsafe { g_last_saved_pc });
        esp8266_hal::logger::write_byte(b'l');
        write_hex32(unsafe { g_last_saved_a0 });
        esp8266_hal::logger::write_byte(b'm');
        write_hex32(unsafe { g_last_saved_a1 });
    }

    let last_restored_task = unsafe { g_last_restored_task };
    if last_restored_task != 0 {
        esp8266_hal::logger::write_byte(b'Q');
        write_task_marker(last_restored_task);
        write_hex32(unsafe { g_last_restored_pc });
        esp8266_hal::logger::write_byte(b'q');
        write_hex32(unsafe { g_last_restored_a0 });
        esp8266_hal::logger::write_byte(b'r');
        write_hex32(unsafe { g_last_restored_a1 });
    }

    let first_zero_save_task = unsafe { g_first_zero_save_task };
    if first_zero_save_task != 0 {
        esp8266_hal::logger::write_byte(b'Z');
        write_task_marker(first_zero_save_task);
        write_hex32(unsafe { g_first_zero_save_pc });
        esp8266_hal::logger::write_byte(b'z');
        write_hex32(unsafe { g_first_zero_save_a0 });
    }

    let first_zero_restore_task = unsafe { g_first_zero_restore_task };
    if first_zero_restore_task != 0 {
        esp8266_hal::logger::write_byte(b'Y');
        write_task_marker(first_zero_restore_task);
        write_hex32(unsafe { g_first_zero_restore_a0 });
    }
}

unsafe fn write_stack_window(a1: u32) {
    let base = a1 as *const u32;
    let offsets = [
        b'0', b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'A', b'B', b'C', b'D', b'E',
        b'F',
    ];

    esp8266_hal::logger::write_byte(b'~');
    for (idx, marker) in offsets.iter().enumerate() {
        esp8266_hal::logger::write_byte(*marker);
        write_hex32(unsafe { core::ptr::read_volatile(base.add(idx)) });
    }
}

#[esp8266_hal::main]
async fn main(_spawner: Spawner) {
    for _ in 0..20_000_000u32 {
        core::hint::spin_loop();
    }

    println!("wifi_connect_driver_probe start");

    let mut init_config = WifiInitConfig::default();
    init_config.rx_buf_num = 14;
    init_config.tx_buf_num = 4;

    let mut wifi = WifiController::init(init_config).expect("wifi init");
    println!("wifi init ok");

    wifi.set_storage(Storage::Ram).expect("set_storage");
    wifi.set_mode(WifiMode::Sta).expect("set_mode");

    let mut sta_cfg_builder = StaConfig::builder(WIFI_SSID, WIFI_PASS).pmf(false, false);
    if !WIFI_PASS.trim().is_empty() {
        sta_cfg_builder = sta_cfg_builder.auth_threshold(AuthMode::Wpa2Psk);
    }
    let sta_cfg = sta_cfg_builder.build();
    wifi.set_sta_config(&sta_cfg).expect("set_sta_config");

    wifi.start().expect("wifi start");
    esp8266_hal::logger::write_byte(b's');

    esp8266_hal::wifi::compat::wifi_os_adapter::arm_pp_sig_trace(4);
    if let Err(e) = wifi.connect_blocking() {
        println!("connect err {:?}", e);
        loop {
            embassy_time::Timer::after(Duration::from_secs(60)).await;
        }
    }
    esp8266_hal::logger::write_byte(b'C');
    println!("connected");

    loop {
        embassy_time::Timer::after(Duration::from_secs(60)).await;
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    extern "C" {
        static g_pp_stage: u32;
        static g_queue_recv_stage: u32;
        static g_queue_block_stage: u32;
        static g_queue_unlock_stage: u32;
        static g_queue_wait_stage: u32;
        static g_ctx_stage: u32;
        static g_wait_stage: u32;
        static g_wait_task: u32;
    }

    esp8266_hal::logger::write_byte(b'P');
    esp8266_hal::logger::write_byte(b'C');
    esp8266_hal::logger::write_byte(esp8266_hal::wifi::compat::task::debug_current_task_marker());
    let pp_stage = unsafe { g_pp_stage };
    if pp_stage != 0 {
        esp8266_hal::logger::write_byte(b'G');
        esp8266_hal::logger::write_byte(pp_stage as u8);
    }
    let queue_recv_stage = unsafe { g_queue_recv_stage };
    if queue_recv_stage != 0 {
        esp8266_hal::logger::write_byte(b'R');
        esp8266_hal::logger::write_byte(queue_recv_stage as u8);
    }
    let queue_block_stage = unsafe { g_queue_block_stage };
    if queue_block_stage != 0 {
        esp8266_hal::logger::write_byte(b'B');
        esp8266_hal::logger::write_byte(queue_block_stage as u8);
    }
    let queue_unlock_stage = unsafe { g_queue_unlock_stage };
    if queue_unlock_stage != 0 {
        esp8266_hal::logger::write_byte(b'U');
        esp8266_hal::logger::write_byte(queue_unlock_stage as u8);
    }
    let queue_wait_stage = unsafe { g_queue_wait_stage };
    if queue_wait_stage != 0 {
        esp8266_hal::logger::write_byte(b'W');
        esp8266_hal::logger::write_byte(queue_wait_stage as u8);
    }
    let ctx_stage = unsafe { g_ctx_stage };
    if ctx_stage != 0 {
        esp8266_hal::logger::write_byte(b'X');
        esp8266_hal::logger::write_byte(ctx_stage as u8);
    }
    let wait_stage = unsafe { g_wait_stage };
    if wait_stage != 0 {
        esp8266_hal::logger::write_byte(b'M');
        esp8266_hal::logger::write_byte(wait_stage as u8);
        write_task_marker(unsafe { g_wait_task });
    }
    if let Some(location) = info.location() {
        let path = location.file().as_bytes();
        let mut start = 0usize;
        let mut end = path.len();
        for (idx, &byte) in path.iter().enumerate() {
            if byte == b'/' {
                start = idx + 1;
                end = path.len();
            } else if byte == b'.' {
                end = idx;
            }
        }
        let stop = core::cmp::min(start + 16, end);
        for &byte in &path[start..stop] {
            esp8266_hal::logger::write_byte(byte);
        }
        esp8266_hal::logger::write_byte(b':');
        write_hex16(location.line() as u16);
    } else {
        esp8266_hal::logger::write_byte(b'n');
        esp8266_hal::logger::write_byte(b'o');
        esp8266_hal::logger::write_byte(b'l');
        esp8266_hal::logger::write_byte(b'o');
        esp8266_hal::logger::write_byte(b'c');
    }
    esp8266_hal::logger::write_byte(b'?');
    esp8266_hal::logger::write_byte(b'\n');
    loop {}
}

#[link_section = ".rwtext"]
#[xtensa_lx_rt::exception]
fn exception(cause: ExceptionCause, frame: Context) -> ! {
    extern "C" {
        static g_queue_recv_stage: u32;
        static g_queue_block_stage: u32;
        static g_ctx_stage: u32;
    }

    let words = &frame as *const Context as *const u32;
    let pc = unsafe { core::ptr::read(words) };
    let a0 = unsafe { core::ptr::read(words.add(2)) };
    let a1 = unsafe { core::ptr::read(words.add(3)) };
    let a12 = unsafe { core::ptr::read(words.add(14)) };
    let excvaddr = unsafe { core::ptr::read(words.add(20)) };

    esp8266_hal::logger::write_byte(b'D');
    write_hex16(cause as u32 as u16);
    esp8266_hal::logger::write_byte(b'P');
    write_hex32(pc);
    esp8266_hal::logger::write_byte(b'A');
    write_hex32(a0);
    esp8266_hal::logger::write_byte(b'V');
    write_hex32(excvaddr);
    esp8266_hal::logger::write_byte(b'T');
    write_hex32(a12);
    esp8266_hal::logger::write_byte(b'O');
    esp8266_hal::logger::write_byte(esp8266_hal::wifi::compat::task::debug_task_marker_for_sp(
        a1,
    ));
    esp8266_hal::logger::write_byte(b'C');
    esp8266_hal::logger::write_byte(esp8266_hal::wifi::compat::task::debug_current_task_marker());
    let queue_recv_stage = unsafe { g_queue_recv_stage };
    if queue_recv_stage != 0 {
        esp8266_hal::logger::write_byte(b'R');
        esp8266_hal::logger::write_byte(queue_recv_stage as u8);
    }
    let queue_block_stage = unsafe { g_queue_block_stage };
    if queue_block_stage != 0 {
        esp8266_hal::logger::write_byte(b'B');
        esp8266_hal::logger::write_byte(queue_block_stage as u8);
    }
    let ctx_stage = unsafe { g_ctx_stage };
    if ctx_stage != 0 {
        esp8266_hal::logger::write_byte(b'X');
        esp8266_hal::logger::write_byte(ctx_stage as u8);
    }
    unsafe { write_task_switch_trace() };
    unsafe { write_pp_signal4_dispatch_state() };
    esp8266_hal::logger::write_byte(b'\n');

    loop {}
}

#[no_mangle]
#[link_section = ".rwtext"]
extern "C" fn __double_exception(cause: ExceptionCause, frame: &Context) -> ! {
    extern "C" {
        static g_pp_stage: u32;
        static g_queue_recv_stage: u32;
        static g_queue_block_stage: u32;
        static g_queue_unlock_stage: u32;
        static g_queue_wait_stage: u32;
        static g_ctx_stage: u32;
        static g_last_saved_task: u32;
        static g_last_saved_pc: u32;
        static g_last_saved_a0: u32;
        static g_last_saved_a1: u32;
        static g_last_saved_ret_slot: u32;
        static g_last_saved_ret_slot_4: u32;
        static g_last_saved_ret_slot_212: u32;
        static g_last_saved_ret_slot_284: u32;
        static g_last_restored_task: u32;
        static g_last_restored_pc: u32;
        static g_last_restored_a0: u32;
        static g_last_restored_a1: u32;
        static g_last_restored_ret_slot: u32;
        static g_last_restored_ret_slot_4: u32;
        static g_last_restored_ret_slot_212: u32;
        static g_last_restored_ret_slot_284: u32;
        static g_first_zero_save_task: u32;
        static g_first_zero_save_pc: u32;
        static g_first_zero_save_a0: u32;
        static g_first_zero_restore_task: u32;
        static g_first_zero_restore_a0: u32;
    }

    let words = frame as *const Context as *const u32;
    let pc = unsafe { core::ptr::read(words) };
    let a0 = unsafe { core::ptr::read(words.add(2)) };
    let a1 = unsafe { core::ptr::read(words.add(3)) };
    let a12 = unsafe { core::ptr::read(words.add(14)) };
    let excvaddr = unsafe { core::ptr::read(words.add(20)) };

    esp8266_hal::logger::write_byte(b'D');
    write_hex16(cause as u32 as u16);
    esp8266_hal::logger::write_byte(b'P');
    write_hex32(pc);
    esp8266_hal::logger::write_byte(b'A');
    write_hex32(a0);
    esp8266_hal::logger::write_byte(b'V');
    write_hex32(excvaddr);
    esp8266_hal::logger::write_byte(b'T');
    write_hex32(a12);
    esp8266_hal::logger::write_byte(b'O');
    esp8266_hal::logger::write_byte(esp8266_hal::wifi::compat::task::debug_task_marker_for_sp(
        a1,
    ));
    esp8266_hal::logger::write_byte(b'C');
    esp8266_hal::logger::write_byte(esp8266_hal::wifi::compat::task::debug_current_task_marker());
    let queue_recv_stage = unsafe { g_queue_recv_stage };
    if queue_recv_stage != 0 {
        esp8266_hal::logger::write_byte(b'R');
        esp8266_hal::logger::write_byte(queue_recv_stage as u8);
    }
    let queue_block_stage = unsafe { g_queue_block_stage };
    if queue_block_stage != 0 {
        esp8266_hal::logger::write_byte(b'B');
        esp8266_hal::logger::write_byte(queue_block_stage as u8);
    }
    let ctx_stage = unsafe { g_ctx_stage };
    if ctx_stage != 0 {
        esp8266_hal::logger::write_byte(b'X');
        esp8266_hal::logger::write_byte(ctx_stage as u8);
    }
    unsafe { write_task_switch_trace() };
    unsafe { write_pp_signal4_dispatch_state() };
    esp8266_hal::logger::write_byte(b'\n');

    loop {}
}
