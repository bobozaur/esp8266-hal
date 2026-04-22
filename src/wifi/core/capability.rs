use super::state::{ApMode, MixedMode, StaMode};

pub trait HasSta {}
pub trait HasAp {}

impl HasSta for StaMode {}
impl HasSta for MixedMode {}
impl HasAp for ApMode {}
impl HasAp for MixedMode {}
