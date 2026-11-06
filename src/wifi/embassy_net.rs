//! Embassy network channels for either concrete Wi-Fi interface.
//!
//! The channel provides fixed RX/TX storage and retains a TX frame while the
//! SDK is busy. Its runner waits for RX space and TX work concurrently, so a
//! full RX channel cannot prevent outgoing traffic from making progress.
//!
//! Stopping Wi-Fi drains driver-owned RX queues. Packets already copied into
//! this channel or the network stack, and existing socket state, belong to the
//! application's lifecycle. Reset the upper stack when changing networks if
//! those packets or connections must be discarded.

use super::neighbor::EthernetNeighborHints;
use crate::wifi_core::{
    DriverError, Interface, LinkState, MAX_ETHERNET_FRAME_SIZE, MIN_IPV4_ETHERNET_FRAME_SIZE,
};
use core::{
    cell::Cell,
    future::poll_fn,
    task::{Context, Poll},
};
use embassy_futures::{select::select, yield_now};
use embassy_net_driver_channel as ch;
use embassy_time::{Duration, Timer};

pub const DEFAULT_MTU: usize = MAX_ETHERNET_FRAME_SIZE;
/// Keep defaults small for the ESP8266 DRAM budget; each slot stores a full frame.
pub const DEFAULT_RX_PACKET_BUFFERS: usize = 1;
pub const DEFAULT_TX_PACKET_BUFFERS: usize = 1;
const COOPERATIVE_PROGRESS_BUDGET: u8 = 8;

/// Packet storage and the per-frame lifecycle tags shared by the channel ends.
/// The channel remains caller-owned; construction performs no allocation.
pub struct NetDriverStateWithMtu<
    const MTU: usize,
    const N_RX: usize = DEFAULT_RX_PACKET_BUFFERS,
    const N_TX: usize = DEFAULT_TX_PACKET_BUFFERS,
> {
    channel: ch::State<MTU, N_RX, N_TX>,
    tx_generations: [Cell<u32>; N_TX],
    generation: Cell<u32>,
}

impl<const MTU: usize, const N_RX: usize, const N_TX: usize>
    NetDriverStateWithMtu<MTU, N_RX, N_TX>
{
    pub const fn new() -> Self {
        Self {
            channel: ch::State::new(),
            tx_generations: [const { Cell::new(0) }; N_TX],
            generation: Cell::new(0),
        }
    }
}

impl<const MTU: usize, const N_RX: usize, const N_TX: usize> Default
    for NetDriverStateWithMtu<MTU, N_RX, N_TX>
{
    fn default() -> Self {
        Self::new()
    }
}

pub type NetDriverState<
    const N_RX: usize = DEFAULT_RX_PACKET_BUFFERS,
    const N_TX: usize = DEFAULT_TX_PACKET_BUFFERS,
> = NetDriverStateWithMtu<DEFAULT_MTU, N_RX, N_TX>;
pub type NetDevice<'s> = NetDeviceWithMtu<'s, DEFAULT_MTU>;

/// Channel device whose transmit tokens preserve their issuing lifecycle.
pub struct NetDeviceWithMtu<'s, const MTU: usize> {
    inner: ch::Device<'s, MTU>,
    tx_generations: &'s [Cell<u32>],
    generation: &'s Cell<u32>,
    next_tx: usize,
}

impl<const MTU: usize> ch::driver::Driver for NetDeviceWithMtu<'_, MTU> {
    type RxToken<'a>
        = ch::RxToken<'a, MTU>
    where
        Self: 'a;
    type TxToken<'a>
        = TxToken<'a, MTU>
    where
        Self: 'a;

    fn receive(&mut self, cx: &mut Context) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let (rx, tx) = self.inner.receive(cx)?;
        Some((
            rx,
            TxToken {
                inner: tx,
                generations: self.tx_generations,
                next: &mut self.next_tx,
                generation: self.generation.get(),
            },
        ))
    }

    fn transmit(&mut self, cx: &mut Context) -> Option<Self::TxToken<'_>> {
        let inner = self.inner.transmit(cx)?;
        Some(TxToken {
            inner,
            generations: self.tx_generations,
            next: &mut self.next_tx,
            generation: self.generation.get(),
        })
    }

    fn capabilities(&self) -> ch::driver::Capabilities {
        self.inner.capabilities()
    }
    fn hardware_address(&self) -> ch::driver::HardwareAddress {
        self.inner.hardware_address()
    }
    fn link_state(&mut self, cx: &mut Context) -> ch::driver::LinkState {
        self.inner.link_state(cx)
    }
}

