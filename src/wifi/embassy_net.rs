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

pub fn new<'s, 'd, const N_RX: usize, const N_TX: usize>(
    state: &'s mut NetDriverState<N_RX, N_TX>,
    interface: Interface<'d>,
) -> Result<(Runner<'s, 'd>, NetDevice<'s>), DriverError> {
    new_with_mtu(state, interface)
}

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
            neighbor_hints: EthernetNeighborHints::new(Some(mac)),
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

#[cfg(test)]
mod tests {
    extern crate std;
    use self::std::{sync::Arc, task::Wake};
    use super::*;
    use ch::driver::{Driver as _, TxToken as _};
    use core::task::Waker;

    struct Noop;
    impl Wake for Noop {
        fn wake(self: Arc<Self>) {}
    }

    #[test]
    fn channel_tokens_keep_issuing_generation_through_restart_and_wraparound() {
        let mut state = NetDriverStateWithMtu::<64, 1, 2>::new();
        state.generation.set(4);
        let (mut runner, inner) = ch::new(
            &mut state.channel,
            ch::driver::HardwareAddress::Ethernet([2, 0, 0, 0, 0, 1]),
        );
        let mut device = NetDeviceWithMtu {
            inner,
            tx_generations: &state.tx_generations,
            generation: &state.generation,
            next_tx: 0,
        };
        let waker = Waker::from(Arc::new(Noop));
        let mut cx = Context::from_waker(&waker);

        let old = device.transmit(&mut cx).unwrap();
        state.generation.set(5);
        old.consume(3, |buffer| buffer.copy_from_slice(b"old"));
        assert_eq!(runner.try_tx_buf().unwrap(), b"old");
        assert_eq!(state.tx_generations[0].get(), 4);
        runner.tx_done();

        device.transmit(&mut cx).unwrap().consume(3, |buffer| {
            // A controller stop/start can happen inside this user closure.
            state.generation.set(6);
            buffer.copy_from_slice(b"mid");
        });
        assert_eq!(runner.try_tx_buf().unwrap(), b"mid");
        assert_eq!(state.tx_generations[1].get(), 5);
        runner.tx_done();

        device
            .transmit(&mut cx)
            .unwrap()
            .consume(3, |buffer| buffer.copy_from_slice(b"new"));
        assert_eq!(runner.try_tx_buf().unwrap(), b"new");
        assert_eq!(state.tx_generations[0].get(), 6);
    }
}
