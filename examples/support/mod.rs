//! Application-owned runtime setup shared by the examples.
#![allow(dead_code)]

use core::alloc::{GlobalAlloc, Layout};
use esp8266_hal::{
    gpio::GpioExt,
    target::{GPIO, UART0},
    timer::Timer2,
    uart::{UART0Ext, UART0Serial},
    wifi::{Runtime, WifiAllocator},
};

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

/// Own UART0 and its pins at the application's 115200 baud rate.
pub fn serial(uart: UART0, gpio: GPIO) -> UART0Serial {
    // SAFETY: the divider fits the field; the owned UART has not been shared.
    uart.uart_clkdiv()
        .write(|w| unsafe { w.uart_clkdiv().bits(80_000_000 / 115_200) });
    let pins = gpio.split();
    uart.serial(pins.gpio1.into_uart(), pins.gpio3.into_uart())
}
