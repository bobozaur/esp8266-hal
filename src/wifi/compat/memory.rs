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
static mut ALLOC_FAIL_COUNT: u32 = 0;
static mut FIRST_EVENT_LOOP_CALLOC_TRACE_DONE: bool = false;
static mut EAP_RSA_WORKSPACE_ACTIVE_COUNT: u32 = 0;
static mut EAP_RSA_WORKSPACE_RESERVE_ATTEMPTS: u32 = 0;
static mut EAP_RSA_WORKSPACE_RESERVED: u32 = 0;
static mut EAP_RSA_WORKSPACE_USED: u32 = 0;
static mut EAP_RSA_WORKSPACE_RELEASED: u32 = 0;
const EAP_RSA_WORKSPACE_SLOTS: usize = 3;
const EAP_RSA_WORKSPACE_SESSION_SLOTS: usize = 2;
const EAP_RSA_WORKSPACE_PRIMARY_USABLE_BYTES: usize = 1144;
const EAP_RSA_WORKSPACE_SECONDARY_USABLE_BYTES: usize = 1072;
const EAP_RSA_WORKSPACE_SMALL_USABLE_BYTES: usize =
    EAP_RSA_WORKSPACE_LATE_BLOCK_USABLE_BYTES + 3 * EAP_RSA_WORKSPACE_MEDIUM_BLOCK_USABLE_BYTES;
const EAP_RSA_WORKSPACE_BLOCKS_PER_SLOT: usize = 7;
const EAP_RSA_WORKSPACE_BLOCK0_USABLE_BYTES: usize = 508;
const EAP_RSA_WORKSPACE_BLOCK1_USABLE_BYTES: usize = 316;
const EAP_RSA_WORKSPACE_BLOCK2_USABLE_BYTES: usize = 176;
const EAP_RSA_WORKSPACE_PRIMARY_TAIL_USABLE_BYTES: usize = 72;
const EAP_RSA_WORKSPACE_LATE_BLOCK_USABLE_BYTES: usize = 176;
const EAP_RSA_WORKSPACE_SMALL_BLOCK_USABLE_BYTES: usize = 32;
const EAP_RSA_WORKSPACE_MEDIUM_BLOCK_USABLE_BYTES: usize = 72;
const EAP_RSA_WORKSPACE_EXTRA_USABLE_BYTES: usize = 32;
const EAP_RSA_WORKSPACE_EXTRA_SLOTS: usize = 4;
const EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USABLE_BYTES: usize = 32;
const EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_SLOTS: usize = 4;
static mut EAP_RSA_WORKSPACE_PTRS: [*mut c_void; EAP_RSA_WORKSPACE_SLOTS] =
    [ptr::null_mut(); EAP_RSA_WORKSPACE_SLOTS];
static mut EAP_RSA_WORKSPACE_USABLE: [usize; EAP_RSA_WORKSPACE_SLOTS] =
    [0; EAP_RSA_WORKSPACE_SLOTS];
static mut EAP_RSA_WORKSPACE_USAGE: [u8; EAP_RSA_WORKSPACE_SLOTS] = [0; EAP_RSA_WORKSPACE_SLOTS];
static mut EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_PTRS: [*mut c_void;
    EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_SLOTS] =
    [ptr::null_mut(); EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_SLOTS];
static mut EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USAGE: u8 = 0;
static mut EAP_RSA_WORKSPACE_EXTRA_CELLS: [u32; EAP_RSA_WORKSPACE_EXTRA_SLOTS
    * EAP_RSA_WORKSPACE_EXTRA_USABLE_BYTES
    / 4] = [0; EAP_RSA_WORKSPACE_EXTRA_SLOTS * EAP_RSA_WORKSPACE_EXTRA_USABLE_BYTES / 4];
static mut EAP_RSA_WORKSPACE_EXTRA_USAGE: u8 = 0;
static mut ALLOC_CONTEXT: u32 = 0;
static mut FIRST_ALLOC_FAIL_WORDS: [u32; 8] = [0; 8];
static mut LAST_ALLOC_FAIL: AllocFailureSnapshot = AllocFailureSnapshot::empty();

#[derive(Clone, Copy)]
pub(crate) struct AllocFailureSnapshot {
    pub kind: u8,
    pub requested_bytes: usize,
    pub free_bytes: usize,
    pub task_marker: u8,
    pub count: u32,
    pub eap_active_count: u32,
    pub eap_reserved: bool,
    pub eap_usable_bytes: usize,
    pub eap_in_use: bool,
    pub eap_reserve_available: bool,
    pub eap_reserved_count: usize,
    pub eap_usage_bits: u32,
    pub realloc_old_usable: usize,
    pub realloc_workspace_usable: usize,
    pub context: u32,
}

