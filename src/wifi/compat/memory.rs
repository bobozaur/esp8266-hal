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

#[cfg(feature = "wifi-enterprise")]
mod eap_workspace {
    use super::*;

    static mut EAP_RSA_WORKSPACE_ACTIVE_COUNT: u32 = 0;
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
    static mut EAP_RSA_WORKSPACE_USAGE: [u8; EAP_RSA_WORKSPACE_SLOTS] =
        [0; EAP_RSA_WORKSPACE_SLOTS];
    static mut EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_PTRS: [*mut c_void;
        EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_SLOTS] =
        [ptr::null_mut(); EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_SLOTS];
    static mut EAP_RSA_WORKSPACE_DYNAMIC_EXTRA_USAGE: u8 = 0;
    static mut EAP_RSA_WORKSPACE_EXTRA_CELLS: [u32; EAP_RSA_WORKSPACE_EXTRA_SLOTS
        * EAP_RSA_WORKSPACE_EXTRA_USABLE_BYTES
        / 4] = [0; EAP_RSA_WORKSPACE_EXTRA_SLOTS * EAP_RSA_WORKSPACE_EXTRA_USABLE_BYTES / 4];
    static mut EAP_RSA_WORKSPACE_EXTRA_USAGE: u8 = 0;

    pub(crate) fn reserve_eap_rsa_workspace() {
        critical_section::with(|_| unsafe {
            ensure_init();
            let target_slots = if EAP_RSA_WORKSPACE_ACTIVE_COUNT == 0 {
                EAP_RSA_WORKSPACE_SESSION_SLOTS
            } else {
                EAP_RSA_WORKSPACE_SLOTS
            };
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

                    if EAP_RSA_WORKSPACE_PTRS[index].is_null()
                        || size > EAP_RSA_WORKSPACE_USABLE[index]
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

    pub(super) fn take_eap_rsa_workspace(size: usize) -> *mut c_void {
        unsafe {
            if EAP_RSA_WORKSPACE_ACTIVE_COUNT == 0 {
                return ptr::null_mut();
            }

            if size <= EAP_RSA_WORKSPACE_EXTRA_USABLE_BYTES {
                for extra in 0..EAP_RSA_WORKSPACE_EXTRA_SLOTS {
                    let mask = eap_workspace_extra_mask(extra);
                    if EAP_RSA_WORKSPACE_EXTRA_USAGE & mask == 0 {
                        EAP_RSA_WORKSPACE_EXTRA_USAGE |= mask;
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
                    return eap_workspace_dynamic_extra_ptr(extra);
                }
            }

            let Some((index, block)) = find_eap_workspace_block(size) else {
                return ptr::null_mut();
            };

            EAP_RSA_WORKSPACE_USAGE[index] |= eap_workspace_block_mask(block);
            eap_workspace_block_ptr(index, block)
        }
    }

    pub(super) fn eap_rsa_workspace_usable(ptr: *mut c_void) -> Option<usize> {
        unsafe {
            for extra in 0..EAP_RSA_WORKSPACE_EXTRA_SLOTS {
                let mask = eap_workspace_extra_mask(extra);
                if EAP_RSA_WORKSPACE_EXTRA_USAGE & mask != 0
                    && ptr == eap_workspace_extra_ptr(extra)
                {
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

    pub(super) unsafe fn grow_eap_rsa_workspace(ptr: *mut c_void, new_size: usize) -> *mut c_void {
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
                if EAP_RSA_WORKSPACE_PTRS[index].is_null()
                    || new_size > EAP_RSA_WORKSPACE_USABLE[index]
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

    pub(super) unsafe fn free_eap_rsa_workspace(ptr: *mut c_void) -> bool {
        unsafe {
            for extra in 0..EAP_RSA_WORKSPACE_EXTRA_SLOTS {
                let mask = eap_workspace_extra_mask(extra);
                if EAP_RSA_WORKSPACE_EXTRA_USAGE & mask != 0
                    && ptr == eap_workspace_extra_ptr(extra)
                {
                    EAP_RSA_WORKSPACE_EXTRA_USAGE &= !mask;
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
                        release_eap_rsa_workspace_if_idle();
                        return true;
                    }
                }
            }

            false
        }
    }
}

#[cfg(feature = "wifi-enterprise")]
use eap_workspace::{
    eap_rsa_workspace_usable, free_eap_rsa_workspace, grow_eap_rsa_workspace,
    take_eap_rsa_workspace,
};
#[cfg(feature = "wifi-enterprise")]
pub(crate) use eap_workspace::{release_eap_rsa_workspace, reserve_eap_rsa_workspace};

unsafe impl GlobalAlloc for SplitHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { alloc_raw(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { dealloc_raw(ptr, layout) }
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

fn c_malloc(size: usize) -> *mut c_void {
    if size == 0 {
        return ptr::null_mut();
    }
    let Some(total) = size.checked_add(HEADER_SIZE) else {
        return ptr::null_mut();
    };
    critical_section::with(|_| unsafe {
        ensure_init();
        let layout = match Layout::from_size_align(total, 4) {
            Ok(l) => l,
            Err(_) => return ptr::null_mut(),
        };
        let raw = alloc_raw(layout);
        if raw.is_null() {
            #[cfg(feature = "wifi-enterprise")]
            {
                let reserved = take_eap_rsa_workspace(size);
                if !reserved.is_null() {
                    return reserved;
                }
            }
            return ptr::null_mut();
        }
        let header = raw as *mut u32;
        header.write(total as u32);
        header.add(1).write(ALLOC_MAGIC);
        let user = (raw as usize + HEADER_SIZE) as *mut c_void;
        user
    })
}

fn c_free(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    critical_section::with(|_| unsafe {
        #[cfg(feature = "wifi-enterprise")]
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
        return c_malloc(new_size);
    }
    if new_size == 0 {
        c_free(ptr);
        return ptr::null_mut();
    }
    #[cfg(feature = "wifi-enterprise")]
    let workspace_usable = eap_rsa_workspace_usable(ptr);
    #[cfg(not(feature = "wifi-enterprise"))]
    let workspace_usable: Option<usize> = None;
    let old_usable = if let Some(size) = workspace_usable {
        size
    } else {
        let old_total = unsafe { *((ptr as usize - HEADER_SIZE) as *const u32) } as usize;
        old_total.saturating_sub(HEADER_SIZE)
    };
    if new_size <= old_usable {
        return ptr;
    }
    #[cfg(feature = "wifi-enterprise")]
    if workspace_usable.is_some() {
        let grown = unsafe { grow_eap_rsa_workspace(ptr, new_size) };
        if !grown.is_null() {
            return grown;
        }
    }

    let new_ptr = c_malloc(new_size);
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
    let Some(total) = count.checked_mul(size) else {
        return ptr::null_mut();
    };
    let p = c_malloc(total);
    if !p.is_null() {
        unsafe { ptr::write_bytes(p as *mut u8, 0, total) };
    }
    p
}

pub unsafe fn zalloc(size: usize) -> *mut c_void {
    let p = c_malloc(size);
    if !p.is_null() {
        unsafe { ptr::write_bytes(p as *mut u8, 0, size) };
    }
    p
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
pub unsafe extern "C" fn pvPortCalloc(count: u32, size: u32) -> *mut c_void {
    unsafe { calloc(count as usize, size as usize) }
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
    total_free() as u32
}

#[no_mangle]
pub unsafe extern "C" fn esp_get_minimum_free_heap_size() -> u32 {
    total_free() as u32
}