/// A channel reservation tagged when issued, before a control operation can
/// stop or restart Wi-Fi inside the application's packet-building closure.
pub struct TxToken<'a, const MTU: usize> {
    inner: ch::TxToken<'a, MTU>,
    generations: &'a [Cell<u32>],
    next: &'a mut usize,
    generation: u32,
}

impl<const MTU: usize> ch::driver::TxToken for TxToken<'_, MTU> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        self.inner.consume(len, |buffer| {
            let result = f(buffer);
            // Publish the matching tag before send_done can wake the runner.
            self.generations[*self.next].set(self.generation);
            *self.next = (*self.next + 1) % self.generations.len();
            result
        })
    }
}

pub type Runner<'s, 'd> = RunnerWithMtu<'s, 'd, DEFAULT_MTU>;

/// Creates a channel for either station or access-point frames.
/// Poll the returned runner alongside the application's network stack.
pub fn new<'s, 'd, const N_RX: usize, const N_TX: usize>(
    state: &'s mut NetDriverState<N_RX, N_TX>,
    interface: Interface<'d>,
) -> Result<(Runner<'s, 'd>, NetDevice<'s>), DriverError> {
    new_with_mtu(state, interface)
}

/// Creates a channel with application-selected frame capacity.
///
/// Both queue counts must be nonzero, and `MTU` must fit the interface and be at
/// least [`MIN_IPV4_ETHERNET_FRAME_SIZE`]. Storage is borrowed without allocation.
pub fn new_with_mtu<'s, 'd, const MTU: usize, const N_RX: usize, const N_TX: usize>(
    state: &'s mut NetDriverStateWithMtu<MTU, N_RX, N_TX>,
    interface: Interface<'d>,
) -> Result<(RunnerWithMtu<'s, 'd, MTU>, NetDeviceWithMtu<'s, MTU>), DriverError> {
    if N_RX == 0 || N_TX == 0 {
        return Err(DriverError::PacketOverflow);
    }
    if MTU < MIN_IPV4_ETHERNET_FRAME_SIZE {
        return Err(DriverError::InvalidFrameSize);
    }
    if MTU > interface.max_frame_size() {
        return Err(DriverError::PayloadTooLong);
    }
    let mac = interface.mac_address();
    let generation = interface.tx_generation();
    state.generation.set(generation);
    let (runner, device) = ch::new(
        &mut state.channel,
        ch::driver::HardwareAddress::Ethernet(mac),
    );
    let (state_runner, rx_runner, tx_runner) = runner.split();
    Ok((
        RunnerWithMtu {
            state_runner,
            rx_runner,
            tx_runner,
            interface,
            last_link: LinkState::Down,
            generation,
            tx_generations: &state.tx_generations,
            shared_generation: &state.generation,
            next_tx: 0,
            neighbor_hints: EthernetNeighborHints::new(mac),
        },
        NetDeviceWithMtu {
            inner: device,
            tx_generations: &state.tx_generations,
            generation: &state.generation,
            next_tx: 0,
        },
    ))
}

pub struct RunnerWithMtu<'s, 'd, const MTU: usize> {
    state_runner: ch::StateRunner<'s>,
    rx_runner: ch::RxRunner<'s, MTU>,
    tx_runner: ch::TxRunner<'s, MTU>,
    interface: Interface<'d>,
    last_link: LinkState,
    generation: u32,
    tx_generations: &'s [Cell<u32>],
    shared_generation: &'s Cell<u32>,
    next_tx: usize,
    neighbor_hints: EthernetNeighborHints,
}

impl<const MTU: usize> RunnerWithMtu<'_, '_, MTU> {
    pub async fn run(&mut self) -> ! {
        let mut budget = COOPERATIVE_PROGRESS_BUDGET;
        loop {
            self.synchronize_generation();
            let current_link = self.interface.link_state();
            if current_link != self.last_link {
                self.state_runner
                    .set_link_state(map_link_state(current_link));
                self.last_link = current_link;
            }
            let rx_progress = self.poll_rx();
            let (tx_progress, tx_blocked) = self.poll_tx();
            if tx_blocked {
                // Native completion precedes descriptor recycling, and other
                // radio traffic can consume capacity without notifying us.
                // Timed retry guarantees progress without a completion wakeup.
                Timer::after(Duration::from_millis(1)).await;
            } else if rx_progress || tx_progress {
                budget -= 1;
                if budget == 0 {
                    budget = COOPERATIVE_PROGRESS_BUDGET;
                    yield_now().await;
                }
            } else {
                budget = COOPERATIVE_PROGRESS_BUDGET;
                self.wait_for_work().await;
            }
        }
    }