impl AllocFailureSnapshot {
    const fn empty() -> Self {
        Self {
            kind: 0,
            requested_bytes: 0,
            free_bytes: 0,
            task_marker: 0,
            count: 0,
            eap_active_count: 0,
            eap_reserved: false,
            eap_usable_bytes: 0,
            eap_in_use: false,
            eap_reserve_available: false,
            eap_reserved_count: 0,
            eap_usage_bits: 0,
            realloc_old_usable: 0,
            realloc_workspace_usable: 0,
            context: 0,
        }
    }
}

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

pub(crate) fn reserve_eap_rsa_workspace() {
    critical_section::with(|_| unsafe {
        ensure_init();
        let target_slots = if EAP_RSA_WORKSPACE_ACTIVE_COUNT == 0 {
            EAP_RSA_WORKSPACE_SESSION_SLOTS
        } else {
            EAP_RSA_WORKSPACE_SLOTS
        };
        EAP_RSA_WORKSPACE_RESERVE_ATTEMPTS = EAP_RSA_WORKSPACE_RESERVE_ATTEMPTS.saturating_add(1);
        EAP_RSA_WORKSPACE_ACTIVE_COUNT = EAP_RSA_WORKSPACE_ACTIVE_COUNT.saturating_add(1);
        ensure_eap_rsa_workspace_reserved(target_slots);
    });
}

pub(crate) fn boost_eap_rsa_workspace() {
    critical_section::with(|_| unsafe {
        ensure_init();
        if EAP_RSA_WORKSPACE_ACTIVE_COUNT == 0 {
            return;
        }

        EAP_RSA_WORKSPACE_RESERVE_ATTEMPTS = EAP_RSA_WORKSPACE_RESERVE_ATTEMPTS.saturating_add(1);
        ensure_eap_rsa_workspace_reserved(EAP_RSA_WORKSPACE_SLOTS);
    });
}

pub(crate) fn release_eap_rsa_workspace() {
    critical_section::with(|_| unsafe {
        EAP_RSA_WORKSPACE_ACTIVE_COUNT = EAP_RSA_WORKSPACE_ACTIVE_COUNT.saturating_sub(1);
        release_eap_rsa_workspace_if_idle();
    });
}

#[no_mangle]
pub unsafe extern "C" fn esp8266_hal_eap_rsa_reserve() {
    reserve_eap_rsa_workspace();
}

#[no_mangle]
pub unsafe extern "C" fn esp8266_hal_eap_rsa_boost() {
    boost_eap_rsa_workspace();
}

#[no_mangle]
pub unsafe extern "C" fn esp8266_hal_eap_rsa_release() {
    release_eap_rsa_workspace();
}

pub fn debug_eap_rsa_workspace() -> (u32, u32, u32, u32, u32, usize, bool) {
    critical_section::with(|_| unsafe {
        (
            EAP_RSA_WORKSPACE_ACTIVE_COUNT,
            EAP_RSA_WORKSPACE_RESERVE_ATTEMPTS,
            EAP_RSA_WORKSPACE_RESERVED,
            EAP_RSA_WORKSPACE_USED,
            EAP_RSA_WORKSPACE_RELEASED,
            eap_workspace_max_usable(),
            eap_workspace_any_in_use(),
        )
    })
}

unsafe fn eap_workspace_reserved_count() -> usize {
    unsafe {
        let mut count = 0;
        for index in 0..EAP_RSA_WORKSPACE_SLOTS {
            if !EAP_RSA_WORKSPACE_PTRS[index].is_null() {
                count += 1;
            }
        }
        count
    }
}

unsafe fn eap_workspace_usage_bits() -> u32 {
    unsafe {
        let mut bits = 0;
        for index in 0..EAP_RSA_WORKSPACE_SLOTS {
            bits |= (EAP_RSA_WORKSPACE_USAGE[index] as u32) << (index * 8);
        }
        bits |= (EAP_RSA_WORKSPACE_EXTRA_USAGE as u32) << 24;
        bits |= (EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USAGE as u32) << 28;
        bits
    }
}

