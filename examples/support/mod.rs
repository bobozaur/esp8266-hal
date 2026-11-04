//! Application-owned runtime setup shared by the examples.
#![allow(dead_code)]

use core::alloc::{GlobalAlloc, Layout};
use esp8266_hal::{
    timer::Timer2,
    wifi::{Controller, DriverError, Runtime, WifiAllocator},
};

pub mod dhcp;
pub mod dhcp_wire;
pub mod ip;

struct Allocator(esp_alloc::EspHeap);

#[global_allocator]
static ALLOCATOR: Allocator = Allocator(esp_alloc::EspHeap::empty());

// SAFETY: EspHeap serializes access with the HAL critical section, which masks
// both ordinary interrupts and Wi-Fi NMI. Its only region is unused
// byte-addressable DRAM, kept alive for the entire boot.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { self.0.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { self.0.dealloc(pointer, layout) }
    }
}
unsafe impl WifiAllocator for Allocator {
    fn free_dram_bytes(&self) -> usize {
        self.0.free()
    }
}

/// Initialize once from the synchronous entry point. Heap and permanent stack
/// budgets come from the [linker memory map](../../ld/memory.x).
pub fn init(timer2: Timer2) -> &'static Runtime {
    extern "C" {
        static mut _heap_start: u8;
        static mut _heap_end: u8;
    }
    // SAFETY: only the addresses of linker-defined boundary symbols are read.
    let (start, end) = unsafe {
        (
            core::ptr::addr_of_mut!(_heap_start) as usize,
            core::ptr::addr_of_mut!(_heap_end) as usize,
        )
    };
    let size = end
        .checked_sub(start)
        .filter(|size| *size > 0)
        .expect("example heap does not fit DRAM");
    // SAFETY: the linker places _heap_start after all static data; the upper
    // bound excludes the live stack. Entry runs once before any allocation.
    unsafe { ALLOCATOR.0.init(start as *mut u8, size) };
    Runtime::init(timer2, &ALLOCATOR).expect("runtime init")
}

pub fn log_line(message: &[u8]) {
    esp8266_hal::logger::write_bytes(message);
    esp8266_hal::logger::write_newline();
}

pub fn log_debug(value: &impl core::fmt::Debug) {
    use core::fmt::Write;
    let _ = write!(esp8266_hal::logger::UartWriter, "{value:?}");
}

/// Example policy: at most three 20-second attempts with explicit cleanup.
pub fn connect(driver: &mut Controller<'_>) -> Result<(), DriverError> {
    let mut attempt = 1;
    loop {
        let error = match driver.connect_blocking(20_000) {
            Ok(()) => {
                log_line(b"wifi connected");
                return Ok(());
            }
            Err(error) => error,
        };
        esp8266_hal::logger::write_bytes(b"wifi connect attempt=");
        esp8266_hal::logger::write_dec_u32(attempt);
        esp8266_hal::logger::write_byte(b' ');
        log_debug(&error);
        esp8266_hal::logger::write_newline();
        if !matches!(
            &error,
            DriverError::Disconnected { .. } | DriverError::Timeout
        ) {
            return Err(error);
        }
        // A timeout only stops waiting; finish the disconnect before retrying.
        if let Err(cleanup_error) = driver.disconnect_blocking(1_000) {
            esp8266_hal::logger::write_bytes(b"wifi connect cleanup ");
            log_debug(&cleanup_error);
            esp8266_hal::logger::write_newline();
            return Err(cleanup_error);
        }
        if attempt == 3 {
            return Err(error);
        }
        esp8266_hal::wifi::delay_ticks(25); // 250 ms with the runtime's 10 ms tick.
        attempt += 1;
    }
}
