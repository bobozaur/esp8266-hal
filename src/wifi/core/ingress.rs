//! Serialized native event ingress. Only the wireless events used by this
//! driver cross this boundary; no SDK event registry or IP stack is involved.
//!
//! Event IDs and payload contracts follow Espressif's Apache-2.0 licensed
//! `esp_event/event_send_compat.inc` in the pinned ESP8266 RTOS SDK.

use esp8266_wifi_sys as sys;

use super::WifiEvent;

#[derive(Default)]
struct Subscription {
    generation: u32,
    enabled: bool,
}

impl Subscription {
    fn set_enabled(&mut self, enabled: bool) {
        if self.enabled != enabled {
            self.generation = self.generation.wrapping_add(1);
            self.enabled = enabled;
        }
    }

    fn accepts(&self, generation: u32) -> bool {
        self.enabled && self.generation == generation
    }
}

fn wifi_event(raw: &sys::system_event_t) -> Option<WifiEvent> {
    // SAFETY: the native event ABI initializes the union member selected by
    // event_id. Copy the needed fields before returning to the producer.
    unsafe {
        Some(match raw.event_id {
            sys::SYSTEM_EVENT_SCAN_DONE => WifiEvent::ScanDone {
                status: raw.event_info.scan_done.status,
            },
            sys::SYSTEM_EVENT_STA_START => WifiEvent::StaStarted,
            sys::SYSTEM_EVENT_STA_STOP => WifiEvent::StaStopped,
            sys::SYSTEM_EVENT_STA_CONNECTED => WifiEvent::StaConnected,
            sys::SYSTEM_EVENT_STA_DISCONNECTED => WifiEvent::StaDisconnected {
                reason: Some(raw.event_info.disconnected.reason),
            },
            sys::SYSTEM_EVENT_AP_START => WifiEvent::ApStarted,
            sys::SYSTEM_EVENT_AP_STOP => WifiEvent::ApStopped,
            #[cfg(feature = "wifi-wps")]
            sys::SYSTEM_EVENT_STA_WPS_ER_SUCCESS => WifiEvent::WpsSucceeded,
            #[cfg(feature = "wifi-wps")]
            sys::SYSTEM_EVENT_STA_WPS_ER_FAILED => WifiEvent::WpsFailed {
                reason: Some(raw.event_info.sta_er_fail_reason),
            },
            #[cfg(feature = "wifi-wps")]
            sys::SYSTEM_EVENT_STA_WPS_ER_TIMEOUT => WifiEvent::WpsTimedOut,
            #[cfg(feature = "wifi-wps")]
            sys::SYSTEM_EVENT_STA_WPS_ER_PIN => WifiEvent::WpsPin {
                pin: super::WpsPin::from_digits(raw.event_info.sta_er_pin.pin_code).ok()?,
            },
            _ => return None,
        })
    }
}

#[cfg(all(target_arch = "xtensa", feature = "wifi-smartconfig"))]
pub(super) use runtime::set_smartconfig_enabled;
#[cfg(all(
    target_arch = "xtensa",
    any(feature = "wifi-wps", feature = "wifi-smartconfig")
))]
pub(super) use runtime::synchronize;
#[cfg(target_arch = "xtensa")]
pub(super) use runtime::{initialize, set_wifi_enabled};

#[cfg(target_arch = "xtensa")]
mod runtime {
    use core::{ffi::c_void, mem::MaybeUninit, ptr};

    #[cfg(any(feature = "wifi-wps", feature = "wifi-smartconfig"))]
    use portable_atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::wifi::compat::{queue, semphr, task};
    use crate::wifi_core::{target_callback::CallbackInvocation, DriverError, InitError};

    #[cfg(feature = "wifi-smartconfig")]
    use crate::wifi_core::{allocator::RuntimeAllocator, SmartConfigCredentials};
    #[cfg(feature = "wifi-smartconfig")]
    use allocator_api2::boxed::Box;

    // Preserve the pinned native queue depth, priority and stack request. The
    // runtime adds its exception frame to this stack allocation.
    const QUEUE_DEPTH: u32 = 8;
    const STACK_BYTES: u16 = 1024;
    const PRIORITY: u32 = 10;

