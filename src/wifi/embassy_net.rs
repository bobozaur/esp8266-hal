//! Embassy-net driver integration over [`wifi_core`](crate::wifi_core).

use core::task::Poll;

use embassy_futures::select::Either;
use embassy_futures::yield_now;
use embassy_net_driver_channel as ch;
use embassy_time::{Duration, Timer};

use crate::wifi_core::{
    ApLink, DriverError, EthernetNeighborHints, HasAp, HasSta, LinkState, StaLink, WifiBackend,
    WifiDataLink, MAX_ETHERNET_FRAME_SIZE, MIN_IPV4_ETHERNET_FRAME_SIZE,
};

/// Default maximum Ethernet frame size (MTU 1500 + 14-byte header).
pub const DEFAULT_MTU: usize = MAX_ETHERNET_FRAME_SIZE;

/// Default number of RX packet buffers reserved for the embassy-net bridge.
///
/// The ESP8266 only has ~80 KiB of DRAM and the channel state stores full-MTU
/// packets inline, so the default is intentionally minimal. Callers that need
/// more burst tolerance can opt into a larger `NetDriverState<N_RX, N_TX>`.
pub const DEFAULT_RX_PACKET_BUFFERS: usize = 1;

/// Default number of TX packet buffers reserved for the embassy-net bridge.
pub const DEFAULT_TX_PACKET_BUFFERS: usize = 1;
const COOPERATIVE_PROGRESS_BUDGET: u8 = 8;

pub type NetDriverState<
    const N_RX: usize = DEFAULT_RX_PACKET_BUFFERS,
    const N_TX: usize = DEFAULT_TX_PACKET_BUFFERS,
> = ch::State<DEFAULT_MTU, N_RX, N_TX>;
pub type NetDriverStateWithMtu<
    const MTU: usize,
    const N_RX: usize = DEFAULT_RX_PACKET_BUFFERS,
    const N_TX: usize = DEFAULT_TX_PACKET_BUFFERS,
> = ch::State<MTU, N_RX, N_TX>;
pub type NetDevice<'s> = ch::Device<'s, DEFAULT_MTU>;
pub type NetDeviceWithMtu<'s, const MTU: usize> = ch::Device<'s, MTU>;

#[cfg(target_arch = "xtensa")]
const _: () = assert!(
    core::mem::size_of::<NetDriverState>() <= 0x0c80,
    "default embassy-net state exceeds ESP8266 DRAM budget"
);

pub fn new<
    's,
    'd,
    M,
    B,
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize,
    const ESP_NOW_RX: usize,
    const PROMISCUOUS_RX: usize,
    const AP_RX_SLOTS: usize,
    const AP_RX_BYTES: usize,
    const N_RX: usize,
    const N_TX: usize,
>(
    state: &'s mut NetDriverState<N_RX, N_TX>,
    link: StaLink<
        'd,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
) -> Result<
    (
        Runner<
            's,
            'd,
            M,
            B,
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        NetDevice<'s>,
    ),
    DriverError,
>
where
    M: HasSta,
    B: WifiBackend,
{
    new_with_mtu(state, link)
}

pub fn new_with_mtu<
    's,
    'd,
    M,
    B,
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize,
    const ESP_NOW_RX: usize,
    const PROMISCUOUS_RX: usize,
    const AP_RX_SLOTS: usize,
    const AP_RX_BYTES: usize,
    const MTU: usize,
    const N_RX: usize,
    const N_TX: usize,
>(
    state: &'s mut NetDriverStateWithMtu<MTU, N_RX, N_TX>,
    link: StaLink<
        'd,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
) -> Result<
    (
        RunnerWithMtu<
            's,
            'd,
            M,
            B,
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
            MTU,
        >,
        NetDeviceWithMtu<'s, MTU>,
    ),
    DriverError,
>
where
    M: HasSta,
    B: WifiBackend,
{
    new_link(state, link)
}

pub fn new_ap<
    's,
    'd,
    M,
    B,
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize,
    const ESP_NOW_RX: usize,
    const PROMISCUOUS_RX: usize,
    const AP_RX_SLOTS: usize,
    const AP_RX_BYTES: usize,
    const N_RX: usize,
    const N_TX: usize,
>(
    state: &'s mut NetDriverState<N_RX, N_TX>,
    link: ApLink<
        'd,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
) -> Result<
    (
        ApRunner<
            's,
            'd,
            M,
            B,
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
        NetDevice<'s>,
    ),
    DriverError,
