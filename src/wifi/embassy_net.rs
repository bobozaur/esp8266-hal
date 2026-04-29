//! Embassy-net driver integration over [`wifi_core`](crate::wifi_core).

use core::task::Poll;

use embassy_futures::select::Either;
use embassy_futures::yield_now;
use embassy_net_driver_channel as ch;
use embassy_time::{Duration, Timer};

use crate::wifi_core::{
    ApLink, DriverError, HasAp, HasSta, LinkState, StaLink, WifiBackend, WifiDataLink,
};
#[cfg(feature = "wifi-debug-trace")]
use crate::wifi_core::{RunnerDebug, RunnerDebugSnapshot};

/// Default maximum Ethernet frame size (MTU 1500 + 14-byte header).
pub const DEFAULT_MTU: usize = 1514;

/// Default number of RX packet buffers reserved for the embassy-net bridge.
///
/// The ESP8266 only has ~80 KiB of DRAM and the channel state stores full-MTU
/// packets inline, so the default is intentionally minimal. Callers that need
/// more burst tolerance can opt into a larger `NetDriverState<N_RX, N_TX>`.
pub const DEFAULT_RX_PACKET_BUFFERS: usize = 1;

/// Default number of TX packet buffers reserved for the embassy-net bridge.
pub const DEFAULT_TX_PACKET_BUFFERS: usize = 1;
const COOPERATIVE_PROGRESS_BUDGET: u8 = 8;

#[cfg(feature = "wifi-debug-trace")]
#[no_mangle]
pub static mut g_net_runner_stage: u32 = 0;
#[cfg(feature = "wifi-debug-trace")]
static RUNNER_DEBUG: RunnerDebug = RunnerDebug::new();

#[inline(always)]
fn record_net_runner_stage(stage: u8) {
    #[cfg(feature = "wifi-debug-trace")]
    unsafe {
        g_net_runner_stage = stage as u32
    };
    #[cfg(not(feature = "wifi-debug-trace"))]
    let _ = stage;
}

#[cfg(feature = "wifi-debug-trace")]
pub fn runner_debug_snapshot() -> RunnerDebugSnapshot {
    RUNNER_DEBUG.snapshot()
}

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
    const SUBS: usize,
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
        SUBS,
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
            SUBS,
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
    const SUBS: usize,
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
        SUBS,
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
            SUBS,
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
    const SUBS: usize,
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
        SUBS,
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
            SUBS,
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
    const SUBS: usize,
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
        SUBS,
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
            SUBS,
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