    enum Pending {
        Wifi {
            generation: u32,
            event: WifiEvent,
        },
        #[cfg(feature = "wifi-smartconfig")]
        SmartConfig {
            generation: u32,
            event: WifiEvent,
        },
        #[cfg(feature = "wifi-smartconfig")]
        Credentials {
            generation: u32,
            value: Box<SmartConfigCredentials, RuntimeAllocator>,
        },
        #[cfg(any(feature = "wifi-wps", feature = "wifi-smartconfig"))]
        Fence(u32),
    }

    struct Service {
        queue: *mut c_void,
        mutex: *mut c_void,
        task: *mut c_void,
    }
    static mut SERVICE: Service = Service {
        queue: ptr::null_mut(),
        mutex: ptr::null_mut(),
        task: ptr::null_mut(),
    };
    static WIFI: super::super::locked::Locked<Subscription> =
        super::super::locked::Locked::new(Subscription {
            generation: 0,
            enabled: false,
        });
    #[cfg(feature = "wifi-smartconfig")]
    static SMARTCONFIG: super::super::locked::Locked<Subscription> =
        super::super::locked::Locked::new(Subscription {
            generation: 0,
            enabled: false,
        });

    struct DeliveryLock;
    impl DeliveryLock {
        fn acquire() -> Result<Self, DriverError> {
            // The service lives for the boot. Initialization precedes native
            // producers; the recursive mutex also permits waker reentrancy.
            let wait = if unsafe {
                crate::wifi::compat::critical::interrupt_is_disable()
                    || task::xTaskGetSchedulerState() != 2
            } {
                0
            } else {
                u32::MAX
            };
            if unsafe { semphr::xSemaphoreTakeRecursive(SERVICE.mutex, wait) } == 1 {
                Ok(Self)
            } else {
                Err(DriverError::Busy)
            }
        }
    }
    impl Drop for DeliveryLock {
        fn drop(&mut self) {
            unsafe { semphr::xSemaphoreGiveRecursive(SERVICE.mutex) };
        }
    }

    pub(crate) fn initialize() -> Result<(), InitError> {
        // The Wi-Fi singleton serializes initialization. Publish all handles
        // before the new higher-priority task can run, and roll back failures.
        critical_section::with(|_| unsafe {
            if !SERVICE.queue.is_null() {
                return Ok(());
            }
            let queue =
                queue::xQueueGenericCreate(QUEUE_DEPTH, core::mem::size_of::<Pending>() as u32, 0);
            if queue.is_null() {
                return Err(InitError::OutOfMemory);
            }
            let mutex = semphr::xSemaphoreCreateRecursiveMutex();
            if mutex.is_null() {
                queue::vQueueDelete(queue);
                return Err(InitError::OutOfMemory);
            }
            SERVICE.queue = queue;
            SERVICE.mutex = mutex;
            if task::xTaskCreate(
                dispatch,
                ptr::null(),
                STACK_BYTES,
                queue,
                PRIORITY,
                ptr::addr_of_mut!(SERVICE.task),
            ) != 1
            {
                SERVICE.queue = ptr::null_mut();
                SERVICE.mutex = ptr::null_mut();
                semphr::vSemaphoreDelete(mutex);
                queue::vQueueDelete(queue);
                return Err(InitError::OutOfMemory);
            }
            Ok(())
        })
    }

    pub(crate) fn set_wifi_enabled(enabled: bool) -> Result<(), DriverError> {
        let _delivery = DeliveryLock::acquire()?;
        WIFI.with(|subscription| subscription.set_enabled(enabled));
        Ok(())
    }

    #[cfg(feature = "wifi-smartconfig")]
    pub(crate) fn set_smartconfig_enabled(enabled: bool) -> Result<(), DriverError> {
        let _delivery = DeliveryLock::acquire()?;
        SMARTCONFIG.with(|subscription| subscription.set_enabled(enabled));
        Ok(())
    }

    fn post(event: Pending, wait: u32) -> i32 {
        unsafe {
            if SERVICE.queue.is_null() {
                return sys::ESP_ERR_INVALID_STATE;
            }
            // Never wait on our own full queue from a reentrant application waker.
            let wait = if task::xTaskGetCurrentTaskHandle() == SERVICE.task {
                0
            } else {
                wait
            };
            if queue::xQueueGenericSend(SERVICE.queue, (&event as *const Pending).cast(), wait, 0)
                != 1
            {
                return sys::ESP_ERR_TIMEOUT;
            }
            // The queue's byte copy transfers ownership, including any credential
            // allocation. Exactly one receiver reconstructs and drops this value.
            core::mem::forget(event);
            sys::ESP_OK
        }
    }

