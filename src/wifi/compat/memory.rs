//! Heap allocator for WiFi blob memory needs.
//!
//! Uses `esp-alloc` (the same allocator crate used on ESP32) backed by the
//! DRAM region between `_heap_start` and `_heap_end` from the linker script.
//!
//! We export `pvPortMalloc` / `vPortFree` / `pvPortCalloc` / `pvPortZalloc`
//! and `_heap_caps_*` as C-compatible symbols for the WiFi blob and SDK code.

use core::alloc::{GlobalAlloc, Layout};
use core::ffi::c_void;
use core::ptr;

use esp_alloc::EspHeap;

// ---------------------------------------------------------------------------
// Global heap
// ---------------------------------------------------------------------------

/// Top of DRAM minus a temporary reservation for the boot stack.
///
/// The ESP8266 DRAM ends at 0x4000_0000. The main task (embassy executor)
/// starts on the CPU boot stack which grows downward from the top of DRAM.
/// Until execution has switched onto the heap-backed `uiT` task stack, the
/// allocator must not hand out that top-of-DRAM region or it will collide
/// with the live boot stack.
///
/// 3 KiB reservation: WiFi init still succeeds (needs ~21 KB heap), and the
/// remaining gap between the top heap allocation and the boot-stack low-water
/// SP (~0x3fffe_e00) is ~1.5 KiB — enough to prevent stack/heap overlap
/// before the first task switch. Once `uiT` is running this region can be
/// reclaimed safely.
const DRAM_TOP: usize = 0x4000_0000;
const BOOT_STACK_RESERVE_BYTES: usize = 3072;
const DRAM_LIMIT: usize = DRAM_TOP - BOOT_STACK_RESERVE_BYTES;
const LOW_HEAP_START: usize = 0;

struct SplitHeap;

#[global_allocator]
static HEAP: SplitHeap = SplitHeap;
static HEAP_HI: EspHeap = EspHeap::empty();
static HEAP_LO: EspHeap = EspHeap::empty();

static mut HEAP_INITIALIZED: bool = false;
static mut HEAP_HI_START: usize = 0;
static mut HEAP_HI_END: usize = 0;
static mut HEAP_LO_START: usize = 0;
static mut HEAP_LO_END: usize = 0;
static mut BOOT_STACK_REGION_RECLAIMED: bool = false;
static mut TASK3_STACK_MONITOR_ACTIVE: bool = false;
static mut TASK3_STACK_START: usize = 0;
static mut TASK3_STACK_END: usize = 0;
static mut TASK3_FRAME_START: usize = 0;
static mut TASK3_FRAME_END: usize = 0;
static mut FIRST_ALLOC_FAIL_KIND: u8 = 0;
static mut FIRST_ALLOC_FAIL_TASK: u8 = 0;
static mut FIRST_ALLOC_FAIL_SIZE: usize = 0;
static mut FIRST_ALLOC_FAIL_FREE: usize = 0;
static mut ALLOC_FAIL_COUNT: u32 = 0;
static mut FIRST_EVENT_LOOP_CALLOC_TRACE_DONE: bool = false;
static mut EAP_RSA_WORKSPACE_ACTIVE: bool = false;
static mut EAP_RSA_WORKSPACE_RESERVE_ATTEMPTS: u32 = 0;
static mut EAP_RSA_WORKSPACE_RESERVED: u32 = 0;
static mut EAP_RSA_WORKSPACE_USED: u32 = 0;
static mut EAP_RSA_WORKSPACE_RELEASED: u32 = 0;

/// Initialize the heap from linker-provided symbols if not already done.
unsafe fn ensure_init() {
    unsafe {
        if HEAP_INITIALIZED {
            return;
        }
        HEAP_INITIALIZED = true;

        extern "C" {
            static _data_start: u8;
            static _heap_start: u8;
        }

        let hi_start = &_heap_start as *const u8 as usize;
        let hi_end = DRAM_LIMIT;
        let lo_start = LOW_HEAP_START;
        let lo_end = if lo_start == 0 {
            0
        } else {
            &_data_start as *const u8 as usize
        };

        HEAP_HI_START = hi_start;
        HEAP_HI_END = hi_end;
        HEAP_LO_START = lo_start;
        HEAP_LO_END = lo_end;

        if hi_end > hi_start {
            HEAP_HI.init(hi_start as *mut u8, hi_end - hi_start);
        }
        if lo_end > lo_start {
            HEAP_LO.init(lo_start as *mut u8, lo_end - lo_start);
        }
    }
}

