use core::marker::PhantomData;

pub struct Uninitialized;
pub struct Initialized;
pub struct Configured<M>(pub(crate) PhantomData<M>);
pub struct Started<M>(pub(crate) PhantomData<M>);

pub struct StaMode;
pub struct ApMode;
pub struct MixedMode;