    unsafe extern "C" fn dispatch(queue_handle: *mut c_void) {
        loop {
            let mut pending = MaybeUninit::<Pending>::uninit();
            if unsafe { queue::xQueueReceive(queue_handle, pending.as_mut_ptr().cast(), u32::MAX) }
                != 1
            {
                continue;
            }
            // SAFETY: only post writes whole Pending values to this private queue.
            let pending = unsafe { pending.assume_init() };
            let _delivery = loop {
                if let Ok(delivery) = DeliveryLock::acquire() {
                    break delivery;
                }
                unsafe { task::vTaskDelay(1) };
            };
            let invocation = CallbackInvocation::enter();
            match pending {
                Pending::Wifi { generation, event } if WIFI.with(|s| s.accepts(generation)) => {
                    if let Some(resources) = invocation.resources() {
                        let _ = resources.publish_event(event);
                    }
                }
                #[cfg(feature = "wifi-smartconfig")]
                Pending::SmartConfig { generation, event }
                    if SMARTCONFIG.with(|s| s.accepts(generation)) =>
                {
                    if let Some(resources) = invocation.resources() {
                        let _ = resources.publish_event(event);
                    }
                }
                #[cfg(feature = "wifi-smartconfig")]
                Pending::Credentials { generation, value }
                    if SMARTCONFIG.with(|s| s.accepts(generation)) =>
                {
                    if let Some(resources) = invocation.resources() {
                        let _ = resources.publish_smartconfig_credentials(*value);
                    }
                }
                #[cfg(any(feature = "wifi-wps", feature = "wifi-smartconfig"))]
                Pending::Fence(token) => FENCE_ACK.store(token, Ordering::Release),
                _ => {}
            }
        }
    }

    /// Native producers borrow their event until return; ordinary lifecycle
    /// events are copied inline and require no payload allocation.
    #[no_mangle]
    unsafe extern "C" fn esp_event_send(raw: *const sys::system_event_t) -> i32 {
        let Some(raw) = (unsafe { raw.as_ref() }) else {
            return sys::ESP_ERR_INVALID_ARG;
        };
        let Some(event) = wifi_event(raw) else {
            return sys::ESP_OK;
        };
        let generation = WIFI.with(|s| s.enabled.then_some(s.generation));
        generation.map_or(sys::ESP_OK, |generation| {
            post(Pending::Wifi { generation, event }, 0)
        })
    }

    #[cfg(feature = "wifi-smartconfig")]
    #[no_mangle]
    static mut SC_EVENT: sys::esp_event_base_t = b"SC_EVENT\0".as_ptr().cast();

    #[cfg(feature = "wifi-smartconfig")]
    #[no_mangle]
    unsafe extern "C" fn esp_event_post(
        base: sys::esp_event_base_t,
        id: i32,
        data: *const c_void,
        size: usize,
        wait: u32,
    ) -> i32 {
        if base != unsafe { SC_EVENT } {
            return sys::ESP_ERR_NOT_SUPPORTED;
        }
        let Some(generation) = SMARTCONFIG.with(|s| s.enabled.then_some(s.generation)) else {
            return sys::ESP_OK;
        };
        let event = match id as u32 {
            sys::SC_EVENT_SCAN_DONE => WifiEvent::SmartConfigScanDone,
            sys::SC_EVENT_FOUND_CHANNEL => WifiEvent::SmartConfigFoundChannel,
            sys::SC_EVENT_SEND_ACK_DONE => WifiEvent::SmartConfigSendAckDone,
            sys::SC_EVENT_GOT_SSID_PSWD => {
                if data.is_null()
                    || size != core::mem::size_of::<sys::smartconfig_event_got_ssid_pswd_t>()
                {
                    return sys::ESP_ERR_INVALID_ARG;
                }
                let raw = unsafe { &*data.cast::<sys::smartconfig_event_got_ssid_pswd_t>() };
                use crate::wifi_core::{target_map::*, Ipv4Addr};
                let value = match SmartConfigCredentials::new(
                    &raw.ssid[..nul_terminated_len(&raw.ssid)],
                    &raw.password[..nul_terminated_len(&raw.password)],
                    raw.bssid_set.then_some(raw.bssid),
                    map_smartconfig_type_from_raw(raw.type_),
                    raw.token,
                    Ipv4Addr::new(
                        raw.cellphone_ip[0],
                        raw.cellphone_ip[1],
                        raw.cellphone_ip[2],
                        raw.cellphone_ip[3],
                    ),
                ) {
                    Ok(value) => value,
                    Err(_) => return sys::ESP_ERR_INVALID_ARG,
                };
                let allocator = RuntimeAllocator::new(crate::wifi::compat::memory::allocator());
                let value = match Box::try_new_in(value, allocator) {
                    Ok(value) => value,
                    Err(_) => return sys::ESP_ERR_NO_MEM,
                };
                return post(Pending::Credentials { generation, value }, wait);
            }
            _ => return sys::ESP_ERR_NOT_SUPPORTED,
        };
        post(Pending::SmartConfig { generation, event }, wait)
    }

