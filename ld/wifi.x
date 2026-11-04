/* Wi-Fi-only native inputs and the three intentional ABI substitutions.
 * Direct assignments override definitions in selected SDK objects, including
 * calls from the same object. Unrelated duplicate symbols remain link errors.
 * The original vendor archives remain unchanged.
 */
INCLUDE esp8266.rom.ld
INCLUDE esp8266.peripherals.ld
/* Select SDK syscall.o before newlib can select its separate sbrk-based heap.
 * Its reentrant allocation functions forward to the HAL's heap_caps ABI. */
EXTERN(abort ets_printf ets_delay_us)

EXTERN(__esp8266_hal_esp_timer_get_time)
esp_timer_get_time = __esp8266_hal_esp_timer_get_time;

EXTERN(__esp8266_hal_ieee80211_deliver_data)
ieee80211_deliver_data = __esp8266_hal_ieee80211_deliver_data;

EXTERN(__esp8266_hal_pm_wait4wakeup)
pm_wait4wakeup = __esp8266_hal_pm_wait4wakeup;