    fn synchronize_generation(&mut self) {
        let generation = self.interface.tx_generation();
        if generation != self.generation {
            self.shared_generation.set(generation);
            self.neighbor_hints.clear();
            self.generation = generation;
        }
    }

    fn poll_rx(&mut self) -> bool {
        let mut progress = false;
        while self.neighbor_hints.has_pending() || self.interface.has_rx_data() {
            let Some(buffer) = self.rx_runner.try_rx_buf() else {
                break;
            };
            match self
                .neighbor_hints
                .receive_into(&mut self.interface, buffer)
            {
                Ok(Some(len)) => {
                    self.rx_runner.rx_done(len);
                    progress = true;
                }
                Ok(None) => break,
                Err(_) => {
                    progress = true;
                }
            }
        }
        progress
    }

    fn poll_tx(&mut self) -> (bool, bool) {
        let mut progress = false;
        while let Some(buffer) = self.tx_runner.try_tx_buf() {
            let result = if buffer.is_empty()
                || self.tx_generations[self.next_tx].get() != self.generation
            {
                Ok(())
            } else {
                self.interface
                    .transmit_in_generation(buffer, self.generation)
            };
            match result {
                Err(DriverError::WouldBlock) => return (progress, true),
                Ok(()) | Err(_) => {
                    self.tx_runner.tx_done();
                    self.next_tx = (self.next_tx + 1) % self.tx_generations.len();
                    progress = true;
                }
            }
        }
        (progress, false)
    }

    async fn wait_for_work(&mut self) {
        let pending_rx = self.neighbor_hints.has_pending() || self.interface.has_rx_data();
        let interface = &self.interface;
        let last_link = self.last_link;
        let generation = self.generation;
        let interface_ready = poll_fn(|cx| {
            interface.register_link_waker(cx.waker());
            interface.register_rx_waker(cx.waker());
            if interface.link_state() != last_link
                || interface.tx_generation() != generation
                || (!pending_rx && interface.has_rx_data())
            {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        });
        let tx_ready = async {
            let _ = self.tx_runner.tx_buf().await;
        };
        if pending_rx {
            let rx_ready = async {
                let _ = self.rx_runner.rx_buf().await;
            };
            select(interface_ready, select(rx_ready, tx_ready)).await;
        } else {
            select(interface_ready, tx_ready).await;
        }
    }
}

fn map_link_state(state: LinkState) -> ch::driver::LinkState {
    match state {
        LinkState::Up => ch::driver::LinkState::Up,
        LinkState::Down => ch::driver::LinkState::Down,
    }
}

#[cfg(all(test, not(target_arch = "xtensa")))]
mod tests {
    extern crate std;
    use self::std::{boxed::Box, sync::Arc, task::Wake};
    use super::*;
    use crate::wifi_core::{
        allocator::test_allocator,
        target::{InterfaceId, RawRxRecord, Resources, Sdk, TxScript},
        Controller, QueueConfig, WifiEvent,
    };
    use ch::driver::{Driver as _, RxToken as _, TxToken as _};
    use core::{future::Future, task::Waker};

    struct Noop;
    impl Wake for Noop {
        fn wake(self: Arc<Self>) {}
    }

    #[test]
    fn restart_discards_issued_and_buffered_frames_before_reusing_channel_slots() {
        let resources = Resources::try_new(&QueueConfig::default(), test_allocator()).unwrap();
        let mut sdk = Sdk::new(&resources);
        let mut controller = Controller {
            sdk: &mut sdk,
            resources: &resources,
        };
        controller.enable_station().unwrap();
        controller.start_blocking(0).unwrap();
        resources.publish_event(WifiEvent::StaConnected).unwrap();
        let interface = Interface {
            resources: &resources,
            id: InterfaceId::Station,
            mac: [2, 0, 0, 0, 0, 1],
        };
        let mut state = NetDriverStateWithMtu::<82, 1, 2>::new();
        let (mut runner, mut device) = new_with_mtu(&mut state, interface).unwrap();
        let tx = TxScript::new([Err(DriverError::WouldBlock), Ok(()), Ok(())]);
        let waker = Waker::from(Arc::new(Noop));
        let mut cx = Context::from_waker(&waker);

        let issued = device.transmit(&mut cx).unwrap();
        controller.stop().unwrap();
        controller.start_blocking(0).unwrap();
        resources.publish_event(WifiEvent::StaConnected).unwrap();
        runner.synchronize_generation();
        issued.consume(14, |buffer| buffer.fill(0x11));
        assert_eq!(runner.poll_tx(), (true, false));
        assert!(tx.frames().is_empty());

        device.transmit(&mut cx).unwrap().consume(14, |buffer| {
            buffer.fill(0x22);
            controller.stop().unwrap();
            controller.start_blocking(0).unwrap();
            resources.publish_event(WifiEvent::StaConnected).unwrap();
            runner.synchronize_generation();
        });
        assert_eq!(runner.poll_tx(), (true, false));
        assert!(tx.frames().is_empty());

        device
            .transmit(&mut cx)
            .unwrap()
            .consume(14, |buffer| buffer.fill(0x33));
        assert_eq!(runner.poll_tx(), (false, true));
        device
            .transmit(&mut cx)
            .unwrap()
            .consume(14, |buffer| buffer.fill(0x44));
        controller.stop().unwrap();
        controller.start_blocking(0).unwrap();
        resources.publish_event(WifiEvent::StaConnected).unwrap();
        runner.synchronize_generation();
        assert_eq!(runner.poll_tx(), (true, false));

        for byte in [0x55, 0x66] {
            device
                .transmit(&mut cx)
                .unwrap()
                .consume(14, |buffer| buffer.fill(byte));
        }
        assert_eq!(runner.poll_tx(), (true, false));
        let frames = tx.frames();
        assert_eq!(frames.len(), 3);
        for ((id, frame), byte) in frames.iter().zip([0x33, 0x55, 0x66]) {
            assert_eq!(*id, InterfaceId::Station);
            assert_eq!(frame.as_slice(), [byte; 14]);
        }
    }

