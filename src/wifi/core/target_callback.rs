use core::ptr;

use portable_atomic::{AtomicPtr, Ordering};

use super::{InitError, WifiResources};

static ACTIVE_WIFI_RESOURCES: AtomicPtr<()> = AtomicPtr::new(ptr::null_mut());

pub(crate) struct CallbackRegistration<
    'd,
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize = 0,
    const ESP_NOW_RX: usize = 0,
    const PROMISCUOUS_RX: usize = 0,
    const AP_RX_SLOTS: usize = 0,
    const AP_RX_BYTES: usize = 0,
> {
    _marker: core::marker::PhantomData<
        &'d WifiResources<
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
    >,
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    >
    CallbackRegistration<
        'd,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    pub fn install(
        resources: &'d WifiResources<
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >,
    ) -> Result<Self, InitError> {
        ACTIVE_WIFI_RESOURCES
            .compare_exchange(
                ptr::null_mut(),
                resources as *const _ as *mut (),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| InitError::AlreadyInitialized)?;
        Ok(Self {
            _marker: core::marker::PhantomData,
        })
    }
}

impl<
        'd,
        const EVENTS: usize,
        const RX: usize,
        const ESP_NOW_PEERS: usize,
        const ESP_NOW_RX: usize,
        const PROMISCUOUS_RX: usize,
        const AP_RX_SLOTS: usize,
        const AP_RX_BYTES: usize,
    > Drop
    for CallbackRegistration<
        'd,
        EVENTS,
        RX,
        ESP_NOW_PEERS,
        ESP_NOW_RX,
        PROMISCUOUS_RX,
        AP_RX_SLOTS,
        AP_RX_BYTES,
    >
{
    fn drop(&mut self) {
        ACTIVE_WIFI_RESOURCES.store(ptr::null_mut(), Ordering::Release);
    }
}

/// Returns the resource block currently registered for SDK callbacks.
///
/// # Safety
///
/// The returned pointer is valid only while the matching [`CallbackRegistration`]
/// is alive. The registration is installed during backend initialization, cleared
/// before the backend drops its controller/resources association, and protected by
/// acquire/release ordering so callback trampolines either see a fully-initialized
/// `WifiResources` value or a null pointer.
pub(crate) unsafe fn active_resources<
    const EVENTS: usize,
    const RX: usize,
    const ESP_NOW_PEERS: usize,
    const ESP_NOW_RX: usize,
    const PROMISCUOUS_RX: usize,
    const AP_RX_SLOTS: usize,
    const AP_RX_BYTES: usize,
>() -> *const WifiResources<
    EVENTS,
    RX,
    ESP_NOW_PEERS,
    ESP_NOW_RX,
    PROMISCUOUS_RX,
    AP_RX_SLOTS,
    AP_RX_BYTES,
> {
    ACTIVE_WIFI_RESOURCES.load(Ordering::Acquire)
        as *const WifiResources<
            EVENTS,
            RX,
            ESP_NOW_PEERS,
            ESP_NOW_RX,
            PROMISCUOUS_RX,
            AP_RX_SLOTS,
            AP_RX_BYTES,
        >
}
