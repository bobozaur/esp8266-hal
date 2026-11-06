/* Wi-Fi-only native inputs and the two intentional closed-binary substitutions.
 * Direct assignments override definitions in selected SDK objects, including
 * calls from the same object. Unrelated duplicate symbols remain link errors.
 * The original vendor archives remain unchanged.
 */
INCLUDE esp8266.rom.ld
INCLUDE esp8266.peripherals.ld
EXTERN(__esp8266_hal_ieee80211_deliver_data)
ieee80211_deliver_data = __esp8266_hal_ieee80211_deliver_data;

EXTERN(__esp8266_hal_pm_wait4wakeup)
pm_wait4wakeup = __esp8266_hal_pm_wait4wakeup;