unsafe fn eap_workspace_any_in_use() -> bool {
    unsafe {
        if EAP_RSA_WORKSPACE_EXTRA_USAGE != 0 || EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USAGE != 0 {
            return true;
        }
        for index in 0..EAP_RSA_WORKSPACE_SLOTS {
            if EAP_RSA_WORKSPACE_USAGE[index] != 0 {
                return true;
            }
        }
        false
    }
}

unsafe fn eap_workspace_max_usable() -> usize {
    unsafe {
        let mut usable = 0;
        for index in 0..EAP_RSA_WORKSPACE_SLOTS {
            if !EAP_RSA_WORKSPACE_PTRS[index].is_null() {
                usable = usable.max(EAP_RSA_WORKSPACE_USABLE[index]);
            }
        }
        usable
    }
}

unsafe fn eap_workspace_reserve_available(size: usize) -> bool {
    unsafe {
        if EAP_RSA_WORKSPACE_ACTIVE_COUNT == 0 {
            return false;
        }

        if size <= EAP_RSA_WORKSPACE_EXTRA_USABLE_BYTES && eap_workspace_extra_available() {
            return true;
        }

        if size <= EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USABLE_BYTES
            && eap_workspace_dynamic_extra_available()
        {
            return true;
        }

        find_eap_workspace_block(size).is_some()
    }
}

unsafe fn ensure_eap_rsa_workspace_reserved(target_slots: usize) {
    unsafe {
        for index in 0..target_slots.min(EAP_RSA_WORKSPACE_SLOTS) {
            if !EAP_RSA_WORKSPACE_PTRS[index].is_null() {
                continue;
            }

            let usable = eap_workspace_slot_usable(index);
            let total = usable + HEADER_SIZE;
            let layout = Layout::from_size_align_unchecked(total, 4);
            let raw = alloc_raw(layout);
            if raw.is_null() {
                return;
            }

            let header = raw as *mut u32;
            header.write(total as u32);
            header.add(1).write(FREED_MAGIC);
            EAP_RSA_WORKSPACE_PTRS[index] = raw.add(HEADER_SIZE) as *mut c_void;
            EAP_RSA_WORKSPACE_USABLE[index] = usable;
            EAP_RSA_WORKSPACE_USAGE[index] = 0;
            EAP_RSA_WORKSPACE_RESERVED = EAP_RSA_WORKSPACE_RESERVED.saturating_add(1);
        }

        if target_slots < EAP_RSA_WORKSPACE_SLOTS {
            return;
        }

        for index in 0..EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_SLOTS {
            if !EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_PTRS[index].is_null() {
                continue;
            }

            let usable = EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USABLE_BYTES;
            let total = usable + HEADER_SIZE;
            let layout = Layout::from_size_align_unchecked(total, 4);
            let raw = alloc_raw(layout);
            if raw.is_null() {
                return;
            }

            let header = raw as *mut u32;
            header.write(total as u32);
            header.add(1).write(FREED_MAGIC);
            EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_PTRS[index] = raw.add(HEADER_SIZE) as *mut c_void;
        }
    }
}

fn eap_workspace_slot_usable(index: usize) -> usize {
    if index + 1 == EAP_RSA_WORKSPACE_SLOTS {
        EAP_RSA_WORKSPACE_SMALL_USABLE_BYTES
    } else if index == 1 {
        EAP_RSA_WORKSPACE_SECONDARY_USABLE_BYTES
    } else {
        EAP_RSA_WORKSPACE_PRIMARY_USABLE_BYTES
    }
}

fn eap_workspace_block_mask(block: usize) -> u8 {
    if block < 8 {
        1u8 << block
    } else {
        0
    }
}

fn eap_workspace_extra_mask(index: usize) -> u8 {
    if index < 8 {
        1u8 << index
    } else {
        0
    }
}

fn eap_workspace_dynamic_extra_mask(index: usize) -> u8 {
    if index < 8 {
        1u8 << index
    } else {
        0
    }
}

unsafe fn eap_workspace_extra_available() -> bool {
    unsafe {
        for index in 0..EAP_RSA_WORKSPACE_EXTRA_SLOTS {
            if EAP_RSA_WORKSPACE_EXTRA_USAGE & eap_workspace_extra_mask(index) == 0 {
                return true;
            }
        }
        false
    }
}

unsafe fn eap_workspace_dynamic_extra_available() -> bool {
    unsafe {
        for index in 0..EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_SLOTS {
            if EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_PTRS[index].is_null() {
                continue;
            }
            if EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USAGE & eap_workspace_dynamic_extra_mask(index) == 0
            {
                return true;
            }
        }
        false
    }
}

