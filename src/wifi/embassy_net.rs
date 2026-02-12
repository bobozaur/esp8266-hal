//! Embassy-net driver integration via `embassy-net-driver-channel`.
//!
//! This module bridges [`WifiDevice`](crate::wifi::device::WifiDevice) with
//! [`embassy-net`] by providing a channel-based driver. A background
//! [`Runner`] task moves packets between the WiFi hardware and the channel.
//!
//! # Usage
//!
//! ```rust,no_run
//! use esp8266_wifi::embassy_net;
//! use esp8266_wifi::controller::WifiInterface;
//! use esp8266_wifi::device::WifiDevice;
//!
//! let device = WifiDevice::new(WifiInterface::Sta);
//! device.register_rx_callback().unwrap();
//!
//! static STATE: embassy_net::NetDriverState = embassy_net::NetDriverState::new();
//!
//! let mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
//! let (runner, net_device) = embassy_net::new(&mut STATE, mac, device);
//!
//! // Spawn `runner.run()` as an embassy task.
//! // Pass `net_device` to `embassy_net::Stack::new()`.
//! ```

use embassy_net_driver_channel as ch;

use crate::wifi::controller::StaConnectionState;
use crate::wifi::device::WifiDevice;
use crate::wifi::event::{self, WifiEventKind};

/// Maximum Ethernet frame size (MTU 1500 + 14-byte header).
const MTU: usize = 1514;

/// Number of RX buffers in the channel.
const N_RX: usize = 2;

/// Number of TX buffers in the channel.
const N_TX: usize = 2;

/// lwIP's temporary "try again later" error.
const ERR_MEM: i32 = -1;

#[no_mangle]
pub static mut g_net_runner_stage: u32 = 0;

#[inline(always)]
fn record_net_runner_stage(stage: u8) {
    unsafe { g_net_runner_stage = stage as u32 };
}

/// State storage for the network driver channel.
///
/// Must be `'static` — use a `static` variable or `StaticCell`.
pub type NetDriverState = ch::State<MTU, N_RX, N_TX>;

/// The network device type for use with `embassy_net::Stack`.
pub type NetDevice<'d> = ch::Device<'d, MTU>;

/// Create a new embassy-net driver backed by a [`WifiDevice`].
///
/// Returns a [`Runner`] (spawn as a background task) and a [`NetDevice`]
/// (pass to `embassy_net::Stack::new()`).
///
/// - `state`: Static storage for the channel buffers.
/// - `mac`: The 6-byte MAC address of the WiFi interface.
/// - `device`: The WiFi device (must have RX callback already registered).
pub fn new<'d>(
    state: &'d mut NetDriverState,
    mac: [u8; 6],
    device: WifiDevice,
) -> (Runner<'d>, NetDevice<'d>) {
    let (runner, net_device) = ch::new(state, ch::driver::HardwareAddress::Ethernet(mac));

    let runner = Runner {
        ch_runner: runner,
        device,
    };

    (runner, net_device)
}

/// Background task that bridges the WiFi hardware with the embassy-net channel.
///
/// Must be spawned as an embassy task. Runs forever, moving packets between
/// the WiFi RX ring buffer / TX path and the channel.
pub struct Runner<'d> {
    ch_runner: ch::Runner<'d, MTU>,
    device: WifiDevice,
}