/// Explicitly initialize the heap. Called early in boot so that both Rust
/// `alloc` and C `malloc` work immediately.
pub fn init() {
    critical_section::with(|_| unsafe { ensure_init() });
}

pub fn reclaim_boot_stack_reservation() {
    critical_section::with(|_| unsafe {
        ensure_init();

        if BOOT_STACK_REGION_RECLAIMED {
            return;
        }

        HEAP_LO.init(DRAM_LIMIT as *mut u8, BOOT_STACK_RESERVE_BYTES);
        HEAP_LO_START = DRAM_LIMIT;
        HEAP_LO_END = DRAM_TOP;
        BOOT_STACK_REGION_RECLAIMED = true;
    });
}

fn ptr_in_region(ptr: *mut u8, start: usize, end: usize) -> bool {
    let addr = ptr as usize;
    addr >= start && addr < end
}

unsafe fn alloc_raw(layout: Layout) -> *mut u8 {
    unsafe {
        critical_section::with(|_| ensure_init());

        if BOOT_STACK_REGION_RECLAIMED {
            let ptr = HEAP_LO.alloc(layout);
            if !ptr.is_null() {
                return ptr;
            }

            return HEAP_HI.alloc(layout);
        }

        let ptr = HEAP_HI.alloc(layout);
        if !ptr.is_null() {
            return ptr;
        }

        HEAP_LO.alloc(layout)
    }
}

unsafe fn dealloc_raw(ptr: *mut u8, layout: Layout) {
    unsafe {
        critical_section::with(|_| ensure_init());

        if ptr_in_region(ptr, HEAP_HI_START, HEAP_HI_END) {
            HEAP_HI.dealloc(ptr, layout);
        } else if ptr_in_region(ptr, HEAP_LO_START, HEAP_LO_END) {
            HEAP_LO.dealloc(ptr, layout);
        }
    }
}

fn total_free() -> usize {
    HEAP_HI.free() + HEAP_LO.free()
}

fn take_eap_rsa_workspace(size: usize) -> *mut c_void {
    let _ = size;
    ptr::null_mut()
}

pub(crate) fn reserve_eap_rsa_workspace() {
    critical_section::with(|_| unsafe {
        ensure_init();
        EAP_RSA_WORKSPACE_RESERVE_ATTEMPTS = EAP_RSA_WORKSPACE_RESERVE_ATTEMPTS.saturating_add(1);
        EAP_RSA_WORKSPACE_ACTIVE = true;
    });
}

pub(crate) fn release_eap_rsa_workspace() {
    critical_section::with(|_| unsafe {
        EAP_RSA_WORKSPACE_ACTIVE = false;
    });
}

#[no_mangle]
pub unsafe extern "C" fn esp8266_hal_eap_rsa_reserve() {
    reserve_eap_rsa_workspace();
}

#[no_mangle]
pub unsafe extern "C" fn esp8266_hal_eap_rsa_release() {
    release_eap_rsa_workspace();
}

pub fn debug_eap_rsa_workspace() -> (bool, u32, u32, u32, u32) {
    critical_section::with(|_| unsafe {
        (
            EAP_RSA_WORKSPACE_ACTIVE,
            EAP_RSA_WORKSPACE_RESERVE_ATTEMPTS,
            EAP_RSA_WORKSPACE_RESERVED,
            EAP_RSA_WORKSPACE_USED,
            EAP_RSA_WORKSPACE_RELEASED,
        )
    })
}

fn eap_rsa_workspace_usable(ptr: *mut c_void) -> Option<usize> {
    let _ = ptr;
    None
}

unsafe fn free_eap_rsa_workspace(ptr: *mut c_void) -> bool {
    let _ = ptr;
    false
}

