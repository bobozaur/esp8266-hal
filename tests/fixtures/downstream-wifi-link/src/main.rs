#![no_main]
#![no_std]

use core::alloc::{GlobalAlloc, Layout};
use esp8266_hal::{
    timer::TimerExt,
    wifi::{InitConfig, Runtime, Wifi, WifiAllocator},
};
use panic_halt as _;

#[cfg(any(feature = "wifi-smoltcp", feature = "wifi-embassy-net"))]
const MTU: usize = 672;

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
    fn free_dram_bytes(&self) -> usize {
        self.0.free()
    }
}

#[esp8266_hal::main]
fn main() -> ! {
    extern "C" {
        static mut _heap_start: u8;
        static mut _heap_end: u8;
    }
    // SAFETY: only the addresses of linker-defined symbols are obtained.
    let (start, end) = unsafe {
        (
            core::ptr::addr_of_mut!(_heap_start) as usize,
            core::ptr::addr_of_mut!(_heap_end) as usize,
        )
    };
    // SAFETY: initialized once before any allocation, using the linker-defined
    // free DRAM range which excludes statics and the permanent application stack.
    unsafe {
        ALLOCATOR
            .0
            .init(start as *mut u8, end.checked_sub(start).unwrap())
    };
    let peripherals = esp8266_hal::target::Peripherals::take().unwrap();
    let (_, timer2) = peripherals.TIMER.timers();
    let runtime = Runtime::init(timer2, &ALLOCATOR).unwrap();
    let config = InitConfig::default();
    #[cfg(any(feature = "wifi-smoltcp", feature = "wifi-embassy-net"))]
    let config = {
        let mut config = config;
        config.queues.frame_bytes = MTU;
        config
    };
    let mut wifi = Wifi::new(runtime, config).unwrap();
    let (mut controller, interfaces) = wifi.split();
    controller.enable_station().unwrap();
    controller.start_blocking(1_000).unwrap();
    let station = interfaces.station;

    #[cfg(feature = "wifi-smoltcp")]
    let station = {
        use smoltcp::{phy::Device, time::Instant};
        let mut tx = [0; MTU];
        let mut device = esp8266_hal::wifi::smoltcp::WifiDevice::new(station, &mut tx).unwrap();
        core::hint::black_box(device.capabilities());
        let _ = device.receive(Instant::from_millis(0));
        let _ = device.transmit(Instant::from_millis(0));
        let _ = device.poll_transmit();
        device.into_interface()
    };

    #[cfg(feature = "wifi-embassy-net")]
    {
        use core::{
            future::Future,
            task::{Context, RawWaker, RawWakerVTable, Waker},
        };
        use embassy_net_driver::Driver;
        use esp8266_hal::wifi::embassy_net::{new_with_mtu, NetDriverStateWithMtu};

        let mut state = NetDriverStateWithMtu::<MTU, 1, 1>::new();
        let (mut runner, mut device) = new_with_mtu(&mut state, station).unwrap();
        core::hint::black_box(device.capabilities());
        core::hint::black_box(device.hardware_address());

        // This link fixture polls directly and yields between polls. No-op wake
        // callbacks are sufficient because every scheduler tick polls again.
        static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
            |_| RawWaker::new(core::ptr::null(), &WAKER_VTABLE),
            |_| {},
            |_| {},
            |_| {},
        );
        // SAFETY: no callback dereferences or owns data; clones use the same
        // permanent vtable. Polling continues independently of wake callbacks.
        let waker = unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &WAKER_VTABLE)) };
        let mut context = Context::from_waker(&waker);
        let mut runner = core::pin::pin!(runner.run());
        loop {
            let _ = runner.as_mut().poll(&mut context);
            core::hint::black_box(device.link_state(&mut context));
            let _ = device.receive(&mut context);
            let _ = device.transmit(&mut context);
            esp8266_hal::wifi::delay_ticks(1);
        }
    }

    #[cfg(not(feature = "wifi-embassy-net"))]
    loop {
        core::hint::black_box(&station);
        esp8266_hal::wifi::delay_ticks(100);
    }
}
