#[cfg(any(test, feature = "wifi-debug-trace"))]
use portable_atomic::{AtomicU32, Ordering};

#[cfg(feature = "wifi-debug-trace")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RxCallbackDebugSnapshot {
    pub calls: u32,
    pub enqueued: u32,
    pub rejected: u32,
    pub null_resources: u32,
    pub last_len: u32,
    pub attach_calls: u32,
    pub refresh_calls: u32,
    pub detach_calls: u32,
    pub register_calls: u32,
    pub unregister_calls: u32,
    pub last_register_ret: u32,
    pub last_unregister_ret: u32,
    pub sta_slot: u32,
    pub ap_slot: u32,
    pub expected_sta_slot: u32,
    pub ap_calls: u32,
    pub ap_enqueued: u32,
    pub ap_rejected: u32,
    pub ap_null_resources: u32,
    pub ap_last_len: u32,
    pub ap_attach_calls: u32,
    pub ap_refresh_calls: u32,
    pub ap_detach_calls: u32,
    pub ap_register_calls: u32,
    pub ap_unregister_calls: u32,
    pub ap_last_register_ret: u32,
    pub ap_last_unregister_ret: u32,
    pub expected_ap_slot: u32,
}

#[cfg(any(test, feature = "wifi-debug-trace"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunnerDebugSnapshot {
    pub loops: u32,
    pub ready_wakes: u32,
    pub timer_wakes: u32,
    pub link_updates: u32,
    pub rx_frames: u32,
    pub tx_frames: u32,
    pub tx_would_block: u32,
    pub tx_errors: u32,
}

#[cfg(feature = "wifi-debug-trace")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TxDebugSnapshot {
    pub calls: u32,
    pub ok: u32,
    pub would_block: u32,
    pub no_mem: u32,
    pub rejected: u32,
    pub last_ret: i32,
    pub last_len: u32,
    pub last_alloc_size: u32,
    pub last_free_heap: u32,
}

#[cfg(feature = "wifi-debug-trace")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeliverDataDebugSnapshot {
    pub calls: u32,
    pub if0_calls: u32,
    pub if1_calls: u32,
    pub other_if_calls: u32,
    pub last_if: u32,
    pub last_len: u32,
    pub last_sta_slot: u32,
    pub last_ap_slot: u32,
    pub cb_hit: u32,
    pub cb_miss: u32,
    pub last_cb_delta: u32,
    pub heap_checks: u32,
    pub last_free_heap: u32,
    pub alloc_attempts: u32,
    pub alloc_failures: u32,
    pub last_alloc_size: u32,
}

#[cfg(any(test, feature = "wifi-debug-trace"))]
pub(crate) struct RunnerDebug {
    loops: AtomicU32,
    ready_wakes: AtomicU32,
    timer_wakes: AtomicU32,
    link_updates: AtomicU32,
    rx_frames: AtomicU32,
    tx_frames: AtomicU32,
    tx_would_block: AtomicU32,
    tx_errors: AtomicU32,
}

#[cfg(any(test, feature = "wifi-debug-trace"))]
impl RunnerDebug {
    pub const fn new() -> Self {
        Self {
            loops: AtomicU32::new(0),
            ready_wakes: AtomicU32::new(0),
            timer_wakes: AtomicU32::new(0),
            link_updates: AtomicU32::new(0),
            rx_frames: AtomicU32::new(0),
            tx_frames: AtomicU32::new(0),
            tx_would_block: AtomicU32::new(0),
            tx_errors: AtomicU32::new(0),
        }
    }

    pub fn snapshot(&self) -> RunnerDebugSnapshot {
        RunnerDebugSnapshot {
            loops: self.loops.load(Ordering::Relaxed),
            ready_wakes: self.ready_wakes.load(Ordering::Relaxed),
            timer_wakes: self.timer_wakes.load(Ordering::Relaxed),
            link_updates: self.link_updates.load(Ordering::Relaxed),
            rx_frames: self.rx_frames.load(Ordering::Relaxed),
            tx_frames: self.tx_frames.load(Ordering::Relaxed),
            tx_would_block: self.tx_would_block.load(Ordering::Relaxed),
            tx_errors: self.tx_errors.load(Ordering::Relaxed),
        }
    }

    pub fn note_loop(&self) {
        self.loops.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_ready_wake(&self) {
        self.ready_wakes.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_timer_wake(&self) {
        self.timer_wakes.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_link_update(&self) {
        self.link_updates.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_rx_frame(&self) {
        self.rx_frames.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_tx_frame(&self) {
        self.tx_frames.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_tx_would_block(&self) {
        self.tx_would_block.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_tx_error(&self) {
        self.tx_errors.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::RunnerDebug;

    #[test]
    fn runner_debug_tracks_loop_progress() {
        let debug = RunnerDebug::new();

        debug.note_loop();
        debug.note_ready_wake();
        debug.note_timer_wake();
        debug.note_link_update();
        debug.note_rx_frame();
        debug.note_tx_frame();
        debug.note_tx_would_block();
        debug.note_tx_error();

        let snapshot = debug.snapshot();
        assert_eq!(snapshot.loops, 1);
        assert_eq!(snapshot.ready_wakes, 1);
        assert_eq!(snapshot.timer_wakes, 1);
        assert_eq!(snapshot.link_updates, 1);
        assert_eq!(snapshot.rx_frames, 1);
        assert_eq!(snapshot.tx_frames, 1);
        assert_eq!(snapshot.tx_would_block, 1);
        assert_eq!(snapshot.tx_errors, 1);
    }
}