pub fn debug_region_info() -> (usize, usize, usize, usize, usize, usize) {
    critical_section::with(|_| unsafe {
        ensure_init();
        (
            HEAP_HI_START,
            HEAP_HI_END,
            HEAP_HI.free(),
            HEAP_LO_START,
            HEAP_LO_END,
            HEAP_LO.free(),
        )
    })
}

pub fn debug_heap_headers() -> (u32, u32, u32, u32) {
    critical_section::with(|_| unsafe {
        ensure_init();

        let hi_size = if HEAP_HI_START < HEAP_HI_END {
            (HEAP_HI_START as *const u32).read_volatile()
        } else {
            0
        };
        let hi_next = if HEAP_HI_START < HEAP_HI_END {
            (HEAP_HI_START as *const u32).add(1).read_volatile()
        } else {
            0
        };
        let lo_size = if HEAP_LO_START < HEAP_LO_END {
            (HEAP_LO_START as *const u32).read_volatile()
        } else {
            0
        };
        let lo_next = if HEAP_LO_START < HEAP_LO_END {
            (HEAP_LO_START as *const u32).add(1).read_volatile()
        } else {
            0
        };

        (hi_size, hi_next, lo_size, lo_next)
    })
}

pub fn debug_low_raw_probe() -> (usize, u32, u32, u32) {
    critical_section::with(|_| unsafe {
        ensure_init();

        if HEAP_LO_START >= HEAP_LO_END {
            return (0, 0, 0, 0);
        }

        if BOOT_STACK_REGION_RECLAIMED {
            return (HEAP_LO_START + 0x20, 0, 0, 0);
        }

        let probe = (HEAP_LO_START + 0x20) as *mut u32;
        let original = probe.read_volatile();
        probe.write_volatile(0xA5A5_5A5A);
        let after_a = probe.read_volatile();
        probe.write_volatile(0x5A5A_A5A5);
        let after_b = probe.read_volatile();
        probe.write_volatile(original);

        (probe as usize, original, after_a, after_b)
    })
}

unsafe impl GlobalAlloc for SplitHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { alloc_raw(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { dealloc_raw(ptr, layout) }
    }
}

pub(crate) unsafe fn monitor_task3_stack(
    stack_base: *mut u8,
    stack_size: usize,
    frame_base: *mut u8,
) {
    unsafe {
        TASK3_STACK_MONITOR_ACTIVE = true;
        TASK3_STACK_START = stack_base as usize;
        TASK3_STACK_END = TASK3_STACK_START.saturating_add(stack_size);
        TASK3_FRAME_START = frame_base as usize;
        TASK3_FRAME_END = TASK3_FRAME_START.saturating_add(0x100);
    }
}

fn ranges_overlap(a_start: usize, a_end: usize, b_start: usize, b_end: usize) -> bool {
    a_start < b_end && b_start < a_end
}

fn trace_task3_overlap(_start: usize, _size: usize, _frame_tag: u8, _stack_tag: u8) {
    // Removed: trace bytes for task3 stack/frame overlap monitoring.
}