unsafe fn eap_workspace_extra_ptr(index: usize) -> *mut c_void {
    if index >= EAP_RSA_WORKSPACE_EXTRA_SLOTS {
        return ptr::null_mut();
    }

    unsafe {
        let base = core::ptr::addr_of_mut!(EAP_RSA_WORKSPACE_EXTRA_CELLS) as *mut u32;
        base.add(index * EAP_RSA_WORKSPACE_EXTRA_USABLE_BYTES / 4) as *mut c_void
    }
}

unsafe fn eap_workspace_dynamic_extra_ptr(index: usize) -> *mut c_void {
    if index >= EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_SLOTS {
        return ptr::null_mut();
    }

    unsafe { EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_PTRS[index] }
}

unsafe fn eap_workspace_block_ptr(index: usize, block: usize) -> *mut c_void {
    unsafe {
        let base = EAP_RSA_WORKSPACE_PTRS[index];
        if base.is_null() || block >= EAP_RSA_WORKSPACE_BLOCKS_PER_SLOT {
            return ptr::null_mut();
        }

        (base as usize + eap_workspace_block_offset(index, block)) as *mut c_void
    }
}

fn eap_workspace_block_offset(index: usize, block: usize) -> usize {
    let mut offset = 0usize;
    for part in 0..block {
        offset = offset.saturating_add(eap_workspace_block_usable(index, part));
    }
    offset
}

fn eap_workspace_is_small_slot(index: usize) -> bool {
    index + 1 == EAP_RSA_WORKSPACE_SLOTS
}

fn eap_workspace_block_usable(index: usize, block: usize) -> usize {
    unsafe {
        if eap_workspace_is_small_slot(index) {
            let (offset, block_size): (usize, usize) = match block {
                0 => (0, EAP_RSA_WORKSPACE_LATE_BLOCK_USABLE_BYTES),
                1 => (
                    EAP_RSA_WORKSPACE_LATE_BLOCK_USABLE_BYTES,
                    EAP_RSA_WORKSPACE_MEDIUM_BLOCK_USABLE_BYTES,
                ),
                2 => (
                    EAP_RSA_WORKSPACE_LATE_BLOCK_USABLE_BYTES
                        + EAP_RSA_WORKSPACE_MEDIUM_BLOCK_USABLE_BYTES,
                    EAP_RSA_WORKSPACE_MEDIUM_BLOCK_USABLE_BYTES,
                ),
                3 => (
                    EAP_RSA_WORKSPACE_LATE_BLOCK_USABLE_BYTES
                        + 2 * EAP_RSA_WORKSPACE_MEDIUM_BLOCK_USABLE_BYTES,
                    EAP_RSA_WORKSPACE_MEDIUM_BLOCK_USABLE_BYTES,
                ),
                _ => return 0,
            };
            if EAP_RSA_WORKSPACE_USABLE[index] >= offset.saturating_add(block_size) {
                return block_size;
            }
            return 0;
        }

        match block {
            0 => EAP_RSA_WORKSPACE_USABLE[index].min(EAP_RSA_WORKSPACE_BLOCK0_USABLE_BYTES),
            1 => EAP_RSA_WORKSPACE_USABLE[index].min(EAP_RSA_WORKSPACE_BLOCK1_USABLE_BYTES),
            2 => EAP_RSA_WORKSPACE_USABLE[index].min(EAP_RSA_WORKSPACE_BLOCK2_USABLE_BYTES),
            3 if index == 0 => EAP_RSA_WORKSPACE_USABLE[index]
                .saturating_sub(EAP_RSA_WORKSPACE_BLOCK0_USABLE_BYTES)
                .saturating_sub(EAP_RSA_WORKSPACE_BLOCK1_USABLE_BYTES)
                .saturating_sub(EAP_RSA_WORKSPACE_BLOCK2_USABLE_BYTES)
                .min(EAP_RSA_WORKSPACE_PRIMARY_TAIL_USABLE_BYTES),
            3 => EAP_RSA_WORKSPACE_USABLE[index]
                .saturating_sub(EAP_RSA_WORKSPACE_BLOCK0_USABLE_BYTES)
                .saturating_sub(EAP_RSA_WORKSPACE_BLOCK1_USABLE_BYTES)
                .saturating_sub(EAP_RSA_WORKSPACE_BLOCK2_USABLE_BYTES),
            4 if index == 0 => EAP_RSA_WORKSPACE_USABLE[index]
                .saturating_sub(EAP_RSA_WORKSPACE_BLOCK0_USABLE_BYTES)
                .saturating_sub(EAP_RSA_WORKSPACE_BLOCK1_USABLE_BYTES)
                .saturating_sub(EAP_RSA_WORKSPACE_BLOCK2_USABLE_BYTES)
                .saturating_sub(EAP_RSA_WORKSPACE_PRIMARY_TAIL_USABLE_BYTES),
            _ => 0,
        }
    }
}

