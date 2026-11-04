#![no_main]
#![no_std]

use core::alloc::{GlobalAlloc, Layout};
use esp8266_hal::{timer::TimerExt, wifi::{InitConfig, Runtime, Wifi, WifiAllocator}};
use panic_halt as _;

struct Allocator(esp_alloc::EspHeap);
#[global_allocator]
static ALLOCATOR: Allocator = Allocator(esp_alloc::EspHeap::empty());

// SAFETY: the only heap region is permanent DRAM; EspHeap serializes access.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { self.0.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { self.0.dealloc(pointer, layout) }
    }
}
unsafe impl WifiAllocator for Allocator {
    fn free_dram_bytes(&self) -> usize { self.0.free() }
}

#[esp8266_hal::main]
fn main() -> ! {
    extern "C" { static mut _heap_start: u8; static mut _heap_end: u8; }
    // SAFETY: only the addresses of linker-defined symbols are obtained.
    let (start, end) = unsafe {
        (core::ptr::addr_of_mut!(_heap_start) as usize,
         core::ptr::addr_of_mut!(_heap_end) as usize)
    };
    // SAFETY: initialized once before any allocation, using the linker-defined
    // free DRAM range which excludes statics and the permanent application stack.
    unsafe { ALLOCATOR.0.init(start as *mut u8, end.checked_sub(start).unwrap()) };
    let peripherals = esp8266_hal::target::Peripherals::take().unwrap();
    let (_, timer2) = peripherals.TIMER.timers();
    let runtime = Runtime::init(timer2, &ALLOCATOR).unwrap();
    let mut wifi = Wifi::new(runtime, InitConfig::default()).unwrap();
    let (mut controller, _interfaces) = wifi.split();
    controller.enable_station().unwrap();
    controller.start_blocking(1_000).unwrap();
    loop { esp8266_hal::wifi::delay_ticks(100); }
}