impl<'d> Runner<'d> {
    /// Run the driver bridge. Never returns.
    ///
    /// This task must be spawned as a background embassy task:
    ///
    /// ```rust,no_run
    /// #[embassy_executor::task]
    /// async fn net_driver(mut runner: esp8266_wifi::embassy_net::Runner<'static>) {
    ///     runner.run().await;
    /// }
    /// ```
    pub async fn run(&mut self) -> ! {
        record_net_runner_stage(b'I');
        // Reconcile link state with current connection state. If the station
        // is already connected (e.g., connect was called before the runner
        // was spawned), start with link Up instead of Down.
        let initial_link = {
            use crate::wifi::controller::{current_sta_state, is_ap_active, WifiInterface};
            let sta_up = current_sta_state() == StaConnectionState::Connected;
            let ap_up = is_ap_active();
            match self.device.interface() {
                WifiInterface::Sta if sta_up => ch::driver::LinkState::Up,
                WifiInterface::Ap if ap_up => ch::driver::LinkState::Up,
                _ => ch::driver::LinkState::Down,
            }
        };
        self.ch_runner.set_link_state(initial_link);
        record_net_runner_stage(b'L');

        // Ensure the event trampoline is registered so we get WiFi events.
        if let Err(e) = event::ensure_trampoline_registered() {
            log::error!("Failed to register WiFi event trampoline: {:?}", e);
        }
        record_net_runner_stage(b'T');

        // Subscribe to WiFi events for link state tracking.
        // If the subscriber limit is reached (4 max), link state changes will
        // not be tracked and must be managed externally via set_link_state().
        let mut event_sub = event::event_subscriber();
        if event_sub.is_none() {
            log::warn!(
                "WiFi event subscriber limit reached; link state changes will not be tracked"
            );
        }
        record_net_runner_stage(b'S');

        loop {
            record_net_runner_stage(b'E');
            // Use select to wait for either:
            // 1. RX data available from WiFi
            // 2. TX data pending from embassy-net
            // 3. A WiFi event (connect/disconnect)
            //
            // We use a short yield to check all sources, but the RX waker
            // will be triggered by the ISR callback when packets arrive.

            // Process any pending WiFi events for link state.
            if let Some(ref mut sub) = event_sub {
                while let Some(event) = sub.try_next_message_pure() {
                    match event.kind() {
                        WifiEventKind::StaConnected | WifiEventKind::ApStart => {
                            self.ch_runner.set_link_state(ch::driver::LinkState::Up);
                        }
                        WifiEventKind::StaDisconnected | WifiEventKind::ApStop => {
                            self.ch_runner.set_link_state(ch::driver::LinkState::Down);
                        }
                        _ => {}
                    }
                }
            }

            // Forward RX packets from WiFi → channel.
            record_net_runner_stage(b'R');
            self.poll_rx();

            // Forward TX packets from channel → WiFi.
            record_net_runner_stage(b'X');
            self.poll_tx();

            // Wait for the RX waker to fire (ISR-driven) or a brief timeout
            // as fallback for TX readiness and event processing.
            record_net_runner_stage(b'W');
            let rx_wait = core::future::poll_fn(|cx| {
                self.device.register_waker(cx.waker());
                if self.device.has_rx_data() {
                    core::task::Poll::Ready(())
                } else {
                    core::task::Poll::Pending
                }
            });

            // Use select: wake on RX data OR after a short timeout for TX/events.
            embassy_futures::select::select(
                rx_wait,
                embassy_time::Timer::after(embassy_time::Duration::from_millis(5)),
            )
            .await;
        }
    }

    /// Forward any available RX packets from the WiFi ring buffer into the
    /// channel.
    fn poll_rx(&mut self) {
        record_net_runner_stage(b'r');
        loop {
            if !self.device.has_rx_data() {
                break;
            }

            // Try to get a free RX buffer from the channel.
            record_net_runner_stage(b'B');
            let Some(buf) = self.ch_runner.try_rx_buf() else {
                break; // Channel full, try again later.
            };

            // Pop the already-copied frame from the WiFi ring directly into
            // the driver-channel buffer. The original WiFi ownership was
            // released in the RX callback after the ring copy succeeded.
            record_net_runner_stage(b'P');
            if let Some(len) = self.device.pop_frame(buf) {
                record_net_runner_stage(b'D');
                self.ch_runner.rx_done(len);
            } else {
                break;
            }
        }
    }

    /// Forward any pending TX packets from the channel to the WiFi driver.
    fn poll_tx(&mut self) {
        record_net_runner_stage(b't');
        loop {
            let interface = self.device.interface();
            let Some(buf) = self.ch_runner.try_tx_buf() else {
                break; // No pending TX.
            };

            let len = buf.len();
            if len == 0 {
                record_net_runner_stage(b'Q');
                self.ch_runner.tx_done();
                continue;
            }

            if !try_send_tx_buf(interface, buf) {
                break;
            }

            record_net_runner_stage(b'Q');
            self.ch_runner.tx_done();
        }
    }

    /// Set the link state (up/down). Call when WiFi connects/disconnects.
    pub fn set_link_state(&mut self, up: bool) {
        self.ch_runner.set_link_state(if up {
            ch::driver::LinkState::Up
        } else {
            ch::driver::LinkState::Down
        });
    }
}

fn try_send_tx_buf(interface: crate::wifi::controller::WifiInterface, buf: &mut [u8]) -> bool {
    record_net_runner_stage(b'O');
    let ret = unsafe {
        esp8266_wifi_sys::esp_wifi_internal_tx(
            interface.to_raw(),
            buf.as_mut_ptr() as *mut core::ffi::c_void,
            buf.len() as u16,
        )
    };
    crate::wifi::device::record_net_tx(interface, buf.as_ptr(), buf.len(), ret);

    if ret == 0 {
        return true;
    }

    // The zerocopy channel keeps the packet at the front until `tx_done()`.
    // On temporary SDK backpressure, return without `tx_done()` so the same
    // buffer is retried on the next poll instead of copying it aside.
    if ret == ERR_MEM || ret == esp8266_wifi_sys::ESP_ERR_NO_MEM {
        record_net_runner_stage(b'M');
        return false;
    }

    log::warn!("WiFi TX failed: {}", ret);
    true
}