unsafe fn record_alloc_failure(kind: u8, size: usize) {
    unsafe {
        let free = HEAP_HI.free() + HEAP_LO.free();
        if ALLOC_FAIL_COUNT == 0 {
            FIRST_ALLOC_FAIL_KIND = kind;
            FIRST_ALLOC_FAIL_TASK = super::task::debug_current_task_marker();
            FIRST_ALLOC_FAIL_SIZE = size;
            FIRST_ALLOC_FAIL_FREE = free;
        }
        ALLOC_FAIL_COUNT = ALLOC_FAIL_COUNT.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Size-tracking wrapper
//
// GlobalAlloc::dealloc requires a Layout, but C code calls free(ptr) without
// a size. We store the allocation size in a 4-byte header before the pointer.
// ---------------------------------------------------------------------------

const HEADER_WORDS: usize = 2;
const HEADER_SIZE: usize = HEADER_WORDS * core::mem::size_of::<u32>();
const ALLOC_MAGIC: u32 = 0x8266_a110;
const FREED_MAGIC: u32 = 0xdead_f833;

fn raw_in_heap_regions(raw: *mut u8) -> bool {
    unsafe {
        ptr_in_region(raw, HEAP_HI_START, HEAP_HI_END)
            || ptr_in_region(raw, HEAP_LO_START, HEAP_LO_END)
    }
}

fn c_malloc_inner(kind: u8, size: usize) -> *mut c_void {
    c_malloc_inner_with_reserve(kind, size, true)
}

fn c_malloc_inner_with_reserve(kind: u8, size: usize, use_reserve: bool) -> *mut c_void {
    if size == 0 {
        return ptr::null_mut();
    }
    let total = size + HEADER_SIZE;
    critical_section::with(|_| unsafe {
        ensure_init();
        let layout = match Layout::from_size_align(total, 4) {
            Ok(l) => l,
            Err(_) => return ptr::null_mut(),
        };
        let raw = alloc_raw(layout);
        if raw.is_null() {
            if use_reserve {
                let reserved = take_eap_rsa_workspace(size);
                if !reserved.is_null() {
                    super::pp_stage::note_deliver_probe_alloc(size, true);
                    trace_task3_overlap(reserved as usize, size, b'V', b'v');
                    return reserved;
                }
            }
            super::pp_stage::note_deliver_probe_alloc(size, false);
            record_alloc_failure(kind, size);
            return ptr::null_mut();
        }
        super::pp_stage::note_deliver_probe_alloc(size, true);
        let header = raw as *mut u32;
        header.write(total as u32);
        header.add(1).write(ALLOC_MAGIC);
        let user = (raw as usize + HEADER_SIZE) as *mut c_void;
        trace_task3_overlap(user as usize, size, b'V', b'v');
        user
    })
}

fn c_malloc(size: usize) -> *mut c_void {
    c_malloc_inner(b'M', size)
}

fn c_free(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    critical_section::with(|_| unsafe {
        if free_eap_rsa_workspace(ptr) {
            return;
        }

        let raw = (ptr as usize).saturating_sub(HEADER_SIZE) as *mut u8;
        ensure_init();
        if !raw_in_heap_regions(raw) {
            return;
        }

        let header = raw as *mut u32;
        let total = header.read() as usize;
        let magic = header.add(1).read();
        if magic != ALLOC_MAGIC || total < HEADER_SIZE {
            return;
        }

        header.add(1).write(FREED_MAGIC);
        let layout = Layout::from_size_align_unchecked(total, 4);
        dealloc_raw(raw, layout);
    })
}

fn c_realloc(ptr: *mut c_void, new_size: usize) -> *mut c_void {
    if ptr.is_null() {
        return c_malloc_inner(b'R', new_size);
    }
    if new_size == 0 {
        c_free(ptr);
        return ptr::null_mut();
    }
    let old_usable = if let Some(size) = eap_rsa_workspace_usable(ptr) {
        size
    } else {
        let old_total = unsafe { *((ptr as usize - HEADER_SIZE) as *const u32) } as usize;
        old_total.saturating_sub(HEADER_SIZE)
    };

    let new_ptr = c_malloc_inner(b'R', new_size);
    if !new_ptr.is_null() {
        unsafe {
            ptr::copy_nonoverlapping(
                ptr as *const u8,
                new_ptr as *mut u8,
                old_usable.min(new_size),
            );
        }
        c_free(ptr);
    }
    new_ptr
}

// ---------------------------------------------------------------------------
// Public API (used by compat layer internals)
// ---------------------------------------------------------------------------

pub unsafe fn malloc(size: usize) -> *mut c_void {
    c_malloc(size)
}
pub unsafe fn free(ptr: *mut c_void) {
    c_free(ptr)
}

pub unsafe fn calloc(count: usize, size: usize) -> *mut c_void {
    let total = count.wrapping_mul(size);
    let p = c_malloc_inner(b'C', total);
    if !p.is_null() {
        unsafe { ptr::write_bytes(p as *mut u8, 0, total) };
        trace_task3_overlap(p as usize, total, b'Z', b'z');
    }
    p
}

pub unsafe fn zalloc(size: usize) -> *mut c_void {
    let p = c_malloc_inner(b'Z', size);
    if !p.is_null() {
        unsafe { ptr::write_bytes(p as *mut u8, 0, size) };
        trace_task3_overlap(p as usize, size, b'Z', b'z');
    }
    p
}

pub fn clear_alloc_failure_debug() {
    critical_section::with(|_| unsafe {
        FIRST_ALLOC_FAIL_KIND = 0;
        FIRST_ALLOC_FAIL_TASK = 0;
        FIRST_ALLOC_FAIL_SIZE = 0;
        FIRST_ALLOC_FAIL_FREE = 0;
        ALLOC_FAIL_COUNT = 0;
    });
}

pub fn debug_last_alloc_failure() -> Option<(u8, usize, usize, u8, u32)> {
    critical_section::with(|_| unsafe {
        if ALLOC_FAIL_COUNT == 0 {
            None
        } else {
            Some((
                FIRST_ALLOC_FAIL_KIND,
                FIRST_ALLOC_FAIL_SIZE,
                FIRST_ALLOC_FAIL_FREE,
                FIRST_ALLOC_FAIL_TASK,
                ALLOC_FAIL_COUNT,
            ))
        }
    })
}

// ---------------------------------------------------------------------------
// FreeRTOS-compatible symbols
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn pvPortMalloc(size: u32) -> *mut c_void {
    c_malloc(size as usize)
}
#[no_mangle]
pub unsafe extern "C" fn vPortFree(ptr: *mut c_void) {
    c_free(ptr)
}
#[no_mangle]
pub unsafe extern "C" fn pvPortCalloc(count: u32, size: u32) -> *mut c_void {
    unsafe {
        let trace = !FIRST_EVENT_LOOP_CALLOC_TRACE_DONE
            && count == 1
            && size == 0x18
            && super::task::debug_current_task_marker() == b'U';
        if trace {
            crate::logger::write_byte(b'8');
        }
        let p = calloc(count as usize, size as usize);
        if trace {
            FIRST_EVENT_LOOP_CALLOC_TRACE_DONE = true;
            crate::logger::write_byte(if p.is_null() { b'X' } else { b'9' });
        }
        p
    }
}
#[no_mangle]
pub unsafe extern "C" fn pvPortZalloc(size: u32) -> *mut c_void {
    unsafe { zalloc(size as usize) }
}

// ---------------------------------------------------------------------------
// heap_caps_* symbols (SDK C code)
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn _heap_caps_malloc(
    size: usize,
    _caps: u32,
    _file: *const u8,
    _line: usize,
) -> *mut c_void {
    c_malloc(size)
}

#[no_mangle]
pub unsafe extern "C" fn _heap_caps_free(ptr: *mut c_void, _file: *const u8, _line: usize) {
    c_free(ptr)
}

#[no_mangle]
pub unsafe extern "C" fn _heap_caps_calloc(
    count: usize,
    size: usize,
    _caps: u32,
    _file: *const u8,
    _line: usize,
) -> *mut c_void {
    unsafe { calloc(count, size) }
}

#[no_mangle]
pub unsafe extern "C" fn _heap_caps_zalloc(
    size: usize,
    _caps: u32,
    _file: *const u8,
    _line: usize,
) -> *mut c_void {
    unsafe { zalloc(size) }
}

#[no_mangle]
pub unsafe extern "C" fn _heap_caps_realloc(
    ptr: *mut c_void,
    size: usize,
    _caps: u32,
    _file: *const u8,
    _line: usize,
) -> *mut c_void {
    c_realloc(ptr, size)
}

#[no_mangle]
pub unsafe extern "C" fn heap_caps_init() {}

#[no_mangle]
pub unsafe extern "C" fn heap_caps_get_dram_free_size() -> u32 {
    total_free() as u32
}

#[no_mangle]
pub unsafe extern "C" fn esp_get_free_heap_size() -> u32 {
    let free = total_free() as u32;
    unsafe { super::pp_stage::note_deliver_probe_free_heap(free) };
    unsafe { super::pp_stage::adjust_deliver_free_heap(free) }
}

#[no_mangle]
pub unsafe extern "C" fn esp_get_minimum_free_heap_size() -> u32 {
    total_free() as u32
}