    #[cfg(any(feature = "wifi-wps", feature = "wifi-smartconfig"))]
    static FENCE_NEXT: AtomicU32 = AtomicU32::new(1);
    #[cfg(any(feature = "wifi-wps", feature = "wifi-smartconfig"))]
    static FENCE_ACK: AtomicU32 = AtomicU32::new(0);

    #[cfg(any(feature = "wifi-wps", feature = "wifi-smartconfig"))]
    pub(crate) fn synchronize() -> Result<(), DriverError> {
        if unsafe { task::xTaskGetCurrentTaskHandle() == SERVICE.task } {
            return Err(DriverError::Busy);
        }
        // Native stop has removed the producers. A FIFO marker drains their
        // already queued events before provisioning observation is reset.
        let token = FENCE_NEXT.fetch_add(1, Ordering::Relaxed);
        crate::wifi::EspError::check(post(Pending::Fence(token), 0))
            .map_err(super::super::target::map_esp_driver_error)?;
        let start = unsafe { task::get_tick_count() };
        while FENCE_ACK.load(Ordering::Acquire) != token {
            if unsafe { task::get_tick_count() }.wrapping_sub(start) >= 100 {
                return Err(DriverError::Timeout);
            }
            unsafe { task::vTaskDelay(1) };
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_and_restart_reject_queued_events_from_previous_subscription() {
        let mut subscription = Subscription::default();
        assert!(!subscription.accepts(0));
        subscription.set_enabled(true);
        let queued = subscription.generation;
        subscription.set_enabled(true);
        assert!(subscription.accepts(queued));
        subscription.set_enabled(false);
        assert!(!subscription.accepts(queued));
        subscription.set_enabled(true);
        assert!(!subscription.accepts(queued));
        assert!(subscription.accepts(subscription.generation));
    }

    #[test]
    fn legacy_event_ids_map_directly_without_the_sdk_event_translation() {
        let mut raw: sys::system_event_t = unsafe { core::mem::zeroed() };
        raw.event_id = sys::SYSTEM_EVENT_STA_DISCONNECTED;
        raw.event_info.disconnected.reason = 201;
        assert_eq!(
            wifi_event(&raw),
            Some(WifiEvent::StaDisconnected { reason: Some(201) })
        );
        raw.event_id = sys::SYSTEM_EVENT_AP_START;
        assert_eq!(wifi_event(&raw), Some(WifiEvent::ApStarted));
        raw.event_id = sys::SYSTEM_EVENT_STA_GOT_IP;
        assert_eq!(wifi_event(&raw), None);
    }

    #[cfg(feature = "wifi-wps")]
    #[test]
    fn malformed_native_wps_pin_is_not_published() {
        let mut raw: sys::system_event_t = unsafe { core::mem::zeroed() };
        raw.event_id = sys::SYSTEM_EVENT_STA_WPS_ER_PIN;
        raw.event_info.sta_er_pin.pin_code = *b"12345670";
        assert!(matches!(wifi_event(&raw), Some(WifiEvent::WpsPin { .. })));
        raw.event_info.sta_er_pin.pin_code = *b"12345671";
        assert_eq!(wifi_event(&raw), None);
    }
}