>
where
    M: HasAp,
    B: WifiBackend,
{
    new_ap_with_mtu(state, link)
}

pub fn new_ap_with_mtu<
    's,
    'd,
    M,
    B,
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize,
    const ESP_NOW_RX: usize,
    const PROMISCUOUS_RX: usize,
    const AP_RX_SLOTS: usize,
    const AP_RX_BYTES: usize,
    const MTU: usize,
    const N_RX: usize,
    const N_TX: usize,
>(
    state: &'s mut NetDriverStateWithMtu<MTU, N_RX, N_TX>,
    link: ApLink<
        'd,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
) -> Result<
    (
        ApRunnerWithMtu<
            's,
            'd,
            M,
            B,
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
            MTU,
        >,
        NetDeviceWithMtu<'s, MTU>,
    ),
    DriverError,
>
where
    M: HasAp,
    B: WifiBackend,
{
    new_link(state, link)
}

/// Builds an embassy-net channel around any Wi-Fi data endpoint.
///
/// This is also the constructor for both endpoints returned by
/// [`MixedLink::split`](crate::wifi_core::MixedLink::split).
pub fn new_link<L, const MTU: usize, const N_RX: usize, const N_TX: usize>(
    state: &mut NetDriverStateWithMtu<MTU, N_RX, N_TX>,
    mut link: L,
) -> Result<(LinkRunner<'_, L, MTU>, NetDeviceWithMtu<'_, MTU>), DriverError>
where
    L: WifiDataLink,
{
    if N_RX == 0 || N_TX == 0 {
        return Err(DriverError::PacketOverflow);
    }
    if MTU < MIN_IPV4_ETHERNET_FRAME_SIZE {
        return Err(DriverError::InvalidFrameSize);
    }
    if MTU > link.max_frame_size() {
        return Err(DriverError::PayloadTooLong);
    }
    link.refresh_rx()?;
    let mac = link.mac_address()?;
    let (ch_runner, net_device) = ch::new(state, ch::driver::HardwareAddress::Ethernet(mac));
    let (state_runner, rx_runner, tx_runner) = ch_runner.split();

    Ok((
        LinkRunner {
            state_runner,
            rx_runner,
            tx_runner,
            link,
            last_link: LinkState::Down,
            neighbor_hints: EthernetNeighborHints::new(Some(mac)),
        },
        net_device,
    ))
}

pub type Runner<
    's,
    'd,
    M,
    B,
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> = LinkRunner<
    's,
    StaLink<
        'd,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
    DEFAULT_MTU,
>;

pub type RunnerWithMtu<
    's,
    'd,
    M,
    B,
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
    const MTU: usize = DEFAULT_MTU,
> = LinkRunner<
    's,
    StaLink<
        'd,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
    MTU,
>;

pub type ApRunner<
    's,
    'd,
    M,
    B,
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> = LinkRunner<
    's,
    ApLink<
        'd,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
    DEFAULT_MTU,
>;

pub type ApRunnerWithMtu<
    's,
    'd,
    M,
    B,
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
    const MTU: usize = DEFAULT_MTU,
> = LinkRunner<
    's,
    ApLink<
        'd,
        M,
        B,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >,
    MTU,
>;

pub struct LinkRunner<'s, L, const MTU: usize = DEFAULT_MTU> {
    state_runner: ch::StateRunner<'s>,
    rx_runner: ch::RxRunner<'s, MTU>,
    tx_runner: ch::TxRunner<'s, MTU>,
    link: L,
    last_link: LinkState,
    neighbor_hints: EthernetNeighborHints,
}

struct RxPollResult {
    made_progress: bool,
    pending_rx: bool,
}

struct TxPollResult {
    made_progress: bool,
    blocked: bool,
}