fn new_link<'s, L, const MTU: usize, const N_RX: usize, const N_TX: usize>(
    state: &'s mut NetDriverStateWithMtu<MTU, N_RX, N_TX>,
    link: L,
) -> Result<(LinkRunner<'s, L, MTU>, NetDeviceWithMtu<'s, MTU>), DriverError>
where
    L: WifiDataLink,
{
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
    const SUBS: usize,
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
        SUBS,
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
    const SUBS: usize,
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
        SUBS,
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
    const SUBS: usize,
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
        SUBS,
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
    const SUBS: usize,
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
        SUBS,
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
        let _ = self.link.refresh_rx();
        let mut progress_budget = COOPERATIVE_PROGRESS_BUDGET;

        loop {
            #[cfg(feature = "wifi-debug-trace")]
            RUNNER_DEBUG.note_loop();
            record_net_runner_stage(b'E');
            self.apply_link_state();

            record_net_runner_stage(b'R');
            let rx = self.poll_rx();

            record_net_runner_stage(b'X');
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
            record_net_runner_stage(if rx.pending_rx { b'B' } else { b'W' });
            self.wait_for_work(rx.pending_rx).await;
        }
    }

    fn apply_link_state(&mut self) {
        let current = self.link.link_state();
        if current != self.last_link {
            #[cfg(feature = "wifi-debug-trace")]
            RUNNER_DEBUG.note_link_update();
            self.state_runner.set_link_state(map_link_state(current));
            self.last_link = current;
        }
    }

    fn poll_rx(&mut self) -> RxPollResult {
        let mut made_progress = false;

        loop {
            if !self.link.has_rx_data() {
                break;
            }

            let Some(buf) = self.rx_runner.try_rx_buf() else {
                break;
            };

            if let Some(len) = self.link.receive(buf) {
                made_progress = true;
                #[cfg(feature = "wifi-debug-trace")]
                RUNNER_DEBUG.note_rx_frame();
                self.rx_runner.rx_done(len);
            } else {
                break;
            }
        }

        RxPollResult {
            made_progress,
            pending_rx: self.link.has_rx_data(),
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
                    #[cfg(feature = "wifi-debug-trace")]
                    RUNNER_DEBUG.note_tx_frame();
                    self.tx_runner.tx_done()
                }
                Err(DriverError::WouldBlock) => {
                    #[cfg(feature = "wifi-debug-trace")]
                    RUNNER_DEBUG.note_tx_would_block();
                    blocked = true;
                    break;
                }
                Err(_) => {
                    made_progress = true;
                    #[cfg(feature = "wifi-debug-trace")]
                    RUNNER_DEBUG.note_tx_error();
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
        let rx_runner = &mut self.rx_runner;
        let tx_runner = &mut self.tx_runner;

        if pending_rx {
            match embassy_futures::select::select(
                async {
                    let buf = rx_runner.rx_buf().await;
                    link.receive(buf)
                },
                tx_runner.tx_buf(),
            )
            .await
            {
                Either::First(Some(len)) => {
                    #[cfg(feature = "wifi-debug-trace")]
                    RUNNER_DEBUG.note_ready_wake();
                    #[cfg(feature = "wifi-debug-trace")]
                    RUNNER_DEBUG.note_rx_frame();
                    rx_runner.rx_done(len);
                }
                Either::First(None) => {
                    #[cfg(feature = "wifi-debug-trace")]
                    RUNNER_DEBUG.note_ready_wake();
                    debug_assert!(
                        false,
                        "wifi_core rx queue emptied while awaiting embassy rx buffer"
                    );
                    rx_runner.rx_done(0);
                }
                Either::Second(buf) => {
                    #[cfg(feature = "wifi-debug-trace")]
                    RUNNER_DEBUG.note_ready_wake();

                    if buf.is_empty() {
                        tx_runner.tx_done();
                        return;
                    }

                    match link.transmit(buf) {
                        Ok(()) => {
                            #[cfg(feature = "wifi-debug-trace")]
                            RUNNER_DEBUG.note_tx_frame();
                            tx_runner.tx_done();
                        }
                        Err(DriverError::WouldBlock) => {
                            #[cfg(feature = "wifi-debug-trace")]
                            RUNNER_DEBUG.note_tx_would_block();
                        }
                        Err(_) => {
                            #[cfg(feature = "wifi-debug-trace")]
                            RUNNER_DEBUG.note_tx_error();
                            tx_runner.tx_done();
                        }
                    }
                }
            }
            return;
        }

        let last_link = self.last_link;
        match embassy_futures::select::select(
            core::future::poll_fn(|cx| {
                link.register_rx_waker(cx.waker());
                link.register_link_waker(cx.waker());

                if link.has_rx_data() || link.link_state() != last_link {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            }),
            tx_runner.tx_buf(),
        )
        .await
        {
            Either::First(()) => {
                #[cfg(feature = "wifi-debug-trace")]
                RUNNER_DEBUG.note_ready_wake();
            }
            Either::Second(buf) => {
                #[cfg(feature = "wifi-debug-trace")]
                RUNNER_DEBUG.note_ready_wake();

                if buf.is_empty() {
                    tx_runner.tx_done();
                    return;
                }

                match link.transmit(buf) {
                    Ok(()) => {
                        #[cfg(feature = "wifi-debug-trace")]
                        RUNNER_DEBUG.note_tx_frame();
                        tx_runner.tx_done();
                    }
                    Err(DriverError::WouldBlock) => {
                        #[cfg(feature = "wifi-debug-trace")]
                        RUNNER_DEBUG.note_tx_would_block();
                    }
                    Err(_) => {
                        #[cfg(feature = "wifi-debug-trace")]
                        RUNNER_DEBUG.note_tx_error();
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