unsafe fn find_eap_workspace_block(size: usize) -> Option<(usize, usize)> {
    unsafe {
        let prefer_small = size <= EAP_RSA_WORKSPACE_SMALL_BLOCK_USABLE_BYTES;
        let passes = if prefer_small { 2 } else { 1 };

        for pass in 0..passes {
            let small_only = prefer_small && pass == 0;
            let mut best: Option<(usize, usize, usize)> = None;

            for rank in 0..EAP_RSA_WORKSPACE_SLOTS {
                let index = if prefer_small && rank == 0 {
                    EAP_RSA_WORKSPACE_SLOTS - 1
                } else if prefer_small {
                    rank - 1
                } else {
                    rank
                };

                if EAP_RSA_WORKSPACE_PTRS[index].is_null() || size > EAP_RSA_WORKSPACE_USABLE[index]
                {
                    continue;
                }
                for block in 0..EAP_RSA_WORKSPACE_BLOCKS_PER_SLOT {
                    let mask = eap_workspace_block_mask(block);
                    let block_size = eap_workspace_block_usable(index, block);
                    if small_only && block_size > EAP_RSA_WORKSPACE_SMALL_BLOCK_USABLE_BYTES {
                        continue;
                    }
                    if block_size < size || EAP_RSA_WORKSPACE_USAGE[index] & mask != 0 {
                        continue;
                    }

                    if best.map_or(true, |(_, _, best_size)| block_size < best_size) {
                        best = Some((index, block, block_size));
                    }
                }
            }

            if let Some((index, block, _)) = best {
                return Some((index, block));
            }
        }

        None
    }
}

unsafe fn release_eap_rsa_workspace_if_idle() {
    unsafe {
        if EAP_RSA_WORKSPACE_ACTIVE_COUNT != 0 || eap_workspace_any_in_use() {
            return;
        }

        for index in 0..EAP_RSA_WORKSPACE_SLOTS {
            let ptr = EAP_RSA_WORKSPACE_PTRS[index];
            if ptr.is_null() {
                continue;
            }

            let raw = (ptr as usize - HEADER_SIZE) as *mut u8;
            let total = EAP_RSA_WORKSPACE_USABLE[index] + HEADER_SIZE;
            let layout = Layout::from_size_align_unchecked(total, 4);
            EAP_RSA_WORKSPACE_PTRS[index] = ptr::null_mut();
            EAP_RSA_WORKSPACE_USABLE[index] = 0;
            EAP_RSA_WORKSPACE_USAGE[index] = 0;
            dealloc_raw(raw, layout);
        }

        for index in 0..EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_SLOTS {
            let ptr = EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_PTRS[index];
            if ptr.is_null() {
                continue;
            }

            let raw = (ptr as usize - HEADER_SIZE) as *mut u8;
            let total = EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USABLE_BYTES + HEADER_SIZE;
            let layout = Layout::from_size_align_unchecked(total, 4);
            EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_PTRS[index] = ptr::null_mut();
            dealloc_raw(raw, layout);
        }
        EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USAGE = 0;
    }
}

fn take_eap_rsa_workspace(size: usize) -> *mut c_void {
    unsafe {
        if EAP_RSA_WORKSPACE_ACTIVE_COUNT == 0 {
            return ptr::null_mut();
        }

        if size <= EAP_RSA_WORKSPACE_EXTRA_USABLE_BYTES {
            for extra in 0..EAP_RSA_WORKSPACE_EXTRA_SLOTS {
                let mask = eap_workspace_extra_mask(extra);
                if EAP_RSA_WORKSPACE_EXTRA_USAGE & mask == 0 {
                    EAP_RSA_WORKSPACE_EXTRA_USAGE |= mask;
                    EAP_RSA_WORKSPACE_USED = EAP_RSA_WORKSPACE_USED.saturating_add(1);
                    return eap_workspace_extra_ptr(extra);
                }
            }
        }

        if size <= EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USABLE_BYTES {
            for extra in 0..EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_SLOTS {
                let mask = eap_workspace_dynamic_extra_mask(extra);
                if EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_PTRS[extra].is_null()
                    || EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USAGE & mask != 0
                {
                    continue;
                }

                EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USAGE |= mask;
                EAP_RSA_WORKSPACE_USED = EAP_RSA_WORKSPACE_USED.saturating_add(1);
                return eap_workspace_dynamic_extra_ptr(extra);
            }
        }

        let Some((index, block)) = find_eap_workspace_block(size) else {
            return ptr::null_mut();
        };

        EAP_RSA_WORKSPACE_USAGE[index] |= eap_workspace_block_mask(block);
        EAP_RSA_WORKSPACE_USED = EAP_RSA_WORKSPACE_USED.saturating_add(1);
        eap_workspace_block_ptr(index, block)
    }
}

