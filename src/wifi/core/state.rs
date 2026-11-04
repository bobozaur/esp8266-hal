#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RawMode {
    Sta,
    Ap,
    ApSta,
}
impl RawMode {
    pub(crate) fn has_station(self) -> bool {
        matches!(self, Self::Sta | Self::ApSta)
    }
    pub(crate) fn has_ap(self) -> bool {
        matches!(self, Self::Ap | Self::ApSta)
    }
}

pub(crate) struct Deadline {
    start: u64,
    timeout: u32,
}
impl Deadline {
    pub(crate) fn new(start: u64, timeout: u32) -> Self {
        Self { start, timeout }
    }
    pub(crate) fn is_expired(&self, now: u64) -> bool {
        now.saturating_sub(self.start) >= self.timeout as u64
    }
}