impl<'s, L, const MTU: usize> LinkRunner<'s, L, MTU>
where
    L: WifiDataLink,
{
    pub async fn run(&mut self) -> ! {
        let initial = self.link.link_state();
        self.state_runner.set_link_state(map_link_state(initial));
        self.last_link = initial;
        let mut progress_budget = COOPERATIVE_PROGRESS_BUDGET;

        loop {
            self.apply_link_state();
            let rx = self.poll_rx();
            let tx = self.poll_tx();

            if tx.blocked {
                Timer::after(Duration::from_millis(1)).await;
                continue;
            }

            if rx.made_progress || tx.made_progress {
                progress_budget = progress_budget.saturating_sub(1);
                if progress_budget == 0 {
                    progress_budget = COOPERATIVE_PROGRESS_BUDGET;
                    yield_now().await;
                }
                continue;
            }

            progress_budget = COOPERATIVE_PROGRESS_BUDGET;
            self.wait_for_work(rx.pending_rx).await;
        }
    }

    fn apply_link_state(&mut self) {
        let current = self.link.link_state();
        if current != self.last_link {
            self.state_runner.set_link_state(map_link_state(current));
            self.last_link = current;
        }
    }

    fn poll_rx(&mut self) -> RxPollResult {
        let mut made_progress = false;

        loop {
            if !self.neighbor_hints.has_pending() && !self.link.has_rx_data() {
                break;
            }

            let Some(buf) = self.rx_runner.try_rx_buf() else {
                break;
            };

            match self.neighbor_hints.receive_into(&mut self.link, buf) {
                Ok(Some(len)) => {
                    made_progress = true;
                    self.rx_runner.rx_done(len);
                }
                Ok(None) => {
                    self.rx_runner.rx_done(0);
                    break;
                }
                Err(_) => {
                    made_progress = true;
                    self.rx_runner.rx_done(0);
                }
            }
        }

        RxPollResult {
            made_progress,
            pending_rx: self.neighbor_hints.has_pending() || self.link.has_rx_data(),
        }
    }

    fn poll_tx(&mut self) -> TxPollResult {
        let mut made_progress = false;
        let mut blocked = false;

        loop {
            let Some(buf) = self.tx_runner.try_tx_buf() else {
                break;
            };

            if buf.is_empty() {
                self.tx_runner.tx_done();
                made_progress = true;
                continue;
            }

            match self.link.transmit(buf) {
                Ok(()) => {
                    made_progress = true;
                    self.tx_runner.tx_done()
                }
                Err(DriverError::WouldBlock) => {
                    blocked = true;
                    break;
                }
                Err(_) => {
                    made_progress = true;
                    self.tx_runner.tx_done()
                }
            }
        }

        TxPollResult {
            made_progress,
            blocked,
        }
    }

    async fn wait_for_work(&mut self, pending_rx: bool) {
        let link = &mut self.link;
        let neighbor_hints = &mut self.neighbor_hints;
        let rx_runner = &mut self.rx_runner;
        let tx_runner = &mut self.tx_runner;

        if pending_rx {
            let receive = async {
                let buf = rx_runner.rx_buf().await;
                neighbor_hints.receive_into(link, buf)
            };
            let work = embassy_futures::select::select(receive, tx_runner.tx_buf()).await;
            match work {
                Either::First(result) => {
                    let len = match result {
                        Ok(Some(len)) => len,
                        Ok(None) | Err(_) => 0,
                    };
                    rx_runner.rx_done(len);
                }
                Either::Second(buf) => {
                    if buf.is_empty() {
                        tx_runner.tx_done();
                        return;
                    }

                    match link.transmit(buf) {
                        Ok(()) => {
                            tx_runner.tx_done();
                        }
                        Err(DriverError::WouldBlock) => {}
                        Err(_) => {
                            tx_runner.tx_done();
                        }
                    }
                }
            }
            return;
        }

        let last_link = self.last_link;
        let link_work = core::future::poll_fn(|cx| {
            link.register_rx_waker(cx.waker());
            link.register_link_waker(cx.waker());

            if link.has_rx_data() || link.link_state() != last_link {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        });
        let work = embassy_futures::select::select(link_work, tx_runner.tx_buf()).await;
        match work {
            Either::First(()) => {}
            Either::Second(buf) => {
                if buf.is_empty() {
                    tx_runner.tx_done();
                    return;
                }

                match link.transmit(buf) {
                    Ok(()) => {
                        tx_runner.tx_done();
                    }
                    Err(DriverError::WouldBlock) => {}
                    Err(_) => {
                        tx_runner.tx_done();
                    }
                }
            }
        }
    }
}

fn map_link_state(state: LinkState) -> ch::driver::LinkState {
    match state {
        LinkState::Up => ch::driver::LinkState::Up,
        LinkState::Down => ch::driver::LinkState::Down,
    }
}