fn eap_rsa_workspace_usable(ptr: *mut c_void) -> Option<usize> {
    unsafe {
        for extra in 0..EAP_RSA_WORKSPACE_EXTRA_SLOTS {
            let mask = eap_workspace_extra_mask(extra);
            if EAP_RSA_WORKSPACE_EXTRA_USAGE & mask != 0 && ptr == eap_workspace_extra_ptr(extra) {
                return Some(EAP_RSA_WORKSPACE_EXTRA_USABLE_BYTES);
            }
        }

        for extra in 0..EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_SLOTS {
            let mask = eap_workspace_dynamic_extra_mask(extra);
            if EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USAGE & mask != 0
                && ptr == eap_workspace_dynamic_extra_ptr(extra)
            {
                return Some(EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USABLE_BYTES);
            }
        }

        for index in 0..EAP_RSA_WORKSPACE_SLOTS {
            for block in 0..EAP_RSA_WORKSPACE_BLOCKS_PER_SLOT {
                let mask = eap_workspace_block_mask(block);
                if ptr == eap_workspace_block_ptr(index, block)
                    && EAP_RSA_WORKSPACE_USAGE[index] & mask != 0
                {
                    return Some(eap_workspace_block_usable(index, block));
                }
            }
        }
        None
    }
}

unsafe fn grow_eap_rsa_workspace(
    ptr: *mut c_void,
    new_size: usize,
    _old_usable: usize,
) -> *mut c_void {
    unsafe {
        for extra in 0..EAP_RSA_WORKSPACE_EXTRA_SLOTS {
            let mask = eap_workspace_extra_mask(extra);
            if EAP_RSA_WORKSPACE_EXTRA_USAGE & mask != 0
                && ptr == eap_workspace_extra_ptr(extra)
                && new_size <= EAP_RSA_WORKSPACE_EXTRA_USABLE_BYTES
            {
                return ptr;
            }
        }

        for extra in 0..EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_SLOTS {
            let mask = eap_workspace_dynamic_extra_mask(extra);
            if EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USAGE & mask != 0
                && ptr == eap_workspace_dynamic_extra_ptr(extra)
                && new_size <= EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USABLE_BYTES
            {
                return ptr;
            }
        }

        for index in 0..EAP_RSA_WORKSPACE_SLOTS {
            if EAP_RSA_WORKSPACE_PTRS[index].is_null() || new_size > EAP_RSA_WORKSPACE_USABLE[index]
            {
                continue;
            }

            for block in 0..EAP_RSA_WORKSPACE_BLOCKS_PER_SLOT {
                let mask = eap_workspace_block_mask(block);
                if ptr == eap_workspace_block_ptr(index, block)
                    && EAP_RSA_WORKSPACE_USAGE[index] & mask != 0
                    && new_size <= eap_workspace_block_usable(index, block)
                {
                    return ptr;
                }
            }
        }

        ptr::null_mut()
    }
}

