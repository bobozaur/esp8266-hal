/*
 * Bindgen wrapper header for esp8266-wifi-sys.
 *
 * Includes the ESP8266 RTOS SDK headers needed for WiFi, events, networking,
 * SmartConfig, WPS, and ESP-NOW bindings. The shims/sdkconfig.h must be on
 * the include path before SDK headers so that CONFIG_* defines are satisfied.
 */

/* Core types (esp_err_t, etc.) */
#include "esp_err.h"

/* Event base types (esp_event_base_t, esp_event_handler_t) */
#include "esp_event_base.h"

/* WiFi interface enum (ESP_IF_WIFI_STA, ESP_IF_WIFI_AP) */
#include "esp_interface.h"

/* WiFi types — structs, enums, constants */
#include "esp_wifi_types.h"

/* WiFi public API (esp_wifi_init, esp_wifi_start, etc.) */
#include "esp_wifi.h"

/* WiFi internal API (esp_wifi_internal_tx, reg_rxcb, free_rx_buffer) */
#include "esp_private/wifi.h"

/* Event loop API (esp_event_handler_register, esp_event_post) */
#include "esp_event.h"

/* Timer API (esp_timer_get_time for microsecond timestamps) */
#include "esp_timer.h"

/* SmartConfig provisioning */
#include "esp_smartconfig.h"

/* WPS (WiFi Protected Setup) */
#include "esp_supplicant/esp_wps.h"

/* ESP-NOW peer-to-peer protocol */
#include "esp_now.h"

/* System utilities (esp_restart, chip info, random) */
#include "esp_system.h"