    #[test]
    fn full_rx_channel_waits_for_tx_and_preserves_retries_on_both_interfaces() {
        static RECEIVED: [u8; 14] = [0x77; 14];
        unsafe fn release_static(_: *mut core::ffi::c_void, _: *mut core::ffi::c_void) {}
        for id in [InterfaceId::Station, InterfaceId::AccessPoint] {
            let resources = Resources::try_new(
                &QueueConfig {
                    station_rx: 2,
                    access_point_rx: 2,
                    ..QueueConfig::default()
                },
                test_allocator(),
            )
            .unwrap();
            resources
                .publish_event(match id {
                    InterfaceId::Station => WifiEvent::StaConnected,
                    InterfaceId::AccessPoint => WifiEvent::ApStarted,
                })
                .unwrap();
            let interface = Interface {
                resources: &resources,
                id,
                mac: [2, 0, 0, 0, 0, 1],
            };
            let mut state = NetDriverStateWithMtu::<82, 1, 2>::new();
            let (mut runner, mut device) = new_with_mtu(&mut state, interface).unwrap();
            // Arrange an observed link so the wait below depends only on packet work.
            runner.last_link = LinkState::Up;
            for _ in 0..2 {
                // SAFETY: the immutable static frame remains live after release.
                let record = unsafe {
                    RawRxRecord::from_raw(
                        RECEIVED.as_ptr().cast_mut().cast(),
                        RECEIVED.len() as u16,
                        core::ptr::null_mut(),
                        release_static,
                    )
                };
                resources.enqueue_rx(id, record).unwrap();
            }
            assert!(runner.poll_rx());
            assert!(runner.interface.has_rx_data());
            assert!(!runner.poll_rx());

            let tx = TxScript::new([Err(DriverError::WouldBlock), Ok(()), Ok(())]);
            let waker = Waker::from(Arc::new(Noop));
            let mut cx = Context::from_waker(&waker);
            {
                let mut wait = Box::pin(runner.wait_for_work());
                assert_eq!(wait.as_mut().poll(&mut cx), Poll::Pending);
                device
                    .transmit(&mut cx)
                    .unwrap()
                    .consume(14, |buffer| buffer.fill(0x11));
                assert_eq!(wait.as_mut().poll(&mut cx), Poll::Ready(()));
            }
            device
                .transmit(&mut cx)
                .unwrap()
                .consume(14, |buffer| buffer.fill(0x22));
            assert_eq!(runner.poll_tx(), (false, true));
            assert!(device.transmit(&mut cx).is_none());
            assert_eq!(runner.poll_tx(), (true, false));
            assert!(!runner.poll_rx());

            let frames = tx.frames();
            assert_eq!(frames.len(), 3);
            for ((interface, frame), byte) in frames.iter().zip([0x11, 0x11, 0x22]) {
                assert_eq!(*interface, id);
                assert_eq!(frame.as_slice(), [byte; 14]);
            }
            {
                let (rx, _tx) = device.receive(&mut cx).unwrap();
                rx.consume(|frame| assert_eq!(frame, RECEIVED));
            }
            assert!(runner.poll_rx());
            assert!(!runner.interface.has_rx_data());
        }
    }
}