unsafe fn free_eap_rsa_workspace(ptr: *mut c_void) -> bool {
    unsafe {
        for extra in 0..EAP_RSA_WORKSPACE_EXTRA_SLOTS {
            let mask = eap_workspace_extra_mask(extra);
            if EAP_RSA_WORKSPACE_EXTRA_USAGE & mask != 0 && ptr == eap_workspace_extra_ptr(extra) {
                EAP_RSA_WORKSPACE_EXTRA_USAGE &= !mask;
                EAP_RSA_WORKSPACE_RELEASED = EAP_RSA_WORKSPACE_RELEASED.saturating_add(1);
                release_eap_rsa_workspace_if_idle();
                return true;
            }
        }

        for extra in 0..EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_SLOTS {
            let mask = eap_workspace_dynamic_extra_mask(extra);
            if EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USAGE & mask != 0
                && ptr == eap_workspace_dynamic_extra_ptr(extra)
            {
                EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USAGE &= !mask;
                EAP_RSA_WORKSPACE_RELEASED = EAP_RSA_WORKSPACE_RELEASED.saturating_add(1);
                release_eap_rsa_workspace_if_idle();
                return true;
            }
        }

        for index in 0..EAP_RSA_WORKSPACE_SLOTS {
            for block in 0..EAP_RSA_WORKSPACE_BLOCKS_PER_SLOT {
                let mask = eap_workspace_block_mask(block);
                if ptr == eap_workspace_block_ptr(index, block)
                    && EAP_RSA_WORKSPACE_USAGE[index] & mask != 0
                {
                    EAP_RSA_WORKSPACE_USAGE[index] &= !mask;
                    EAP_RSA_WORKSPACE_RELEASED = EAP_RSA_WORKSPACE_RELEASED.saturating_add(1);
                    release_eap_rsa_workspace_if_idle();
                    return true;
                }
            }
        }

        false
    }
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
    _stack_base: *mut u8,
    _stack_size: usize,
    _frame_base: *mut u8,
) {
}

fn trace_task3_overlap(_start: usize, _size: usize, _frame_tag: u8, _stack_tag: u8) {
    // Removed: trace bytes for task3 stack/frame overlap monitoring.
}

const FIRST_ALLOC_KIND_MASK: u32 = 0xff;
const FIRST_ALLOC_TASK_SHIFT: u32 = 8;
const FIRST_ALLOC_EAP_ACTIVE_SHIFT: u32 = 16;
const FIRST_ALLOC_EAP_COUNT_SHIFT: u32 = 24;
const FIRST_ALLOC_EAP_AVAIL_BIT: u32 = 1 << 31;

fn pack_first_alloc_head(
    kind: u8,
    task_marker: u8,
    eap_active_count: u32,
    eap_reserved_count: usize,
    eap_reserve_available: bool,
) -> u32 {
    (kind as u32)
        | ((task_marker as u32) << FIRST_ALLOC_TASK_SHIFT)
        | ((eap_active_count.min(0xff)) << FIRST_ALLOC_EAP_ACTIVE_SHIFT)
        | (((eap_reserved_count as u32).min(0x7f)) << FIRST_ALLOC_EAP_COUNT_SHIFT)
        | if eap_reserve_available {
            FIRST_ALLOC_EAP_AVAIL_BIT
        } else {
            0
        }
}

unsafe fn store_first_alloc_failure(snapshot: &AllocFailureSnapshot) {
    unsafe {
        FIRST_ALLOC_FAIL_WORDS = [
            pack_first_alloc_head(
                snapshot.kind,
                snapshot.task_marker,
                snapshot.eap_active_count,
                snapshot.eap_reserved_count,
                snapshot.eap_reserve_available,
            ),
            snapshot.requested_bytes as u32,
            snapshot.free_bytes as u32,
            snapshot.context,
            snapshot.eap_usage_bits,
            snapshot.eap_usable_bytes as u32,
            snapshot.realloc_old_usable as u32,
            snapshot.realloc_workspace_usable as u32,
        ];
    }
}

unsafe fn first_alloc_failure_snapshot(total_count: u32) -> AllocFailureSnapshot {
    unsafe {
        let head = FIRST_ALLOC_FAIL_WORDS[0];
        let eap_reserved_count = ((head >> FIRST_ALLOC_EAP_COUNT_SHIFT) & 0x7f) as usize;
        let eap_usage_bits = FIRST_ALLOC_FAIL_WORDS[4];
        AllocFailureSnapshot {
            kind: (head & FIRST_ALLOC_KIND_MASK) as u8,
            requested_bytes: FIRST_ALLOC_FAIL_WORDS[1] as usize,
            free_bytes: FIRST_ALLOC_FAIL_WORDS[2] as usize,
            task_marker: ((head >> FIRST_ALLOC_TASK_SHIFT) & 0xff) as u8,
            count: total_count,
            eap_active_count: (head >> FIRST_ALLOC_EAP_ACTIVE_SHIFT) & 0xff,
            eap_reserved: eap_reserved_count != 0,
            eap_usable_bytes: FIRST_ALLOC_FAIL_WORDS[5] as usize,
            eap_in_use: eap_usage_bits != 0,
            eap_reserve_available: head & FIRST_ALLOC_EAP_AVAIL_BIT != 0,
            eap_reserved_count,
            eap_usage_bits,
            realloc_old_usable: FIRST_ALLOC_FAIL_WORDS[6] as usize,
            realloc_workspace_usable: FIRST_ALLOC_FAIL_WORDS[7] as usize,
            context: FIRST_ALLOC_FAIL_WORDS[3],
        }
    }
}

unsafe fn record_alloc_failure_with_realloc(
    kind: u8,
    size: usize,
    realloc_old_usable: usize,
    realloc_workspace_usable: usize,
) {
    unsafe {
        let free = HEAP_HI.free() + HEAP_LO.free();
        let task = super::task::debug_current_task_marker();
        let count = ALLOC_FAIL_COUNT.saturating_add(1);
        let eap_reserved_count = eap_workspace_reserved_count();
        let snapshot = AllocFailureSnapshot {
            kind,
            requested_bytes: size,
            free_bytes: free,
            task_marker: task,
            count,
            eap_active_count: EAP_RSA_WORKSPACE_ACTIVE_COUNT,
            eap_reserved: eap_workspace_reserved_count() != 0,
            eap_usable_bytes: eap_workspace_max_usable(),
            eap_in_use: eap_workspace_any_in_use(),
            eap_reserve_available: eap_workspace_reserve_available(size),
            eap_reserved_count,
            eap_usage_bits: eap_workspace_usage_bits(),
            realloc_old_usable,
            realloc_workspace_usable,
            context: ALLOC_CONTEXT,
        };
        if ALLOC_FAIL_COUNT == 0 {
            store_first_alloc_failure(&snapshot);
        }
        LAST_ALLOC_FAIL = snapshot;
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
    c_malloc_inner_with_context(kind, size, use_reserve, 0, 0)
}

fn c_malloc_inner_with_context(
    kind: u8,
    size: usize,
    use_reserve: bool,
    realloc_old_usable: usize,
    realloc_workspace_usable: usize,
) -> *mut c_void {
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
            record_alloc_failure_with_realloc(
                kind,
                size,
                realloc_old_usable,
                realloc_workspace_usable,
            );
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
    let workspace_usable = eap_rsa_workspace_usable(ptr);
    let old_usable = if let Some(size) = workspace_usable {
        size
    } else {
        let old_total = unsafe { *((ptr as usize - HEADER_SIZE) as *const u32) } as usize;
        old_total.saturating_sub(HEADER_SIZE)
    };
    if new_size <= old_usable {
        return ptr;
    }
    if workspace_usable.is_some() {
        let grown = unsafe { grow_eap_rsa_workspace(ptr, new_size, old_usable) };
        if !grown.is_null() {
            return grown;
        }
    }

    let new_ptr = c_malloc_inner_with_context(
        b'R',
        new_size,
        true,
        old_usable,
        workspace_usable.unwrap_or(0),
    );
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
        ALLOC_CONTEXT = 0;
        FIRST_ALLOC_FAIL_WORDS = [0; 8];
        LAST_ALLOC_FAIL = AllocFailureSnapshot::empty();
        ALLOC_FAIL_COUNT = 0;
    });
}

pub(crate) fn debug_first_alloc_failure() -> Option<AllocFailureSnapshot> {
    critical_section::with(|_| unsafe {
        if ALLOC_FAIL_COUNT == 0 {
            None
        } else {
            Some(first_alloc_failure_snapshot(ALLOC_FAIL_COUNT))
        }
    })
}

pub(crate) fn debug_last_alloc_failure() -> Option<AllocFailureSnapshot> {
    critical_section::with(|_| unsafe {
        if ALLOC_FAIL_COUNT == 0 {
            None
        } else {
            let mut snapshot = LAST_ALLOC_FAIL;
            snapshot.count = ALLOC_FAIL_COUNT;
            Some(snapshot)
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
pub unsafe extern "C" fn pvPortRealloc(ptr: *mut c_void, size: u32) -> *mut c_void {
    c_realloc(ptr, size as usize)
}
#[no_mangle]
pub unsafe extern "C" fn esp8266_hal_realloc(ptr: *mut c_void, size: u32) -> *mut c_void {
    c_realloc(ptr, size as usize)
}
#[no_mangle]
pub unsafe extern "C" fn esp8266_hal_alloc_context_set(context: u32) {
    critical_section::with(|_| unsafe {
        ALLOC_CONTEXT = context;
    });
}
#[no_mangle]
pub unsafe extern "C" fn esp8266_hal_alloc_context_clear() {
    critical_section::with(|_| unsafe {
        ALLOC_CONTEXT = 0;
    });
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
