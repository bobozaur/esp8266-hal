#include <ctype.h>
#include <errno.h>
#include <inttypes.h>
#include <stdbool.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "driver/uart.h"
#include "esp_err.h"
#include "esp_event.h"
#include "esp_mac.h"
#include "esp_now.h"
#include "esp_netif.h"
#include "esp_netif_net_stack.h"
#include "esp_system.h"
#include "esp_wifi.h"
#include "esp_wps.h"
#include "freertos/FreeRTOS.h"
#include "freertos/event_groups.h"
#include "freertos/task.h"
#include "lwip/err.h"
#include "lwip/etharp.h"
#include "lwip/inet.h"
#include "lwip/netif.h"
#include "lwip/sockets.h"
#include "nvs_flash.h"

#define COMPANION_UART UART_NUM_0
#define COMPANION_UART_BAUD 115200
#define COMMAND_BUFFER_SIZE 512
#define RESPONSE_BUFFER_SIZE 768
#define UDP_BUFFER_SIZE 1460
#define DHCP_PACKET_SIZE 300
#define DHCP_CLIENT_PORT 68
#define DHCP_SERVER_PORT 67
#define ESPNOW_MAX_PAYLOAD 250
#define RAW80211_MAX_FRAME_SIZE 256
#define RAW80211_DEFAULT_DATA_PAYLOAD_SIZE 48
#define DEFAULT_STA_TIMEOUT_MS 15000
#define DEFAULT_ESPNOW_TIMEOUT_MS 10000
#define DEFAULT_SMARTCONFIG_SEND_TIMEOUT_MS 90000
#define ESPTOUCH_EXTRA_LEN 40
#define ESPTOUCH_EXTRA_HEAD_LEN 5
#define ESPTOUCH_MAX_DATA_CODES 128
#define ESPTOUCH_MAX_LENGTH_CODES (ESPTOUCH_MAX_DATA_CODES * 3)
#define ESPTOUCH_GUIDE_MS 2000
#define ESPTOUCH_TOTAL_PERIOD_MS 6000
#define ESPTOUCH_INTERVAL_MS 8
#define ESPTOUCH_TARGET_PORT 7001
#define ESPTOUCH_ACK_PORT 18266

#define WIFI_CONNECTED_BIT BIT0
#define WIFI_GOT_IP_BIT BIT1
#define WIFI_DISCONNECTED_BIT BIT2
#define ESPNOW_SEND_DONE_BIT BIT3
#define ESPNOW_RECV_DONE_BIT BIT4
#define WPS_AP_SUCCESS_BIT BIT5
#define WPS_AP_FAILED_BIT BIT6
#define WPS_AP_TIMEOUT_BIT BIT7
#define TCP_SERVE_READY_BIT BIT8
#define TCP_SERVE_DONE_BIT BIT9

#define DHCP_OPT_MESSAGE_TYPE 53
#define DHCP_OPT_REQUESTED_IP 50
#define DHCP_OPT_SERVER_IDENTIFIER 54
#define DHCP_OPT_PARAMETER_REQUEST_LIST 55
#define DHCP_OPT_END 255

typedef struct {
    const char *key;
    const char *value;
} arg_pair_t;

static EventGroupHandle_t s_events;
static portMUX_TYPE s_state_lock = portMUX_INITIALIZER_UNLOCKED;
static esp_netif_t *s_sta_netif;
static esp_netif_t *s_ap_netif;
static bool s_wifi_started;
static bool s_espnow_active;
static esp_now_send_status_t s_last_send_status;
static bool s_last_send_valid;
static bool s_last_recv_valid;
static uint8_t s_last_recv_peer[6];
static uint8_t s_last_recv_payload[ESPNOW_MAX_PAYLOAD];
static int s_last_recv_len;
static wifi_event_sta_disconnected_t s_last_disconnect;
static esp_ip4_addr_t s_last_ipv4;
static int s_last_tcp_errno;
static int s_last_tcp_arp_result;
static int s_last_tcp_sta_count;
static const char *s_last_tcp_stage;
static TaskHandle_t s_tcp_serve_task;
static int s_tcp_serve_listen_fd = -1;
static int s_tcp_serve_client_fd = -1;
static esp_err_t s_tcp_serve_status;
static uint8_t s_last_wps_peer[6];
static int s_last_wps_reason;
static uint32_t s_wps_ap_failures;
static uint32_t s_wps_ap_timeouts;
static uint32_t s_wps_ap_restarts;
static esp_err_t s_last_wps_restart_err;
static bool s_wps_ap_active;
static bool s_wps_ap_restarting;
static bool s_wps_ap_pin_mode;
static unsigned char s_wps_ap_pin[9];
static const char *s_last_smartconfig_step = "none";
static int s_last_smartconfig_errno;

static void write_line(const char *prefix, const char *fmt, ...) {
    char line[RESPONSE_BUFFER_SIZE];
    int offset = snprintf(line, sizeof(line), "%s", prefix);
    if (offset < 0) {
        return;
    }

    if (fmt != NULL && offset < (int)sizeof(line)) {
        va_list args;
        va_start(args, fmt);
        int written = vsnprintf(line + offset, sizeof(line) - (size_t)offset, fmt, args);
        va_end(args);
        if (written > 0) {
            offset += written;
        }
    }

    size_t bounded = (size_t)offset;
    if (bounded >= sizeof(line)) {
        bounded = sizeof(line) - 1;
    }
    line[bounded++] = '\n';
    uart_write_bytes(COMPANION_UART, line, bounded);
}

static void reply_ok(const char *fmt, ...) {
    char line[RESPONSE_BUFFER_SIZE];
    int offset = snprintf(line, sizeof(line), "OK ");
    if (offset < 0) {
        return;
    }

    if (fmt != NULL && offset < (int)sizeof(line)) {
        va_list args;
        va_start(args, fmt);
        int written = vsnprintf(line + offset, sizeof(line) - (size_t)offset, fmt, args);
        va_end(args);
        if (written > 0) {
            offset += written;
        }
    }

    size_t bounded = (size_t)offset;
    if (bounded >= sizeof(line)) {
        bounded = sizeof(line) - 1;
    }
    line[bounded++] = '\n';
    uart_write_bytes(COMPANION_UART, line, bounded);
}

static void reply_err(const char *code, const char *fmt, ...) {
    char line[RESPONSE_BUFFER_SIZE];
    int offset = snprintf(line, sizeof(line), "ERR code=%s", code);
    if (offset < 0) {
        return;
    }

    if (fmt != NULL && offset < (int)sizeof(line)) {
        int written = snprintf(line + offset, sizeof(line) - (size_t)offset, " detail=");
        if (written > 0) {
            offset += written;
        }
    }

    if (fmt != NULL && offset < (int)sizeof(line)) {
        va_list args;
        va_start(args, fmt);
        int written = vsnprintf(line + offset, sizeof(line) - (size_t)offset, fmt, args);
        va_end(args);
        if (written > 0) {
            offset += written;
        }
    }

    size_t bounded = (size_t)offset;
    if (bounded >= sizeof(line)) {
        bounded = sizeof(line) - 1;
    }
    line[bounded++] = '\n';
    uart_write_bytes(COMPANION_UART, line, bounded);
}

static void format_mac(const uint8_t mac[6], char *buffer, size_t len) {
    snprintf(
        buffer,
        len,
        "%02x:%02x:%02x:%02x:%02x:%02x",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5]
    );
}

static void format_ipv4(esp_ip4_addr_t addr, char *buffer, size_t len) {
    snprintf(
        buffer,
        len,
        "%" PRIu32 ".%" PRIu32 ".%" PRIu32 ".%" PRIu32,
        (uint32_t)ip4_addr1(&addr),
        (uint32_t)ip4_addr2(&addr),
        (uint32_t)ip4_addr3(&addr),
        (uint32_t)ip4_addr4(&addr)
    );
}

static const char *auth_mode_name(wifi_auth_mode_t authmode) {
    switch (authmode) {
        case WIFI_AUTH_OPEN:
            return "open";
        case WIFI_AUTH_WEP:
            return "wep";
        case WIFI_AUTH_WPA_PSK:
            return "wpa";
        case WIFI_AUTH_WPA2_PSK:
            return "wpa2";
        case WIFI_AUTH_WPA_WPA2_PSK:
            return "wpa+wpa2";
        case WIFI_AUTH_WPA2_ENTERPRISE:
            return "wpa2-enterprise";
        case WIFI_AUTH_WPA3_PSK:
            return "wpa3";
        case WIFI_AUTH_WPA2_WPA3_PSK:
            return "wpa2+wpa3";
        case WIFI_AUTH_WAPI_PSK:
            return "wapi";
        case WIFI_AUTH_OWE:
            return "owe";
        case WIFI_AUTH_WPA3_ENT_192:
            return "wpa3-ent-192";
        default:
            return "unknown";
    }
}

static size_t hex_encode(const uint8_t *input, size_t input_len, char *output, size_t output_len) {
    static const char digits[] = "0123456789abcdef";
    size_t written = 0;
    for (size_t index = 0; index < input_len && written + 2 < output_len; index++) {
        output[written++] = digits[input[index] >> 4];
        output[written++] = digits[input[index] & 0x0f];
    }
    if (written < output_len) {
        output[written] = '\0';
    }
    return written;
}

static int hex_nibble(char value) {
    if (value >= '0' && value <= '9') {
        return value - '0';
    }
    if (value >= 'a' && value <= 'f') {
        return value - 'a' + 10;
    }
    if (value >= 'A' && value <= 'F') {
        return value - 'A' + 10;
    }
    return -1;
}

static bool parse_hex_bytes(const char *text, uint8_t *output, size_t output_len, size_t *written) {
    size_t text_len = strlen(text);
    if ((text_len % 2) != 0 || (text_len / 2) > output_len) {
        return false;
    }

    size_t out_index = 0;
    for (size_t index = 0; index < text_len; index += 2) {
        int high = hex_nibble(text[index]);
        int low = hex_nibble(text[index + 1]);
        if (high < 0 || low < 0) {
            return false;
        }
        output[out_index++] = (uint8_t)((high << 4) | low);
    }

    if (written != NULL) {
        *written = out_index;
    }
    return true;
}

static bool parse_mac(const char *text, uint8_t mac[6]) {
    if (strlen(text) != 17) {
        return false;
    }
    for (size_t index = 0; index < 6; index++) {
        int high = hex_nibble(text[index * 3]);
        int low = hex_nibble(text[index * 3 + 1]);
        if (high < 0 || low < 0) {
            return false;
        }
        mac[index] = (uint8_t)((high << 4) | low);
        if (index < 5 && text[index * 3 + 2] != ':') {
            return false;
        }
    }
    return true;
}

static bool parse_u32(const char *text, uint32_t *value) {
    char *end = NULL;
    errno = 0;
    unsigned long parsed = strtoul(text, &end, 10);
    if (errno != 0 || end == text || *end != '\0' || parsed > UINT32_MAX) {
        return false;
    }
    *value = (uint32_t)parsed;
    return true;
}

static bool parse_ipv4(const char *text, esp_ip4_addr_t *out) {
    struct in_addr addr;
    if (inet_aton(text, &addr) == 0) {
        return false;
    }
    out->addr = addr.s_addr;
    return true;
}

static const char *find_arg(const arg_pair_t *args, size_t arg_count, const char *key) {
    for (size_t index = 0; index < arg_count; index++) {
        if (strcmp(args[index].key, key) == 0) {
            return args[index].value;
        }
    }
    return NULL;
}

static void clear_async_state(void) {
    portENTER_CRITICAL(&s_state_lock);
    memset(&s_last_disconnect, 0, sizeof(s_last_disconnect));
    memset(&s_last_ipv4, 0, sizeof(s_last_ipv4));
    s_last_send_valid = false;
    s_last_recv_valid = false;
    s_last_recv_len = 0;
    memset(s_last_wps_peer, 0, sizeof(s_last_wps_peer));
    s_last_wps_reason = 0;
    s_wps_ap_failures = 0;
    s_wps_ap_timeouts = 0;
    s_wps_ap_restarts = 0;
    s_last_wps_restart_err = ESP_OK;
    s_wps_ap_restarting = false;
    portEXIT_CRITICAL(&s_state_lock);
    xEventGroupClearBits(
        s_events,
        WIFI_CONNECTED_BIT | WIFI_GOT_IP_BIT | WIFI_DISCONNECTED_BIT | ESPNOW_SEND_DONE_BIT |
            ESPNOW_RECV_DONE_BIT | WPS_AP_SUCCESS_BIT | WPS_AP_FAILED_BIT | WPS_AP_TIMEOUT_BIT |
            TCP_SERVE_READY_BIT | TCP_SERVE_DONE_BIT
    );
}

static bool wps_disable_error_is_ignorable(esp_err_t err) {
    return err == ESP_OK || err == ESP_ERR_WIFI_MODE || err == ESP_ERR_WIFI_WPS_SM ||
           err == ESP_ERR_WIFI_NOT_STARTED || err == ESP_FAIL;
}

static void wps_ap_mark_inactive(void) {
    portENTER_CRITICAL(&s_state_lock);
    s_wps_ap_active = false;
    s_wps_ap_restarting = false;
    portEXIT_CRITICAL(&s_state_lock);
}

static esp_err_t stop_wps_ap_registrar(void) {
    wps_ap_mark_inactive();
    esp_err_t err = esp_wifi_ap_wps_disable();
    return wps_disable_error_is_ignorable(err) ? ESP_OK : err;
}

static void record_wps_ap_restart_result(esp_err_t err) {
    portENTER_CRITICAL(&s_state_lock);
    s_last_wps_restart_err = err;
    if (err == ESP_OK) {
        s_wps_ap_restarts++;
    } else {
        s_wps_ap_active = false;
    }
    s_wps_ap_restarting = false;
    portEXIT_CRITICAL(&s_state_lock);
}

static esp_err_t restart_wps_ap_registrar(void) {
    bool pin_mode;
    unsigned char pin[9];

    portENTER_CRITICAL(&s_state_lock);
    if (s_wps_ap_restarting) {
        portEXIT_CRITICAL(&s_state_lock);
        return ESP_OK;
    }
    s_wps_ap_restarting = true;
    pin_mode = s_wps_ap_pin_mode;
    memcpy(pin, s_wps_ap_pin, sizeof(pin));
    portEXIT_CRITICAL(&s_state_lock);

    esp_err_t err = esp_wifi_ap_wps_disable();
    if (!wps_disable_error_is_ignorable(err)) {
        record_wps_ap_restart_result(err);
        return err;
    }

    esp_wps_config_t config = WPS_CONFIG_INIT_DEFAULT(pin_mode ? WPS_TYPE_PIN : WPS_TYPE_PBC);
    err = esp_wifi_ap_wps_enable(&config);
    if (err != ESP_OK) {
        record_wps_ap_restart_result(err);
        return err;
    }

    err = esp_wifi_ap_wps_start(pin_mode ? (const unsigned char *)pin : NULL);
    record_wps_ap_restart_result(err);
    return err;
}

static void wifi_event_handler(
    void *arg,
    esp_event_base_t event_base,
    int32_t event_id,
    void *event_data
) {
    (void)arg;

    if (event_base == WIFI_EVENT && event_id == WIFI_EVENT_STA_CONNECTED) {
        xEventGroupSetBits(s_events, WIFI_CONNECTED_BIT);
        xEventGroupClearBits(s_events, WIFI_DISCONNECTED_BIT);
        return;
    }

    if (event_base == WIFI_EVENT && event_id == WIFI_EVENT_STA_DISCONNECTED) {
        portENTER_CRITICAL(&s_state_lock);
        memcpy(&s_last_disconnect, event_data, sizeof(s_last_disconnect));
        memset(&s_last_ipv4, 0, sizeof(s_last_ipv4));
        portEXIT_CRITICAL(&s_state_lock);
        xEventGroupClearBits(s_events, WIFI_CONNECTED_BIT | WIFI_GOT_IP_BIT);
        xEventGroupSetBits(s_events, WIFI_DISCONNECTED_BIT);
        return;
    }

    if (event_base == WIFI_EVENT && event_id == WIFI_EVENT_AP_WPS_RG_SUCCESS) {
        const wifi_event_ap_wps_rg_success_t *event = event_data;
        portENTER_CRITICAL(&s_state_lock);
        if (event != NULL) {
            memcpy(s_last_wps_peer, event->peer_macaddr, sizeof(s_last_wps_peer));
        } else {
            memset(s_last_wps_peer, 0, sizeof(s_last_wps_peer));
        }
        s_last_wps_reason = 0;
        s_wps_ap_active = false;
        portEXIT_CRITICAL(&s_state_lock);
        write_line("EV ", "WPS_AP_SUCCESS");
        xEventGroupSetBits(s_events, WPS_AP_SUCCESS_BIT);
        return;
    }

    if (event_base == WIFI_EVENT && event_id == WIFI_EVENT_AP_WPS_RG_FAILED) {
        const wifi_event_ap_wps_rg_fail_reason_t *event = event_data;
        bool restart;
        esp_err_t restart_err = ESP_OK;
        portENTER_CRITICAL(&s_state_lock);
        if (event != NULL) {
            memcpy(s_last_wps_peer, event->peer_macaddr, sizeof(s_last_wps_peer));
            s_last_wps_reason = event->reason;
        } else {
            memset(s_last_wps_peer, 0, sizeof(s_last_wps_peer));
            s_last_wps_reason = -1;
        }
        s_wps_ap_failures++;
        restart = s_wps_ap_active && !s_wps_ap_restarting;
        portEXIT_CRITICAL(&s_state_lock);
        write_line("EV ", "WPS_AP_FAILED reason=%d", event != NULL ? event->reason : -1);
        if (restart) {
            restart_err = restart_wps_ap_registrar();
        }
        if (!restart || restart_err != ESP_OK) {
            xEventGroupSetBits(s_events, WPS_AP_FAILED_BIT);
        }
        return;
    }

    if (event_base == WIFI_EVENT && event_id == WIFI_EVENT_AP_WPS_RG_TIMEOUT) {
        bool restart;
        esp_err_t restart_err = ESP_OK;
        portENTER_CRITICAL(&s_state_lock);
        s_wps_ap_timeouts++;
        restart = s_wps_ap_active && !s_wps_ap_restarting;
        portEXIT_CRITICAL(&s_state_lock);
        write_line("EV ", "WPS_AP_TIMEOUT");
        if (restart) {
            restart_err = restart_wps_ap_registrar();
        }
        if (!restart || restart_err != ESP_OK) {
            xEventGroupSetBits(s_events, restart_err == ESP_OK ? WPS_AP_TIMEOUT_BIT : WPS_AP_FAILED_BIT);
        }
        return;
    }

    if (event_base == IP_EVENT && event_id == IP_EVENT_STA_GOT_IP) {
        const ip_event_got_ip_t *event = event_data;
        portENTER_CRITICAL(&s_state_lock);
        s_last_ipv4 = event->ip_info.ip;
        portEXIT_CRITICAL(&s_state_lock);
        xEventGroupSetBits(s_events, WIFI_GOT_IP_BIT);
        xEventGroupClearBits(s_events, WIFI_DISCONNECTED_BIT);
    }
}

static void espnow_send_cb(const uint8_t *mac_addr, esp_now_send_status_t status) {
    (void)mac_addr;
    portENTER_CRITICAL(&s_state_lock);
    s_last_send_status = status;
    s_last_send_valid = true;
    portEXIT_CRITICAL(&s_state_lock);
    xEventGroupSetBits(s_events, ESPNOW_SEND_DONE_BIT);
}

static void espnow_recv_cb(const esp_now_recv_info_t *info, const uint8_t *data, int len) {
    if (info == NULL || data == NULL || len < 0) {
        return;
    }

    int bounded_len = len > ESPNOW_MAX_PAYLOAD ? ESPNOW_MAX_PAYLOAD : len;
    portENTER_CRITICAL(&s_state_lock);
    memcpy(s_last_recv_peer, info->src_addr, sizeof(s_last_recv_peer));
    memcpy(s_last_recv_payload, data, (size_t)bounded_len);
    s_last_recv_len = bounded_len;
    s_last_recv_valid = true;
    portEXIT_CRITICAL(&s_state_lock);
    xEventGroupSetBits(s_events, ESPNOW_RECV_DONE_BIT);
}

static esp_err_t init_runtime(void) {
    esp_err_t err = nvs_flash_init();
    if (err == ESP_ERR_NVS_NO_FREE_PAGES || err == ESP_ERR_NVS_NEW_VERSION_FOUND) {
        ESP_ERROR_CHECK(nvs_flash_erase());
        err = nvs_flash_init();
    }
    ESP_ERROR_CHECK(err);

    ESP_ERROR_CHECK(esp_netif_init());
    err = esp_event_loop_create_default();
    if (err != ESP_OK && err != ESP_ERR_INVALID_STATE) {
        return err;
    }

    s_events = xEventGroupCreate();
    if (s_events == NULL) {
        return ESP_ERR_NO_MEM;
    }

    s_sta_netif = esp_netif_create_default_wifi_sta();
    if (s_sta_netif == NULL) {
        return ESP_ERR_NO_MEM;
    }
    s_ap_netif = esp_netif_create_default_wifi_ap();
    if (s_ap_netif == NULL) {
        return ESP_ERR_NO_MEM;
    }

    wifi_init_config_t config = WIFI_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_wifi_init(&config));
    ESP_ERROR_CHECK(esp_wifi_set_storage(WIFI_STORAGE_RAM));
    ESP_ERROR_CHECK(esp_event_handler_register(WIFI_EVENT, ESP_EVENT_ANY_ID, &wifi_event_handler, NULL));
    ESP_ERROR_CHECK(
        esp_event_handler_register(IP_EVENT, IP_EVENT_STA_GOT_IP, &wifi_event_handler, NULL)
    );

    uart_driver_delete(COMPANION_UART);
    uart_config_t uart_config = {
        .baud_rate = COMPANION_UART_BAUD,
        .data_bits = UART_DATA_8_BITS,
        .parity = UART_PARITY_DISABLE,
        .stop_bits = UART_STOP_BITS_1,
        .flow_ctrl = UART_HW_FLOWCTRL_DISABLE,
        .source_clk = UART_SCLK_APB,
    };
    ESP_ERROR_CHECK(uart_driver_install(COMPANION_UART, 2048, 0, 0, NULL, 0));
    ESP_ERROR_CHECK(uart_param_config(COMPANION_UART, &uart_config));
    ESP_ERROR_CHECK(
        uart_set_pin(COMPANION_UART, UART_PIN_NO_CHANGE, UART_PIN_NO_CHANGE, UART_PIN_NO_CHANGE, UART_PIN_NO_CHANGE)
    );

    return ESP_OK;
}

static esp_err_t reset_wifi_runtime(void) {
    if (s_tcp_serve_listen_fd >= 0) {
        close(s_tcp_serve_listen_fd);
        s_tcp_serve_listen_fd = -1;
    }
    if (s_tcp_serve_client_fd >= 0) {
        close(s_tcp_serve_client_fd);
        s_tcp_serve_client_fd = -1;
    }
    if (s_tcp_serve_task != NULL) {
        TaskHandle_t task = s_tcp_serve_task;
        s_tcp_serve_task = NULL;
        vTaskDelete(task);
    }
    xEventGroupClearBits(s_events, TCP_SERVE_READY_BIT | TCP_SERVE_DONE_BIT);

    if (s_espnow_active) {
        esp_now_unregister_recv_cb();
        esp_now_unregister_send_cb();
        esp_err_t err = esp_now_deinit();
        if (err != ESP_OK && err != ESP_ERR_ESPNOW_NOT_INIT) {
            return err;
        }
        s_espnow_active = false;
    }

    esp_err_t wps_err = stop_wps_ap_registrar();
    if (wps_err != ESP_OK) {
        return wps_err;
    }
    esp_err_t disconnect_err = esp_wifi_disconnect();
    if (disconnect_err != ESP_OK && disconnect_err != ESP_ERR_WIFI_NOT_CONNECT &&
        disconnect_err != ESP_ERR_WIFI_NOT_STARTED && disconnect_err != ESP_ERR_WIFI_MODE &&
        disconnect_err != ESP_FAIL) {
        return disconnect_err;
    }
    esp_err_t stop_err = esp_wifi_stop();
    if (stop_err != ESP_OK && stop_err != ESP_ERR_WIFI_NOT_STARTED &&
        stop_err != ESP_ERR_INVALID_STATE && stop_err != ESP_FAIL) {
        return stop_err;
    }
    s_wifi_started = false;

    esp_err_t deinit_err = esp_wifi_deinit();
    if (deinit_err != ESP_OK && deinit_err != ESP_ERR_WIFI_NOT_INIT && deinit_err != ESP_FAIL) {
        return deinit_err;
    }
    wifi_init_config_t init_config = WIFI_INIT_CONFIG_DEFAULT();
    esp_err_t init_err = esp_wifi_init(&init_config);
    if (init_err != ESP_OK && init_err != ESP_ERR_INVALID_STATE) {
        return init_err;
    }
    esp_err_t storage_err = esp_wifi_set_storage(WIFI_STORAGE_RAM);
    if (storage_err != ESP_OK) {
        return storage_err;
    }

    esp_err_t err = esp_wifi_set_mode(WIFI_MODE_NULL);
    if (err != ESP_OK && err != ESP_ERR_WIFI_NOT_STARTED && err != ESP_ERR_INVALID_STATE &&
        err != ESP_FAIL) {
        return err;
    }
    s_wifi_started = false;
    clear_async_state();
    return ESP_OK;
}

static esp_err_t ensure_sta_dhcp_client(void) {
    if (s_sta_netif == NULL) {
        return ESP_ERR_INVALID_STATE;
    }
    esp_err_t err = esp_netif_dhcpc_start(s_sta_netif);
    return err == ESP_ERR_ESP_NETIF_DHCP_ALREADY_STARTED ? ESP_OK : err;
}

static esp_err_t ensure_wifi_station(void) {
    esp_err_t err = reset_wifi_runtime();
    if (err != ESP_OK) {
        return err;
    }
    err = esp_wifi_set_mode(WIFI_MODE_STA);
    if (err != ESP_OK) {
        return err;
    }
    err = esp_wifi_start();
    if (err != ESP_OK) {
        return err;
    }
    err = esp_wifi_set_ps(WIFI_PS_NONE);
    if (err != ESP_OK) {
        return err;
    }
    err = esp_netif_set_default_netif(s_sta_netif);
    if (err != ESP_OK) {
        return err;
    }
    err = ensure_sta_dhcp_client();
    if (err != ESP_OK) {
        return err;
    }
    s_wifi_started = true;
    return ESP_OK;
}

static esp_err_t ensure_wifi_station_preserve_ap(void) {
    if (!s_wifi_started) {
        return ensure_wifi_station();
    }

    wifi_mode_t mode = WIFI_MODE_NULL;
    esp_err_t err = esp_wifi_get_mode(&mode);
    if (err != ESP_OK) {
        return err;
    }

    if (mode == WIFI_MODE_NULL) {
        return ensure_wifi_station();
    }
    if (mode == WIFI_MODE_AP) {
        err = esp_wifi_set_mode(WIFI_MODE_APSTA);
        if (err != ESP_OK) {
            return err;
        }
    } else if (mode != WIFI_MODE_STA && mode != WIFI_MODE_APSTA) {
        return ensure_wifi_station();
    }

    err = esp_wifi_set_ps(WIFI_PS_NONE);
    if (err != ESP_OK) {
        return err;
    }
    err = esp_netif_set_default_netif(s_sta_netif);
    if (err != ESP_OK) {
        return err;
    }
    err = ensure_sta_dhcp_client();
    if (err != ESP_OK) {
        return err;
    }
    return ESP_OK;
}

static esp_err_t set_softap_mac_for_ssid(const char *ssid) {
    uint8_t mac[6];
    esp_err_t err = esp_read_mac(mac, ESP_MAC_WIFI_STA);
    if (err != ESP_OK) {
        return err;
    }

    uint32_t hash = 2166136261u;
    for (const unsigned char *cursor = (const unsigned char *)ssid; *cursor != 0; cursor++) {
        hash ^= *cursor;
        hash *= 16777619u;
    }

    mac[0] &= 0xFCu;
    mac[5] = (uint8_t)(hash & 0xFEu);
    if (mac[5] == 0) {
        mac[5] = 0x02u;
    }
    return esp_wifi_set_mac(WIFI_IF_AP, mac);
}

static esp_err_t start_softap(const char *ssid, const char *pass, uint8_t channel) {
    if (ssid == NULL || ssid[0] == '\0' || channel == 0 || channel > 14) {
        return ESP_ERR_INVALID_ARG;
    }

    esp_err_t err = reset_wifi_runtime();
    if (err != ESP_OK) {
        return err;
    }

    err = esp_wifi_set_mode(WIFI_MODE_AP);
    if (err != ESP_OK) {
        return err;
    }
    err = set_softap_mac_for_ssid(ssid);
    if (err != ESP_OK) {
        return err;
    }

    wifi_config_t config = {0};
    size_t ssid_len = strlen(ssid);
    if (ssid_len >= sizeof(config.ap.ssid)) {
        return ESP_ERR_INVALID_ARG;
    }
    memcpy(config.ap.ssid, ssid, ssid_len);
    config.ap.ssid_len = ssid_len;
    config.ap.channel = channel;
    config.ap.max_connection = 4;
    config.ap.beacon_interval = 100;

    bool open = pass == NULL || strcmp(pass, "open") == 0 || pass[0] == '\0';
    if (open) {
        config.ap.authmode = WIFI_AUTH_OPEN;
    } else {
        size_t pass_len = strlen(pass);
        if (pass_len < 8 || pass_len >= sizeof(config.ap.password)) {
            return ESP_ERR_INVALID_ARG;
        }
        memcpy(config.ap.password, pass, pass_len);
        config.ap.authmode = WIFI_AUTH_WPA_WPA2_PSK;
    }

    err = esp_wifi_set_config(WIFI_IF_AP, &config);
    if (err != ESP_OK) {
        return err;
    }
    err = esp_wifi_start();
    if (err != ESP_OK) {
        return err;
    }
    err = esp_netif_set_default_netif(s_ap_netif);
    if (err != ESP_OK) {
        return err;
    }
    s_wifi_started = true;
    return ESP_OK;
}

static esp_err_t wait_for_sta_ip(uint32_t timeout_ms) {
    EventBits_t bits = xEventGroupWaitBits(
        s_events,
        WIFI_GOT_IP_BIT | WIFI_DISCONNECTED_BIT,
        pdFALSE,
        pdFALSE,
        pdMS_TO_TICKS(timeout_ms)
    );
    if ((bits & WIFI_GOT_IP_BIT) != 0) {
        return ESP_OK;
    }
    if ((bits & WIFI_DISCONNECTED_BIT) != 0) {
        return ESP_FAIL;
    }
    return ESP_ERR_TIMEOUT;
}

static esp_err_t wait_for_sta_link(uint32_t timeout_ms) {
    EventBits_t bits = xEventGroupWaitBits(
        s_events,
        WIFI_CONNECTED_BIT | WIFI_DISCONNECTED_BIT,
        pdFALSE,
        pdFALSE,
        pdMS_TO_TICKS(timeout_ms)
    );
    if ((bits & WIFI_CONNECTED_BIT) != 0) {
        return ESP_OK;
    }
    if ((bits & WIFI_DISCONNECTED_BIT) != 0) {
        return ESP_FAIL;
    }
    return ESP_ERR_TIMEOUT;
}

static esp_err_t join_station_with_mode(
    const char *ssid,
    const char *pass,
    uint32_t timeout_ms,
    uint8_t channel,
    bool wait_for_ip,
    bool preserve_ap
) {
    if (ssid == NULL || ssid[0] == '\0') {
        return ESP_ERR_INVALID_ARG;
    }

    esp_err_t err = preserve_ap ? ensure_wifi_station_preserve_ap() : ensure_wifi_station();
    if (err != ESP_OK) {
        return err;
    }
    if (preserve_ap) {
        err = esp_wifi_disconnect();
        if (err != ESP_OK && err != ESP_ERR_WIFI_NOT_CONNECT && err != ESP_ERR_WIFI_MODE) {
            return err;
        }
    }

    wifi_config_t config = {0};
    size_t ssid_len = strlen(ssid);
    if (ssid_len >= sizeof(config.sta.ssid)) {
        return ESP_ERR_INVALID_ARG;
    }
    memcpy(config.sta.ssid, ssid, ssid_len);

    bool open = pass == NULL || strcmp(pass, "open") == 0 || pass[0] == '\0';
    if (!open) {
        size_t pass_len = strlen(pass);
        if (pass_len >= sizeof(config.sta.password)) {
            return ESP_ERR_INVALID_ARG;
        }
        memcpy(config.sta.password, pass, pass_len);
        config.sta.threshold.authmode = WIFI_AUTH_WPA2_PSK;
    } else {
        config.sta.threshold.authmode = WIFI_AUTH_OPEN;
    }
    config.sta.channel = channel;
    config.sta.pmf_cfg.capable = true;
    config.sta.pmf_cfg.required = false;
    config.sta.scan_method = channel == 0 ? WIFI_ALL_CHANNEL_SCAN : WIFI_FAST_SCAN;

    clear_async_state();
    err = esp_wifi_set_config(WIFI_IF_STA, &config);
    if (err != ESP_OK) {
        return err;
    }
    err = esp_wifi_connect();
    if (err != ESP_OK) {
        return err;
    }
    return wait_for_ip ? wait_for_sta_ip(timeout_ms) : wait_for_sta_link(timeout_ms);
}

static esp_err_t join_station(
    const char *ssid,
    const char *pass,
    uint32_t timeout_ms,
    bool wait_for_ip
) {
    return join_station_with_mode(ssid, pass, timeout_ms, 0, wait_for_ip, false);
}

static esp_err_t udp_send_packets(esp_ip4_addr_t host, uint16_t port, uint32_t count, uint32_t size) {
    if (size == 0 || size > UDP_BUFFER_SIZE) {
        return ESP_ERR_INVALID_SIZE;
    }

    int fd = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    if (fd < 0) {
        return ESP_FAIL;
    }

    struct sockaddr_in remote = {
        .sin_family = AF_INET,
        .sin_port = htons(port),
        .sin_addr.s_addr = host.addr,
    };

    uint8_t payload[UDP_BUFFER_SIZE];
    for (uint32_t index = 0; index < size; index++) {
        payload[index] = (uint8_t)(index & 0xffu);
    }

    esp_err_t result = ESP_OK;
    for (uint32_t packet_index = 0; packet_index < count; packet_index++) {
        payload[0] = (uint8_t)packet_index;
        int sent = sendto(fd, payload, size, 0, (struct sockaddr *)&remote, sizeof(remote));
        if (sent != (int)size) {
            result = ESP_FAIL;
            break;
        }
        vTaskDelay(pdMS_TO_TICKS(20));
    }

    close(fd);
    return result;
}

static esp_err_t bind_socket_to_netif(int fd, esp_netif_t *netif);
static size_t build_raw80211_data(
    uint8_t *frame,
    size_t capacity,
    const uint8_t mac[6],
    const uint8_t destination[6],
    size_t frame_len
);

static uint8_t esptouch_crc8_update(uint8_t crc, const uint8_t *data, size_t len) {
    for (size_t index = 0; index < len; index++) {
        uint8_t remainder = data[index] ^ crc;
        for (uint8_t bit = 0; bit < 8; bit++) {
            if ((remainder & 0x01u) != 0) {
                remainder = (uint8_t)((remainder >> 1) ^ 0x8cu);
            } else {
                remainder >>= 1;
            }
        }
        crc = remainder;
    }
    return crc;
}

static uint8_t esptouch_crc8_byte_pair(uint8_t value, uint8_t index) {
    uint8_t crc = esptouch_crc8_update(0, &value, 1);
    return esptouch_crc8_update(crc, &index, 1);
}

static uint16_t esptouch_encoded_pair(uint8_t high, uint8_t low) {
    return (uint16_t)(((uint16_t)high << 8) | low) + ESPTOUCH_EXTRA_LEN;
}

static size_t esptouch_append_data_code(
    uint16_t *lengths,
    size_t length_capacity,
    uint8_t value,
    uint8_t index
) {
    if (length_capacity < 3) {
        return 0;
    }

    uint8_t data_high = value >> 4;
    uint8_t data_low = value & 0x0fu;
    uint8_t crc = esptouch_crc8_byte_pair(value, index);
    uint8_t crc_high = crc >> 4;
    uint8_t crc_low = crc & 0x0fu;

    lengths[0] = esptouch_encoded_pair(0x00, (uint8_t)((crc_high << 4) | data_high));
    lengths[1] = esptouch_encoded_pair(0x01, index);
    lengths[2] = esptouch_encoded_pair(0x00, (uint8_t)((crc_low << 4) | data_low));
    return 3;
}

typedef struct {
    uint8_t value;
    uint8_t index;
} esptouch_code_t;

static bool esptouch_insert_code(
    esptouch_code_t *codes,
    size_t *count,
    size_t capacity,
    size_t position,
    esptouch_code_t code
) {
    if (*count >= capacity || position > *count) {
        return false;
    }
    memmove(&codes[position + 1], &codes[position], (*count - position) * sizeof(codes[0]));
    codes[position] = code;
    *count += 1;
    return true;
}

static bool esptouch_build_lengths(
    const char *ssid,
    const char *pass,
    const uint8_t bssid[6],
    esp_ip4_addr_t phone_ip,
    uint16_t *lengths,
    size_t length_capacity,
    size_t *length_count
) {
    const uint8_t *ssid_bytes = (const uint8_t *)ssid;
    const uint8_t *pass_bytes = (const uint8_t *)pass;
    size_t ssid_len = strlen(ssid);
    size_t pass_len = pass == NULL || strcmp(pass, "open") == 0 ? 0 : strlen(pass);
    if (ssid_len == 0 || ssid_len > 32 || pass_len > 64) {
        return false;
    }

    uint8_t ip_bytes[4];
    memcpy(ip_bytes, &phone_ip.addr, sizeof(ip_bytes));
    uint8_t total_len = (uint8_t)(ESPTOUCH_EXTRA_HEAD_LEN + sizeof(ip_bytes) + pass_len + ssid_len);
    uint8_t total_xor = 0;
    esptouch_code_t codes[ESPTOUCH_MAX_DATA_CODES];
    size_t code_count = 0;

#define ESPTOUCH_PUSH(value_, index_)                                            \
    do {                                                                         \
        if (code_count >= ESPTOUCH_MAX_DATA_CODES) {                             \
            return false;                                                        \
        }                                                                        \
        codes[code_count++] = (esptouch_code_t){(uint8_t)(value_), (uint8_t)(index_)}; \
        total_xor ^= (uint8_t)(value_);                                          \
    } while (0)

    ESPTOUCH_PUSH(total_len, 0);
    ESPTOUCH_PUSH(pass_len, 1);
    ESPTOUCH_PUSH(esptouch_crc8_update(0, ssid_bytes, ssid_len), 2);
    ESPTOUCH_PUSH(esptouch_crc8_update(0, bssid, 6), 3);
    for (size_t index = 0; index < sizeof(ip_bytes); index++) {
        ESPTOUCH_PUSH(ip_bytes[index], index + ESPTOUCH_EXTRA_HEAD_LEN);
    }
    for (size_t index = 0; index < pass_len; index++) {
        ESPTOUCH_PUSH(pass_bytes[index], index + ESPTOUCH_EXTRA_HEAD_LEN + sizeof(ip_bytes));
    }
    for (size_t index = 0; index < ssid_len; index++) {
        ESPTOUCH_PUSH(
            ssid_bytes[index],
            index + ESPTOUCH_EXTRA_HEAD_LEN + sizeof(ip_bytes) + pass_len
        );
    }

#undef ESPTOUCH_PUSH

    if (!esptouch_insert_code(
            codes,
            &code_count,
            ESPTOUCH_MAX_DATA_CODES,
            4,
            (esptouch_code_t){total_xor, 4}
        )) {
        return false;
    }

    size_t bssid_insert_index = ESPTOUCH_EXTRA_HEAD_LEN;
    for (size_t index = 0; index < 6; index++) {
        size_t position = bssid_insert_index + 1;
        if (position > code_count) {
            position = code_count;
        }
        if (!esptouch_insert_code(
                codes,
                &code_count,
                ESPTOUCH_MAX_DATA_CODES,
                position,
                (esptouch_code_t){bssid[index], (uint8_t)(total_len + index)}
            )) {
            return false;
        }
        bssid_insert_index += 4;
    }

    size_t out_count = 0;
    for (size_t index = 0; index < code_count; index++) {
        size_t written = esptouch_append_data_code(
            &lengths[out_count],
            length_capacity - out_count,
            codes[index].value,
            codes[index].index
        );
        if (written == 0) {
            return false;
        }
        out_count += written;
    }

    *length_count = out_count;
    return true;
}

static esp_err_t raw80211_tx_with_retry(wifi_interface_t interface, const uint8_t *frame, size_t frame_len) {
    for (uint32_t attempt = 0; attempt < 16; attempt++) {
        esp_err_t err = esp_wifi_80211_tx(interface, frame, (int)frame_len, true);
        if (err == ESP_OK) {
            return ESP_OK;
        }
        if (err != ESP_ERR_NO_MEM) {
            return err;
        }
        vTaskDelay(pdMS_TO_TICKS(20));
    }
    return ESP_ERR_NO_MEM;
}

static esp_err_t send_udp_payload_len(int fd, const struct sockaddr_in *remote, uint16_t len) {
    static uint8_t payload[UDP_BUFFER_SIZE];
    if (len == 0 || len > sizeof(payload)) {
        return ESP_ERR_INVALID_SIZE;
    }
    memset(payload, 0x31, len);
    int sent = sendto(fd, payload, len, 0, (const struct sockaddr *)remote, sizeof(*remote));
    return sent == (int)len ? ESP_OK : ESP_FAIL;
}

static esp_err_t smartconfig_fail(const char *step, esp_err_t err) {
    s_last_smartconfig_step = step;
    s_last_smartconfig_errno = errno;
    return err;
}

static esp_err_t smartconfig_ap_provision(
    const char *ssid,
    const char *pass,
    uint8_t channel,
    uint32_t timeout_ms
) {
    s_last_smartconfig_step = "start";
    s_last_smartconfig_errno = 0;
    esp_err_t err = start_softap(ssid, pass, channel);
    if (err != ESP_OK) {
        return smartconfig_fail("start-softap", err);
    }

    uint8_t bssid[6];
    err = esp_wifi_get_mac(WIFI_IF_AP, bssid);
    if (err != ESP_OK) {
        return smartconfig_fail("ap-mac", err);
    }

    esp_ip4_addr_t phone_ip;
    if (!parse_ipv4("192.168.4.1", &phone_ip)) {
        return smartconfig_fail("phone-ip", ESP_ERR_INVALID_ARG);
    }

    uint16_t data_lengths[ESPTOUCH_MAX_LENGTH_CODES];
    size_t data_length_count = 0;
    if (!esptouch_build_lengths(
            ssid,
            pass,
            bssid,
            phone_ip,
            data_lengths,
            sizeof(data_lengths) / sizeof(data_lengths[0]),
            &data_length_count
        )) {
        return smartconfig_fail("build-lengths", ESP_ERR_INVALID_SIZE);
    }

    int tx_fd = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    if (tx_fd < 0) {
        smartconfig_fail("tx-socket", ESP_FAIL);
        return ESP_FAIL;
    }
    int rx_fd = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    if (rx_fd < 0) {
        close(tx_fd);
        smartconfig_fail("rx-socket", ESP_FAIL);
        return ESP_FAIL;
    }

    int enabled = 1;
    (void)setsockopt(tx_fd, SOL_SOCKET, SO_BROADCAST, &enabled, sizeof(enabled));
    (void)setsockopt(rx_fd, SOL_SOCKET, SO_REUSEADDR, &enabled, sizeof(enabled));
    err = bind_socket_to_netif(tx_fd, s_ap_netif);
    if (err != ESP_OK) {
        close(rx_fd);
        close(tx_fd);
        return smartconfig_fail("bind-tx-netif", err);
    }

    struct timeval rx_timeout = {
        .tv_sec = 0,
        .tv_usec = 20 * 1000,
    };
    (void)setsockopt(rx_fd, SOL_SOCKET, SO_RCVTIMEO, &rx_timeout, sizeof(rx_timeout));

    struct sockaddr_in ack_local = {
        .sin_family = AF_INET,
        .sin_port = htons(ESPTOUCH_ACK_PORT),
        .sin_addr.s_addr = htonl(INADDR_ANY),
    };
    if (bind(rx_fd, (struct sockaddr *)&ack_local, sizeof(ack_local)) != 0) {
        smartconfig_fail("bind-ack", ESP_FAIL);
        close(rx_fd);
        close(tx_fd);
        return ESP_FAIL;
    }

    struct sockaddr_in remote = {
        .sin_family = AF_INET,
        .sin_port = htons(ESPTOUCH_TARGET_PORT),
        .sin_addr.s_addr = htonl(INADDR_BROADCAST),
    };

    TickType_t start = xTaskGetTickCount();
    TickType_t timeout_ticks = pdMS_TO_TICKS(timeout_ms);
    size_t data_index = 0;
    uint8_t ack[32];
    while ((xTaskGetTickCount() - start) < timeout_ticks) {
        uint32_t elapsed_ms = (uint32_t)((xTaskGetTickCount() - start) * portTICK_PERIOD_MS);
        if ((elapsed_ms % ESPTOUCH_TOTAL_PERIOD_MS) < ESPTOUCH_GUIDE_MS) {
            const uint16_t guide_lengths[] = {515, 514, 513, 512};
            for (size_t index = 0; index < sizeof(guide_lengths) / sizeof(guide_lengths[0]); index++) {
                err = send_udp_payload_len(tx_fd, &remote, guide_lengths[index]);
                if (err != ESP_OK) {
                    close(rx_fd);
                    close(tx_fd);
                    smartconfig_fail("send-guide", err);
                    return err;
                }
                vTaskDelay(pdMS_TO_TICKS(ESPTOUCH_INTERVAL_MS));
            }
        } else {
            for (size_t sent = 0; sent < 3; sent++) {
                err = send_udp_payload_len(tx_fd, &remote, data_lengths[data_index]);
                if (err != ESP_OK) {
                    close(rx_fd);
                    close(tx_fd);
                    smartconfig_fail("send-data", err);
                    return err;
                }
                data_index = (data_index + 1) % data_length_count;
                vTaskDelay(pdMS_TO_TICKS(ESPTOUCH_INTERVAL_MS));
            }
        }

        struct sockaddr_in peer;
        socklen_t peer_len = sizeof(peer);
        int received = recvfrom(rx_fd, ack, sizeof(ack), 0, (struct sockaddr *)&peer, &peer_len);
        if (received > 0) {
            close(rx_fd);
            close(tx_fd);
            return ESP_OK;
        }
    }

    close(rx_fd);
    close(tx_fd);
    return smartconfig_fail("ack-timeout", ESP_ERR_TIMEOUT);
}

static size_t build_raw80211_beacon(uint8_t *frame, size_t capacity, const uint8_t mac[6], uint8_t channel) {
    static const uint8_t broadcast[6] = {0xff, 0xff, 0xff, 0xff, 0xff, 0xff};
    static const char ssid[] = "esp8266-hal-promisc";
    static const uint8_t supported_rates[] = {0x82, 0x84, 0x8b, 0x96};
    size_t offset = 0;
    size_t ssid_len = sizeof(ssid) - 1;
    size_t needed = 24 + 8 + 2 + 2 + 2 + ssid_len + 2 + sizeof(supported_rates) + 3;
    if (capacity < needed || ssid_len > 32) {
        return 0;
    }

    memset(frame, 0, needed);
    frame[offset++] = 0x80;
    frame[offset++] = 0x00;
    offset += 2;
    memcpy(&frame[offset], broadcast, sizeof(broadcast));
    offset += sizeof(broadcast);
    memcpy(&frame[offset], mac, 6);
    offset += 6;
    memcpy(&frame[offset], mac, 6);
    offset += 6;
    offset += 2;
    offset += 8;
    frame[offset++] = 100;
    frame[offset++] = 0;
    frame[offset++] = 0x01;
    frame[offset++] = 0x00;
    frame[offset++] = 0;
    frame[offset++] = (uint8_t)ssid_len;
    memcpy(&frame[offset], ssid, ssid_len);
    offset += ssid_len;
    frame[offset++] = 1;
    frame[offset++] = sizeof(supported_rates);
    memcpy(&frame[offset], supported_rates, sizeof(supported_rates));
    offset += sizeof(supported_rates);
    frame[offset++] = 3;
    frame[offset++] = 1;
    frame[offset++] = channel;
    return offset;
}

static size_t build_raw80211_probe_request(
    uint8_t *frame,
    size_t capacity,
    const uint8_t mac[6],
    uint8_t channel
) {
    static const uint8_t broadcast[6] = {0xff, 0xff, 0xff, 0xff, 0xff, 0xff};
    static const uint8_t supported_rates[] = {0x82, 0x84, 0x8b, 0x96};
    size_t offset = 0;
    size_t needed = 24 + 2 + 2 + sizeof(supported_rates) + 3;
    if (capacity < needed) {
        return 0;
    }

    memset(frame, 0, needed);
    frame[offset++] = 0x40;
    frame[offset++] = 0x00;
    offset += 2;
    memcpy(&frame[offset], broadcast, sizeof(broadcast));
    offset += sizeof(broadcast);
    memcpy(&frame[offset], mac, 6);
    offset += 6;
    memcpy(&frame[offset], broadcast, sizeof(broadcast));
    offset += sizeof(broadcast);
    offset += 2;
    frame[offset++] = 0;
    frame[offset++] = 0;
    frame[offset++] = 1;
    frame[offset++] = sizeof(supported_rates);
    memcpy(&frame[offset], supported_rates, sizeof(supported_rates));
    offset += sizeof(supported_rates);
    frame[offset++] = 3;
    frame[offset++] = 1;
    frame[offset++] = channel;
    return offset;
}

static size_t build_raw80211_data(
    uint8_t *frame,
    size_t capacity,
    const uint8_t mac[6],
    const uint8_t destination[6],
    size_t frame_len
) {
    static const uint8_t llc_snap_ipv4[] = {0xaa, 0xaa, 0x03, 0x00, 0x00, 0x00, 0x08, 0x00};
    size_t header_len = 24 + sizeof(llc_snap_ipv4);
    if (frame_len < header_len || capacity < frame_len) {
        return 0;
    }

    memset(frame, 0, frame_len);
    size_t offset = 0;
    frame[offset++] = 0x08;
    frame[offset++] = 0x02;
    offset += 2;
    memcpy(&frame[offset], destination, 6);
    offset += 6;
    memcpy(&frame[offset], mac, 6);
    offset += 6;
    memcpy(&frame[offset], mac, 6);
    offset += 6;
    offset += 2;
    memcpy(&frame[offset], llc_snap_ipv4, sizeof(llc_snap_ipv4));
    offset += sizeof(llc_snap_ipv4);
    for (size_t index = 0; offset < frame_len; index++) {
        frame[offset++] = (uint8_t)index;
    }
    return frame_len;
}

static esp_err_t send_raw80211_frames(
    const char *kind,
    uint8_t channel,
    const uint8_t destination[6],
    uint32_t count,
    uint32_t interval_ms
) {
    if (kind == NULL || channel == 0 || channel > 14 || count == 0) {
        return ESP_ERR_INVALID_ARG;
    }

    esp_err_t err = ESP_OK;
    wifi_interface_t interface = WIFI_IF_AP;
    uint8_t mac[6];
    uint8_t frame[RAW80211_MAX_FRAME_SIZE];
    size_t frame_len;
    if (strcmp(kind, "management") == 0) {
        esp_err_t err = ensure_wifi_station();
        if (err != ESP_OK) {
            return err;
        }
        err = esp_wifi_set_channel(channel, WIFI_SECOND_CHAN_NONE);
        if (err != ESP_OK) {
            return err;
        }
        err = esp_wifi_get_mac(WIFI_IF_STA, mac);
        if (err != ESP_OK) {
            return err;
        }
        interface = WIFI_IF_STA;
        frame_len = build_raw80211_probe_request(frame, sizeof(frame), mac, channel);
    } else if (strcmp(kind, "beacon") == 0) {
        esp_err_t err = start_softap("esp8266-hal-promisc", "open", channel);
        if (err != ESP_OK) {
            return err;
        }
        err = esp_wifi_get_mac(WIFI_IF_AP, mac);
        if (err != ESP_OK) {
            return err;
        }
        frame_len = build_raw80211_beacon(frame, sizeof(frame), mac, channel);
    } else if (strcmp(kind, "data") == 0) {
        esp_err_t err = start_softap("esp8266-hal-promisc", "open", channel);
        if (err != ESP_OK) {
            return err;
        }
        err = esp_wifi_get_mac(WIFI_IF_AP, mac);
        if (err != ESP_OK) {
            return err;
        }
        frame_len = build_raw80211_data(
            frame,
            sizeof(frame),
            mac,
            destination,
            24 + 8 + RAW80211_DEFAULT_DATA_PAYLOAD_SIZE
        );
    } else {
        return ESP_ERR_INVALID_ARG;
    }
    if (frame_len == 0) {
        return ESP_ERR_INVALID_SIZE;
    }

    for (uint32_t index = 0; index < count; index++) {
        err = raw80211_tx_with_retry(interface, frame, frame_len);
        if (err != ESP_OK) {
            return err;
        }
        if (interval_ms != 0) {
            vTaskDelay(pdMS_TO_TICKS(interval_ms));
        }
    }

    return ESP_OK;
}

static bool dhcp_push_option(
    uint8_t *packet,
    size_t packet_len,
    size_t *offset,
    uint8_t kind,
    const uint8_t *value,
    uint8_t value_len
) {
    if (*offset + 2u + value_len > packet_len) {
        return false;
    }
    packet[*offset] = kind;
    packet[*offset + 1u] = value_len;
    memcpy(packet + *offset + 2u, value, value_len);
    *offset += 2u + value_len;
    return true;
}

static esp_err_t send_dhcp_request_probe(esp_ip4_addr_t server, esp_ip4_addr_t requested) {
    uint8_t mac[6];
    esp_err_t err = esp_wifi_get_mac(WIFI_IF_STA, mac);
    if (err != ESP_OK) {
        return err;
    }

    int fd = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    if (fd < 0) {
        return ESP_FAIL;
    }

    int enabled = 1;
    (void)setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &enabled, sizeof(enabled));
    (void)setsockopt(fd, SOL_SOCKET, SO_BROADCAST, &enabled, sizeof(enabled));

    err = bind_socket_to_netif(fd, s_sta_netif);
    if (err != ESP_OK) {
        close(fd);
        return err;
    }

    struct sockaddr_in local = {
        .sin_family = AF_INET,
        .sin_port = htons(DHCP_CLIENT_PORT),
        .sin_addr.s_addr = htonl(INADDR_ANY),
    };
    if (bind(fd, (struct sockaddr *)&local, sizeof(local)) != 0) {
        close(fd);
        return ESP_FAIL;
    }

    uint8_t packet[DHCP_PACKET_SIZE];
    memset(packet, 0, sizeof(packet));
    packet[0] = 1;
    packet[1] = 1;
    packet[2] = 6;
    packet[4] = 0x82;
    packet[5] = 0x82;
    packet[6] = 0x32;
    packet[7] = 0x80;
    packet[10] = 0x80;
    memcpy(packet + 28, mac, sizeof(mac));
    packet[236] = 0x63;
    packet[237] = 0x82;
    packet[238] = 0x53;
    packet[239] = 0x63;

    size_t offset = 240;
    uint8_t message_type = 3;
    uint8_t requested_params[] = {1, 3, 6, 51, 54, 58, 59};
    bool ok = dhcp_push_option(
                  packet,
                  sizeof(packet),
                  &offset,
                  DHCP_OPT_MESSAGE_TYPE,
                  &message_type,
                  sizeof(message_type)
              ) &&
              dhcp_push_option(
                  packet,
                  sizeof(packet),
                  &offset,
                  DHCP_OPT_REQUESTED_IP,
                  (const uint8_t *)&requested.addr,
                  sizeof(requested.addr)
              ) &&
              dhcp_push_option(
                  packet,
                  sizeof(packet),
                  &offset,
                  DHCP_OPT_SERVER_IDENTIFIER,
                  (const uint8_t *)&server.addr,
                  sizeof(server.addr)
              ) &&
              dhcp_push_option(
                  packet,
                  sizeof(packet),
                  &offset,
                  DHCP_OPT_PARAMETER_REQUEST_LIST,
                  requested_params,
                  sizeof(requested_params)
              );
    if (!ok || offset >= sizeof(packet)) {
        close(fd);
        return ESP_ERR_INVALID_SIZE;
    }
    packet[offset++] = DHCP_OPT_END;

    struct sockaddr_in remote = {
        .sin_family = AF_INET,
        .sin_port = htons(DHCP_SERVER_PORT),
        .sin_addr.s_addr = htonl(INADDR_BROADCAST),
    };
    int sent = sendto(fd, packet, offset, 0, (struct sockaddr *)&remote, sizeof(remote));
    close(fd);
    return sent == (int)offset ? ESP_OK : ESP_FAIL;
}

static void handle_sta_set_static(const arg_pair_t *args, size_t arg_count) {
    const char *ip_text = find_arg(args, arg_count, "ip");
    const char *gw_text = find_arg(args, arg_count, "gw");
    const char *mask_text = find_arg(args, arg_count, "mask");
    esp_netif_ip_info_t info = {0};

    if (!parse_ipv4(ip_text, &info.ip) || !parse_ipv4(gw_text, &info.gw) || !parse_ipv4(mask_text, &info.netmask)) {
        reply_err("sta-set-static", "invalid-ip-info");
        return;
    }
    if (s_sta_netif == NULL) {
        reply_err("sta-set-static", "no-netif");
        return;
    }

    esp_err_t err = esp_netif_dhcpc_stop(s_sta_netif);
    if (err != ESP_OK && err != ESP_ERR_ESP_NETIF_DHCP_ALREADY_STOPPED) {
        reply_err("sta-set-static", "%s", esp_err_to_name(err));
        return;
    }

    err = esp_netif_set_ip_info(s_sta_netif, &info);
    if (err != ESP_OK) {
        reply_err("sta-set-static", "%s", esp_err_to_name(err));
        return;
    }

    portENTER_CRITICAL(&s_state_lock);
    s_last_ipv4 = info.ip;
    portEXIT_CRITICAL(&s_state_lock);
    xEventGroupSetBits(s_events, WIFI_GOT_IP_BIT);

    char text[16];
    format_ipv4(info.ip, text, sizeof(text));
    reply_ok("STA_SET_STATIC ip=%s", text);
}

static esp_err_t bind_socket_to_netif(int fd, esp_netif_t *netif) {
    if (netif == NULL) {
        return ESP_ERR_INVALID_ARG;
    }

    char if_name[6] = {0};
    esp_err_t err = esp_netif_get_netif_impl_name(netif, if_name);
    if (err != ESP_OK) {
        return err;
    }

    struct ifreq iface = {0};
    strncpy(iface.ifr_name, if_name, sizeof(iface.ifr_name) - 1);
    if (setsockopt(fd, SOL_SOCKET, SO_BINDTODEVICE, &iface, sizeof(iface)) != 0) {
        s_last_tcp_errno = errno;
        return ESP_FAIL;
    }
    return ESP_OK;
}

static esp_err_t connect_with_timeout(int fd, const struct sockaddr_in *remote, uint32_t timeout_ms) {
    int flags = fcntl(fd, F_GETFL, 0);
    if (flags < 0) {
        s_last_tcp_errno = errno;
        return ESP_FAIL;
    }
    if (fcntl(fd, F_SETFL, flags | O_NONBLOCK) != 0) {
        s_last_tcp_errno = errno;
        return ESP_FAIL;
    }

    if (connect(fd, (const struct sockaddr *)remote, sizeof(*remote)) == 0) {
        (void)fcntl(fd, F_SETFL, flags);
        return ESP_OK;
    }
    if (errno != EINPROGRESS && errno != EALREADY) {
        s_last_tcp_errno = errno;
        (void)fcntl(fd, F_SETFL, flags);
        return ESP_FAIL;
    }

    fd_set write_fds;
    FD_ZERO(&write_fds);
    FD_SET(fd, &write_fds);
    struct timeval timeout = {
        .tv_sec = (time_t)(timeout_ms / 1000),
        .tv_usec = (suseconds_t)((timeout_ms % 1000) * 1000),
    };
    int ready = select(fd + 1, NULL, &write_fds, NULL, &timeout);
    if (ready == 0) {
        s_last_tcp_errno = ETIMEDOUT;
        (void)fcntl(fd, F_SETFL, flags);
        return ESP_FAIL;
    }
    if (ready < 0) {
        s_last_tcp_errno = errno;
        (void)fcntl(fd, F_SETFL, flags);
        return ESP_FAIL;
    }

    int socket_error = 0;
    socklen_t socket_error_len = sizeof(socket_error);
    if (getsockopt(fd, SOL_SOCKET, SO_ERROR, &socket_error, &socket_error_len) != 0) {
        s_last_tcp_errno = errno;
        (void)fcntl(fd, F_SETFL, flags);
        return ESP_FAIL;
    }
    if (socket_error != 0) {
        s_last_tcp_errno = socket_error;
        (void)fcntl(fd, F_SETFL, flags);
        return ESP_FAIL;
    }

    if (fcntl(fd, F_SETFL, flags) != 0) {
        s_last_tcp_errno = errno;
        return ESP_FAIL;
    }
    return ESP_OK;
}

static esp_err_t set_socket_io_timeout(int fd, uint32_t timeout_ms) {
    struct timeval timeout = {
        .tv_sec = (time_t)(timeout_ms / 1000),
        .tv_usec = (suseconds_t)((timeout_ms % 1000) * 1000),
    };
    if (setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout)) != 0) {
        s_last_tcp_errno = errno;
        return ESP_FAIL;
    }
    if (setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)) != 0) {
        s_last_tcp_errno = errno;
        return ESP_FAIL;
    }
    return ESP_OK;
}

static esp_err_t send_all(int fd, const uint8_t *payload, size_t payload_len) {
    size_t offset = 0;
    while (offset < payload_len) {
        int sent = send(fd, payload + offset, payload_len - offset, 0);
        if (sent <= 0) {
            s_last_tcp_errno = errno;
            return ESP_FAIL;
        }
        offset += (size_t)sent;
    }
    return ESP_OK;
}

typedef struct {
    esp_ip4_addr_t ip;
    uint8_t mac[6];
    err_t result;
} static_arp_ctx_t;

static esp_err_t add_static_arp_entry(void *arg) {
    static_arp_ctx_t *ctx = arg;
    ip4_addr_t ip = {
        .addr = ctx->ip.addr,
    };
    struct eth_addr eth = {0};
    memcpy(eth.addr, ctx->mac, sizeof(eth.addr));

    ctx->result = etharp_add_static_entry(&ip, &eth);
    return ctx->result == ERR_OK ? ESP_OK : ESP_FAIL;
}

static esp_err_t seed_ap_station_arp(esp_ip4_addr_t ip) {
    s_last_tcp_arp_result = 0;
    s_last_tcp_sta_count = 0;

    wifi_sta_list_t stations = {0};
    esp_err_t err = esp_wifi_ap_get_sta_list(&stations);
    if (err != ESP_OK) {
        return err;
    }
    s_last_tcp_sta_count = stations.num;
    if (stations.num == 0) {
        return ESP_ERR_NOT_FOUND;
    }

    static_arp_ctx_t ctx = {
        .ip = ip,
        .result = ERR_OK,
    };
    memcpy(ctx.mac, stations.sta[0].mac, sizeof(ctx.mac));

    err = esp_netif_set_default_netif(s_ap_netif);
    if (err != ESP_OK) {
        return err;
    }
    err = esp_netif_tcpip_exec(add_static_arp_entry, &ctx);
    s_last_tcp_arp_result = ctx.result;
    return err;
}

static esp_err_t tcp_round_trip(
    esp_ip4_addr_t host,
    uint16_t port,
    const uint8_t *payload,
    size_t payload_len,
    esp_netif_t *bind_netif,
    uint32_t timeout_ms
) {
    s_last_tcp_errno = 0;
    s_last_tcp_stage = "socket";
    int fd = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (fd < 0) {
        s_last_tcp_errno = errno;
        return ESP_FAIL;
    }

    if (bind_netif != NULL) {
        s_last_tcp_stage = "bind";
        esp_err_t err = bind_socket_to_netif(fd, bind_netif);
        if (err != ESP_OK) {
            close(fd);
            return err;
        }
    }

    struct sockaddr_in remote = {
        .sin_family = AF_INET,
        .sin_port = htons(port),
        .sin_addr.s_addr = host.addr,
    };

    s_last_tcp_stage = "connect";
    esp_err_t err = connect_with_timeout(fd, &remote, timeout_ms);
    if (err != ESP_OK) {
        close(fd);
        return err;
    }

    s_last_tcp_stage = "timeout";
    err = set_socket_io_timeout(fd, timeout_ms);
    if (err != ESP_OK) {
        close(fd);
        return err;
    }

    if (payload_len > 0) {
        s_last_tcp_stage = "send";
        err = send_all(fd, payload, payload_len);
        if (err != ESP_OK) {
            close(fd);
            return err;
        }
    }

    s_last_tcp_stage = "recv";
    uint8_t rx[256];
    int received = recv(fd, rx, sizeof(rx), 0);
    close(fd);
    if (received < 0) {
        s_last_tcp_errno = errno;
        return ESP_FAIL;
    }
    if (received == 0) {
        s_last_tcp_errno = 0;
        s_last_tcp_stage = "recv-eof";
        return ESP_FAIL;
    }

    size_t hex_len = (size_t)received * 2 + 1;
    char *hex = malloc(hex_len);
    if (hex == NULL) {
        s_last_tcp_errno = ENOMEM;
        s_last_tcp_stage = "recv-format";
        return ESP_ERR_NO_MEM;
    }
    hex_encode(rx, (size_t)received, hex, hex_len);
    reply_ok("TCP_CONNECT recv=%s", hex);
    free(hex);
    return ESP_OK;
}

typedef struct {
    uint16_t port;
    size_t response_len;
    uint8_t response[256];
} tcp_serve_once_config_t;

static void tcp_serve_once_task(void *param) {
    tcp_serve_once_config_t *config = param;
    int listen_fd = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (listen_fd < 0) {
        s_tcp_serve_status = ESP_FAIL;
        xEventGroupSetBits(s_events, TCP_SERVE_DONE_BIT);
        write_line("ERR ", "code=tcp-serve detail=socket errno=%d", errno);
        free(config);
        s_tcp_serve_task = NULL;
        vTaskDelete(NULL);
        return;
    }
    s_tcp_serve_listen_fd = listen_fd;

    int reuse = 1;
    (void)setsockopt(listen_fd, SOL_SOCKET, SO_REUSEADDR, &reuse, sizeof(reuse));
    if (bind_socket_to_netif(listen_fd, s_ap_netif) != ESP_OK) {
        s_tcp_serve_status = ESP_FAIL;
        xEventGroupSetBits(s_events, TCP_SERVE_DONE_BIT);
        write_line("ERR ", "code=tcp-serve detail=bind-if errno=%d", s_last_tcp_errno);
        close(listen_fd);
        s_tcp_serve_listen_fd = -1;
        free(config);
        s_tcp_serve_task = NULL;
        vTaskDelete(NULL);
        return;
    }

    struct sockaddr_in local = {
        .sin_family = AF_INET,
        .sin_port = htons(config->port),
        .sin_addr.s_addr = htonl(INADDR_ANY),
    };
    if (bind(listen_fd, (struct sockaddr *)&local, sizeof(local)) != 0) {
        s_tcp_serve_status = ESP_FAIL;
        xEventGroupSetBits(s_events, TCP_SERVE_DONE_BIT);
        write_line("ERR ", "code=tcp-serve detail=bind errno=%d", errno);
        close(listen_fd);
        s_tcp_serve_listen_fd = -1;
        free(config);
        s_tcp_serve_task = NULL;
        vTaskDelete(NULL);
        return;
    }
    if (listen(listen_fd, 1) != 0) {
        s_tcp_serve_status = ESP_FAIL;
        xEventGroupSetBits(s_events, TCP_SERVE_DONE_BIT);
        write_line("ERR ", "code=tcp-serve detail=listen errno=%d", errno);
        close(listen_fd);
        s_tcp_serve_listen_fd = -1;
        free(config);
        s_tcp_serve_task = NULL;
        vTaskDelete(NULL);
        return;
    }
    xEventGroupSetBits(s_events, TCP_SERVE_READY_BIT);

    struct timeval timeout = {
        .tv_sec = 120,
        .tv_usec = 0,
    };
    (void)setsockopt(listen_fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));

    int client_fd = accept(listen_fd, NULL, NULL);
    if (client_fd < 0) {
        s_tcp_serve_status = ESP_FAIL;
        xEventGroupSetBits(s_events, TCP_SERVE_DONE_BIT);
        write_line("ERR ", "code=tcp-serve detail=accept errno=%d", errno);
        close(listen_fd);
        s_tcp_serve_listen_fd = -1;
        free(config);
        s_tcp_serve_task = NULL;
        vTaskDelete(NULL);
        return;
    }
    s_tcp_serve_client_fd = client_fd;

    uint8_t request[256];
    int received = recv(client_fd, request, sizeof(request), 0);
    if (received < 0) {
        s_tcp_serve_status = ESP_FAIL;
        xEventGroupSetBits(s_events, TCP_SERVE_DONE_BIT);
        write_line("ERR ", "code=tcp-serve detail=recv errno=%d", errno);
        close(client_fd);
        s_tcp_serve_client_fd = -1;
        close(listen_fd);
        s_tcp_serve_listen_fd = -1;
        free(config);
        s_tcp_serve_task = NULL;
        vTaskDelete(NULL);
        return;
    }

    size_t sent_total = 0;
    while (sent_total < config->response_len) {
        int sent = send(
            client_fd,
            config->response + sent_total,
            config->response_len - sent_total,
            0
        );
        if (sent <= 0) {
            s_tcp_serve_status = ESP_FAIL;
            xEventGroupSetBits(s_events, TCP_SERVE_DONE_BIT);
            write_line("ERR ", "code=tcp-serve detail=send errno=%d", errno);
            close(client_fd);
            s_tcp_serve_client_fd = -1;
            close(listen_fd);
            s_tcp_serve_listen_fd = -1;
            free(config);
            s_tcp_serve_task = NULL;
            vTaskDelete(NULL);
            return;
        }
        sent_total += (size_t)sent;
    }

    shutdown(client_fd, SHUT_RDWR);
    close(client_fd);
    s_tcp_serve_client_fd = -1;
    close(listen_fd);
    s_tcp_serve_listen_fd = -1;
    s_tcp_serve_status = ESP_OK;
    write_line("OK ", "TCP_SERVED recv=%d sent=%u", received, (unsigned)sent_total);
    xEventGroupSetBits(s_events, TCP_SERVE_DONE_BIT);
    free(config);
    s_tcp_serve_task = NULL;
    vTaskDelete(NULL);
}

static esp_err_t start_tcp_serve_once(uint16_t port, const uint8_t *response, size_t response_len) {
    if (response_len == 0 || response_len > 256) {
        return ESP_ERR_INVALID_SIZE;
    }
    if (s_tcp_serve_task != NULL) {
        return ESP_ERR_INVALID_STATE;
    }
    xEventGroupClearBits(s_events, TCP_SERVE_READY_BIT | TCP_SERVE_DONE_BIT);
    s_tcp_serve_status = ESP_ERR_INVALID_STATE;
    tcp_serve_once_config_t *config = calloc(1, sizeof(*config));
    if (config == NULL) {
        return ESP_ERR_NO_MEM;
    }
    config->port = port;
    config->response_len = response_len;
    memcpy(config->response, response, response_len);

    BaseType_t ok = xTaskCreate(
        tcp_serve_once_task,
        "tcp_serve_once",
        4096,
        config,
        tskIDLE_PRIORITY + 2,
        &s_tcp_serve_task
    );
    if (ok != pdPASS) {
        s_tcp_serve_task = NULL;
        free(config);
        return ESP_ERR_NO_MEM;
    }
    EventBits_t bits = xEventGroupWaitBits(
        s_events,
        TCP_SERVE_READY_BIT | TCP_SERVE_DONE_BIT,
        pdFALSE,
        pdFALSE,
        pdMS_TO_TICKS(5000)
    );
    if ((bits & TCP_SERVE_READY_BIT) != 0) {
        return ESP_OK;
    }
    return s_tcp_serve_status == ESP_OK ? ESP_FAIL : s_tcp_serve_status;
}

static esp_err_t init_espnow(uint8_t channel, const uint8_t *pmk, bool has_pmk) {
    esp_err_t err = ensure_wifi_station();
    if (err != ESP_OK) {
        return err;
    }
    err = esp_wifi_set_channel(channel, WIFI_SECOND_CHAN_NONE);
    if (err != ESP_OK) {
        return err;
    }

    if (s_espnow_active) {
        esp_now_unregister_recv_cb();
        esp_now_unregister_send_cb();
        err = esp_now_deinit();
        if (err != ESP_OK) {
            return err;
        }
        s_espnow_active = false;
    }

    err = esp_now_init();
    if (err != ESP_OK) {
        return err;
    }
    err = esp_now_register_send_cb(espnow_send_cb);
    if (err != ESP_OK) {
        return err;
    }
    err = esp_now_register_recv_cb(espnow_recv_cb);
    if (err != ESP_OK) {
        return err;
    }
    if (has_pmk) {
        err = esp_now_set_pmk(pmk);
        if (err != ESP_OK) {
            return err;
        }
    }
    s_espnow_active = true;
    return ESP_OK;
}

static esp_err_t add_espnow_peer(
    const uint8_t peer[6],
    uint8_t channel,
    bool encrypt,
    const uint8_t *lmk,
    bool has_lmk
) {
    esp_now_peer_info_t info = {0};
    memcpy(info.peer_addr, peer, 6);
    info.channel = channel;
    info.ifidx = WIFI_IF_STA;
    info.encrypt = encrypt;
    if (encrypt && has_lmk) {
        memcpy(info.lmk, lmk, ESP_NOW_KEY_LEN);
    }
    return esp_now_add_peer(&info);
}

static bool read_line(char *buffer, size_t len) {
    size_t used = 0;
    while (true) {
        uint8_t byte = 0;
        int read = uart_read_bytes(COMPANION_UART, &byte, 1, portMAX_DELAY);
        if (read <= 0) {
            continue;
        }
        if (byte == '\r') {
            continue;
        }
        if (byte == '\n') {
            if (used == 0) {
                continue;
            }
            buffer[used] = '\0';
            return true;
        }
        if (used + 1 < len) {
            buffer[used++] = (char)byte;
        }
    }
}

static size_t split_args(char *line, char **tokens, size_t max_tokens) {
    size_t count = 0;
    char *cursor = line;
    while (*cursor != '\0' && count < max_tokens) {
        while (*cursor != '\0' && isspace((unsigned char)*cursor)) {
            cursor++;
        }
        if (*cursor == '\0') {
            break;
        }
        tokens[count++] = cursor;
        while (*cursor != '\0' && !isspace((unsigned char)*cursor)) {
            cursor++;
        }
        if (*cursor == '\0') {
            break;
        }
        *cursor++ = '\0';
    }
    return count;
}

static size_t collect_pairs(char **tokens, size_t token_count, arg_pair_t *pairs, size_t max_pairs) {
    size_t pair_count = 0;
    for (size_t index = 0; index < token_count && pair_count < max_pairs; index++) {
        char *eq = strchr(tokens[index], '=');
        if (eq == NULL || eq == tokens[index] || eq[1] == '\0') {
            continue;
        }
        *eq = '\0';
        pairs[pair_count].key = tokens[index];
        pairs[pair_count].value = eq + 1;
        pair_count++;
    }
    return pair_count;
}

static void handle_ping(void) {
    reply_ok("PONG");
}

static void handle_mac(void) {
    uint8_t mac[6];
    esp_err_t err = esp_read_mac(mac, ESP_MAC_WIFI_STA);
    if (err != ESP_OK) {
        reply_err("mac", "%s", esp_err_to_name(err));
        return;
    }
    char text[18];
    format_mac(mac, text, sizeof(text));
    reply_ok("MAC %s", text);
}

static void handle_reset_wifi(void) {
    esp_err_t err = reset_wifi_runtime();
    if (err == ESP_OK) {
        reply_ok("RESET_WIFI");
    } else {
        reply_err("reset-wifi", "%s", esp_err_to_name(err));
    }
}

static void handle_sta_join_common(
    const arg_pair_t *args,
    size_t arg_count,
    const char *command_name,
    bool preserve_ap
) {
    const char *ssid = find_arg(args, arg_count, "ssid");
    const char *pass = find_arg(args, arg_count, "pass");
    const char *timeout_text = find_arg(args, arg_count, "timeout_ms");
    const char *wait_ip_text = find_arg(args, arg_count, "wait_ip");
    const char *channel_text = find_arg(args, arg_count, "channel");
    uint32_t timeout_ms = DEFAULT_STA_TIMEOUT_MS;
    uint32_t wait_ip = 1;
    uint32_t channel = 0;
    if (timeout_text != NULL && !parse_u32(timeout_text, &timeout_ms)) {
        reply_err("invalid-timeout", "%s", timeout_text);
        return;
    }
    if (wait_ip_text != NULL && !parse_u32(wait_ip_text, &wait_ip)) {
        reply_err("invalid-wait-ip", "%s", wait_ip_text);
        return;
    }
    if (channel_text != NULL &&
        (!parse_u32(channel_text, &channel) || channel == 0 || channel > 14)) {
        reply_err("invalid-channel", "%s", channel_text);
        return;
    }

    esp_err_t err =
        join_station_with_mode(ssid, pass, timeout_ms, (uint8_t)channel, wait_ip != 0, preserve_ap);
    if (err == ESP_OK) {
        esp_ip4_addr_t ip;
        wifi_ap_record_t ap_info;
        portENTER_CRITICAL(&s_state_lock);
        ip = s_last_ipv4;
        portEXIT_CRITICAL(&s_state_lock);
        char text[16];
        format_ipv4(ip, text, sizeof(text));
        memset(&ap_info, 0, sizeof(ap_info));
        err = esp_wifi_sta_get_ap_info(&ap_info);
        if (err == ESP_OK && wait_ip != 0) {
            reply_ok("%s ip=%s ch=%u", command_name, text, (unsigned)ap_info.primary);
        } else if (err == ESP_OK) {
            reply_ok("%s connected=1 ch=%u", command_name, (unsigned)ap_info.primary);
        } else if (wait_ip != 0) {
            reply_ok("%s ip=%s", command_name, text);
        } else {
            reply_ok("%s connected=1", command_name);
        }
        return;
    }
    if (err == ESP_ERR_TIMEOUT) {
        EventBits_t bits = xEventGroupGetBits(s_events);
        reply_err(
            "sta-timeout",
            "%" PRIu32 "ms connected=%u got_ip=%u",
            timeout_ms,
            (unsigned)((bits & WIFI_CONNECTED_BIT) != 0),
            (unsigned)((bits & WIFI_GOT_IP_BIT) != 0)
        );
        return;
    }

    wifi_event_sta_disconnected_t disconnect;
    portENTER_CRITICAL(&s_state_lock);
    disconnect = s_last_disconnect;
    portEXIT_CRITICAL(&s_state_lock);
    reply_err("sta-join", "reason=%d", disconnect.reason);
}

static void handle_sta_join(const arg_pair_t *args, size_t arg_count) {
    handle_sta_join_common(args, arg_count, "STA_JOIN", false);
}

static void handle_sta_join_keep_ap(const arg_pair_t *args, size_t arg_count) {
    handle_sta_join_common(args, arg_count, "STA_JOIN_KEEP_AP", true);
}

static void handle_sta_disconnect(void) {
    esp_err_t err = esp_wifi_disconnect();
    if (err == ESP_OK || err == ESP_ERR_WIFI_NOT_CONNECT || err == ESP_ERR_WIFI_NOT_STARTED) {
        reply_ok("STA_DISCONNECT");
    } else {
        reply_err("sta-disconnect", "%s", esp_err_to_name(err));
    }
}

static void handle_ap_start(const arg_pair_t *args, size_t arg_count) {
    const char *ssid = find_arg(args, arg_count, "ssid");
    const char *pass = find_arg(args, arg_count, "pass");
    const char *channel_text = find_arg(args, arg_count, "channel");
    uint32_t channel = 6;
    if (channel_text != NULL && !parse_u32(channel_text, &channel)) {
        reply_err("invalid-channel", "%s", channel_text);
        return;
    }
    if (channel == 0 || channel > 14) {
        reply_err("invalid-channel", "%" PRIu32, channel);
        return;
    }

    esp_err_t err = start_softap(ssid, pass, (uint8_t)channel);
    if (err == ESP_OK) {
        reply_ok("AP_START ssid=%s ch=%" PRIu32, ssid, channel);
    } else {
        reply_err("ap-start", "%s", esp_err_to_name(err));
    }
}

static void handle_ap_info(void) {
    wifi_mode_t mode = WIFI_MODE_NULL;
    esp_err_t mode_err = esp_wifi_get_mode(&mode);
    wifi_config_t config = {0};
    esp_err_t config_err = esp_wifi_get_config(WIFI_IF_AP, &config);
    if (mode_err != ESP_OK || config_err != ESP_OK) {
        reply_err(
            "ap-info",
            "mode=%s config=%s",
            esp_err_to_name(mode_err),
            esp_err_to_name(config_err)
        );
        return;
    }

    char ssid[33];
    size_t ssid_len = config.ap.ssid_len;
    if (ssid_len == 0) {
        ssid_len = strnlen((const char *)config.ap.ssid, sizeof(config.ap.ssid));
    }
    if (ssid_len > 32) {
        ssid_len = 32;
    }
    memcpy(ssid, config.ap.ssid, ssid_len);
    ssid[ssid_len] = '\0';

    reply_ok(
        "AP_INFO mode=%d started=%d ssid=%s channel=%u auth=%d",
        (int)mode,
        s_wifi_started ? 1 : 0,
        ssid,
        (unsigned)config.ap.channel,
        (int)config.ap.authmode
    );
}

static void handle_scan_find(const arg_pair_t *args, size_t arg_count) {
    const char *ssid = find_arg(args, arg_count, "ssid");
    if (ssid == NULL || ssid[0] == '\0') {
        reply_err("invalid-scan-find", "expected ssid");
        return;
    }

    esp_err_t err = ensure_wifi_station();
    if (err != ESP_OK) {
        reply_err("scan-find", "%s", esp_err_to_name(err));
        return;
    }

    uint8_t scan_ssid[33] = {0};
    size_t ssid_len = strlen(ssid);
    if (ssid_len >= sizeof(scan_ssid)) {
        reply_err("invalid-scan-find", "ssid too long");
        return;
    }
    memcpy(scan_ssid, ssid, ssid_len);

    wifi_scan_config_t config = {0};
    config.ssid = scan_ssid;
    config.show_hidden = true;
    config.scan_type = WIFI_SCAN_TYPE_ACTIVE;
    config.scan_time.active.min = 30;
    config.scan_time.active.max = 120;

    err = esp_wifi_scan_start(&config, true);
    if (err != ESP_OK) {
        reply_err("scan-find", "%s", esp_err_to_name(err));
        return;
    }

    uint16_t total = 0;
    err = esp_wifi_scan_get_ap_num(&total);
    if (err != ESP_OK) {
        reply_err("scan-find", "%s", esp_err_to_name(err));
        return;
    }

    wifi_ap_record_t records[8];
    memset(records, 0, sizeof(records));
    uint16_t record_count = total;
    if (record_count > (sizeof(records) / sizeof(records[0]))) {
        record_count = sizeof(records) / sizeof(records[0]);
    }

    err = esp_wifi_scan_get_ap_records(&record_count, records);
    if (err != ESP_OK) {
        reply_err("scan-find", "%s", esp_err_to_name(err));
        return;
    }

    bool found = false;
    wifi_ap_record_t best = {0};
    for (uint16_t index = 0; index < record_count; index++) {
        if (strcmp((const char *)records[index].ssid, ssid) != 0) {
            continue;
        }
        if (!found || records[index].rssi > best.rssi) {
            best = records[index];
            found = true;
        }
    }

    if (!found) {
        reply_ok("SCAN_FIND found=0 ssid=%s total=%u", ssid, (unsigned)total);
        return;
    }

    reply_ok(
        "SCAN_FIND found=1 ssid=%s ch=%u rssi=%d auth=%s total=%u",
        ssid,
        (unsigned)best.primary,
        best.rssi,
        auth_mode_name(best.authmode),
        (unsigned)total
    );
}

static void handle_sta_wait_ip(const arg_pair_t *args, size_t arg_count) {
    const char *timeout_text = find_arg(args, arg_count, "timeout_ms");
    uint32_t timeout_ms = DEFAULT_STA_TIMEOUT_MS;
    if (timeout_text != NULL && !parse_u32(timeout_text, &timeout_ms)) {
        reply_err("invalid-timeout", "%s", timeout_text);
        return;
    }

    esp_err_t err = wait_for_sta_ip(timeout_ms);
    if (err == ESP_OK) {
        esp_ip4_addr_t ip;
        portENTER_CRITICAL(&s_state_lock);
        ip = s_last_ipv4;
        portEXIT_CRITICAL(&s_state_lock);

        char text[16];
        format_ipv4(ip, text, sizeof(text));
        reply_ok("STA_WAIT_IP ip=%s", text);
        return;
    }
    if (err == ESP_ERR_TIMEOUT) {
        reply_err("sta-timeout", "%" PRIu32 "ms", timeout_ms);
        return;
    }

    wifi_event_sta_disconnected_t disconnect;
    portENTER_CRITICAL(&s_state_lock);
    disconnect = s_last_disconnect;
    portEXIT_CRITICAL(&s_state_lock);
    reply_err("sta-wait-ip", "reason=%d", disconnect.reason);
}

static void handle_udp_send(const arg_pair_t *args, size_t arg_count) {
    const char *host_text = find_arg(args, arg_count, "host");
    const char *port_text = find_arg(args, arg_count, "port");
    const char *count_text = find_arg(args, arg_count, "count");
    const char *size_text = find_arg(args, arg_count, "size");
    esp_ip4_addr_t host;
    uint32_t port = 0;
    uint32_t count = 0;
    uint32_t size = 0;
    if (host_text == NULL || !parse_ipv4(host_text, &host) || port_text == NULL ||
        !parse_u32(port_text, &port) || count_text == NULL || !parse_u32(count_text, &count) ||
        size_text == NULL || !parse_u32(size_text, &size) || port > UINT16_MAX) {
        reply_err("invalid-udp-send", "expected host/port/count/size");
        return;
    }

    esp_err_t err = udp_send_packets(host, (uint16_t)port, count, size);
    if (err == ESP_OK) {
        reply_ok("UDP_SEND count=%" PRIu32, count);
    } else {
        reply_err("udp-send", "%s", esp_err_to_name(err));
    }
}

static void handle_dhcp_request(const arg_pair_t *args, size_t arg_count) {
    const char *server_text = find_arg(args, arg_count, "server");
    const char *requested_text = find_arg(args, arg_count, "requested");
    esp_ip4_addr_t server;
    esp_ip4_addr_t requested;
    if (server_text == NULL || requested_text == NULL || !parse_ipv4(server_text, &server) ||
        !parse_ipv4(requested_text, &requested)) {
        reply_err("invalid-dhcp-request", "expected server/requested");
        return;
    }

    esp_err_t err = send_dhcp_request_probe(server, requested);
    if (err == ESP_OK) {
        reply_ok("DHCP_REQUEST requested=%s server=%s", requested_text, server_text);
    } else {
        reply_err("dhcp-request", "%s", esp_err_to_name(err));
    }
}

static void handle_tcp_connect(const arg_pair_t *args, size_t arg_count) {
    const char *host_text = find_arg(args, arg_count, "host");
    const char *port_text = find_arg(args, arg_count, "port");
    const char *payload_text = find_arg(args, arg_count, "payload");
    const char *iface_text = find_arg(args, arg_count, "iface");
    const char *timeout_text = find_arg(args, arg_count, "timeout_ms");
    esp_ip4_addr_t host;
    uint32_t port = 0;
    uint32_t timeout_ms = 3000;
    uint8_t payload[256];
    size_t payload_len = 0;
    if (host_text == NULL || !parse_ipv4(host_text, &host) || port_text == NULL ||
        !parse_u32(port_text, &port) || port > UINT16_MAX || payload_text == NULL ||
        !parse_hex_bytes(payload_text, payload, sizeof(payload), &payload_len)) {
        reply_err("invalid-tcp-connect", "expected host/port/payload");
        return;
    }
    if (timeout_text != NULL && (!parse_u32(timeout_text, &timeout_ms) || timeout_ms == 0 || timeout_ms > 60000)) {
        reply_err("invalid-tcp-connect", "invalid-timeout");
        return;
    }

    esp_netif_t *bind_netif = NULL;
    if (iface_text != NULL && iface_text[0] != '\0') {
        if (strcmp(iface_text, "ap") == 0) {
            bind_netif = s_ap_netif;
        } else if (strcmp(iface_text, "sta") == 0) {
            bind_netif = s_sta_netif;
        } else {
            reply_err("invalid-tcp-connect", "unknown iface");
            return;
        }
    }

    if (bind_netif == s_ap_netif) {
        esp_err_t arp_err = seed_ap_station_arp(host);
        if (arp_err != ESP_OK) {
            reply_err(
                "tcp-connect",
                "arp-seed=%s arp=%d sta=%d",
                esp_err_to_name(arp_err),
                s_last_tcp_arp_result,
                s_last_tcp_sta_count
            );
            return;
        }
    }

    esp_err_t err = tcp_round_trip(host, (uint16_t)port, payload, payload_len, bind_netif, timeout_ms);
    if (err != ESP_OK) {
        reply_err(
            "tcp-connect",
            "%s stage=%s errno=%d arp=%d sta=%d",
            esp_err_to_name(err),
            s_last_tcp_stage != NULL ? s_last_tcp_stage : "unknown",
            s_last_tcp_errno,
            s_last_tcp_arp_result,
            s_last_tcp_sta_count
        );
    }
}

static void handle_tcp_serve_once(const arg_pair_t *args, size_t arg_count) {
    const char *port_text = find_arg(args, arg_count, "port");
    const char *response_text = find_arg(args, arg_count, "response");
    uint32_t port = 0;
    uint8_t response[256];
    size_t response_len = 0;
    if (port_text == NULL || !parse_u32(port_text, &port) || port == 0 || port > UINT16_MAX ||
        response_text == NULL || !parse_hex_bytes(response_text, response, sizeof(response), &response_len)) {
        reply_err("invalid-tcp-serve-once", "expected port/response");
        return;
    }

    esp_err_t err = start_tcp_serve_once((uint16_t)port, response, response_len);
    if (err == ESP_OK) {
        reply_ok("TCP_SERVE_ONCE port=%" PRIu32, port);
    } else {
        reply_err("tcp-serve-once", "%s", esp_err_to_name(err));
    }
}

static void handle_raw80211_tx(const arg_pair_t *args, size_t arg_count) {
    static const uint8_t broadcast[6] = {0xff, 0xff, 0xff, 0xff, 0xff, 0xff};
    const char *kind = find_arg(args, arg_count, "kind");
    const char *channel_text = find_arg(args, arg_count, "channel");
    const char *count_text = find_arg(args, arg_count, "count");
    const char *interval_text = find_arg(args, arg_count, "interval_ms");
    const char *dest_text = find_arg(args, arg_count, "dest");
    uint32_t channel = 6;
    uint32_t count = 64;
    uint32_t interval_ms = 5;
    uint8_t destination[6];
    memcpy(destination, broadcast, sizeof(destination));

    if (kind == NULL ||
        (channel_text != NULL && (!parse_u32(channel_text, &channel) || channel == 0 || channel > 14)) ||
        (count_text != NULL && (!parse_u32(count_text, &count) || count == 0)) ||
        (interval_text != NULL && !parse_u32(interval_text, &interval_ms)) ||
        (dest_text != NULL && !parse_mac(dest_text, destination))) {
        reply_err("invalid-raw80211-tx", "expected kind/channel/count/interval_ms/dest");
        return;
    }

    esp_err_t err =
        send_raw80211_frames(kind, (uint8_t)channel, destination, count, interval_ms);
    if (err == ESP_OK) {
        reply_ok("RAW80211_TX kind=%s channel=%" PRIu32 " count=%" PRIu32, kind, channel, count);
    } else {
        reply_err("raw80211-tx", "%s", esp_err_to_name(err));
    }
}

static void handle_wps_ap_start(const arg_pair_t *args, size_t arg_count) {
    const char *ssid = find_arg(args, arg_count, "ssid");
    const char *pass = find_arg(args, arg_count, "pass");
    const char *channel_text = find_arg(args, arg_count, "channel");
    const char *mode_text = find_arg(args, arg_count, "mode");
    const char *pin_text = find_arg(args, arg_count, "pin");
    const char *reuse_ap_text = find_arg(args, arg_count, "reuse_ap");
    uint32_t channel = 0;
    if (ssid == NULL || channel_text == NULL || !parse_u32(channel_text, &channel) ||
        channel == 0 || channel > 14) {
        reply_err("invalid-wps-ap-start", "expected ssid/pass/channel");
        return;
    }
    bool pin_mode = mode_text != NULL && strcmp(mode_text, "pin") == 0;
    if (pin_mode && (pin_text == NULL || strlen(pin_text) != 8)) {
        reply_err("invalid-wps-ap-start", "pin mode requires 8 digit pin");
        return;
    }

    bool reuse_ap = reuse_ap_text != NULL && strcmp(reuse_ap_text, "1") == 0;
    esp_err_t err = ESP_OK;
    if (!reuse_ap) {
        err = start_softap(ssid, pass, (uint8_t)channel);
        if (err != ESP_OK) {
            reply_err("wps-ap-start", "%s", esp_err_to_name(err));
            return;
        }
    }

    portENTER_CRITICAL(&s_state_lock);
    memset(s_last_wps_peer, 0, sizeof(s_last_wps_peer));
    s_last_wps_reason = 0;
    s_wps_ap_failures = 0;
    s_wps_ap_timeouts = 0;
    s_wps_ap_restarts = 0;
    s_last_wps_restart_err = ESP_OK;
    s_wps_ap_active = false;
    s_wps_ap_restarting = false;
    s_wps_ap_pin_mode = pin_mode;
    memset(s_wps_ap_pin, 0, sizeof(s_wps_ap_pin));
    if (pin_mode) {
        memcpy(s_wps_ap_pin, pin_text, 8);
    }
    portEXIT_CRITICAL(&s_state_lock);
    xEventGroupClearBits(s_events, WPS_AP_SUCCESS_BIT | WPS_AP_FAILED_BIT | WPS_AP_TIMEOUT_BIT);

    err = esp_wifi_ap_wps_disable();
    if (!wps_disable_error_is_ignorable(err)) {
        reply_err("wps-ap-disable", "%s", esp_err_to_name(err));
        return;
    }

    esp_wps_config_t config = WPS_CONFIG_INIT_DEFAULT(pin_mode ? WPS_TYPE_PIN : WPS_TYPE_PBC);
    err = esp_wifi_ap_wps_enable(&config);
    if (err != ESP_OK) {
        reply_err("wps-ap-enable", "%s", esp_err_to_name(err));
        return;
    }
    err = esp_wifi_ap_wps_start(pin_mode ? (const unsigned char *)pin_text : NULL);
    if (err != ESP_OK) {
        reply_err("wps-ap-start", "%s", esp_err_to_name(err));
        return;
    }
    portENTER_CRITICAL(&s_state_lock);
    s_wps_ap_active = true;
    portEXIT_CRITICAL(&s_state_lock);
    reply_ok("WPS_AP_START ssid=%s channel=%" PRIu32 " mode=%s", ssid, channel, pin_mode ? "pin" : "pbc");
}

static void handle_wps_ap_wait(const arg_pair_t *args, size_t arg_count) {
    const char *timeout_text = find_arg(args, arg_count, "timeout_ms");
    uint32_t timeout_ms = DEFAULT_STA_TIMEOUT_MS;
    if (timeout_text != NULL && !parse_u32(timeout_text, &timeout_ms)) {
        reply_err("invalid-wps-ap-wait", "expected timeout_ms");
        return;
    }

    EventBits_t bits = xEventGroupWaitBits(
        s_events,
        WPS_AP_SUCCESS_BIT | WPS_AP_FAILED_BIT | WPS_AP_TIMEOUT_BIT,
        pdFALSE,
        pdFALSE,
        pdMS_TO_TICKS(timeout_ms)
    );
    if ((bits & WPS_AP_SUCCESS_BIT) != 0) {
        uint8_t peer[6];
        uint32_t failures;
        uint32_t timeouts;
        uint32_t restarts;
        portENTER_CRITICAL(&s_state_lock);
        memcpy(peer, s_last_wps_peer, sizeof(peer));
        failures = s_wps_ap_failures;
        timeouts = s_wps_ap_timeouts;
        restarts = s_wps_ap_restarts;
        portEXIT_CRITICAL(&s_state_lock);
        char peer_text[18];
        format_mac(peer, peer_text, sizeof(peer_text));
        reply_ok(
            "WPS_AP_WAIT peer=%s failures=%" PRIu32 " timeouts=%" PRIu32 " restarts=%" PRIu32,
            peer_text,
            failures,
            timeouts,
            restarts
        );
        return;
    }
    if ((bits & WPS_AP_FAILED_BIT) != 0) {
        int reason;
        uint32_t failures;
        uint32_t timeouts;
        uint32_t restarts;
        esp_err_t restart_err;
        portENTER_CRITICAL(&s_state_lock);
        reason = s_last_wps_reason;
        failures = s_wps_ap_failures;
        timeouts = s_wps_ap_timeouts;
        restarts = s_wps_ap_restarts;
        restart_err = s_last_wps_restart_err;
        portEXIT_CRITICAL(&s_state_lock);
        reply_err(
            "wps-ap-failed",
            "reason=%d failures=%" PRIu32 " timeouts=%" PRIu32 " restarts=%" PRIu32 " restart_err=%s",
            reason,
            failures,
            timeouts,
            restarts,
            esp_err_to_name(restart_err)
        );
        return;
    }
    if ((bits & WPS_AP_TIMEOUT_BIT) != 0) {
        uint32_t failures;
        uint32_t timeouts;
        uint32_t restarts;
        portENTER_CRITICAL(&s_state_lock);
        failures = s_wps_ap_failures;
        timeouts = s_wps_ap_timeouts;
        restarts = s_wps_ap_restarts;
        portEXIT_CRITICAL(&s_state_lock);
        reply_err(
            "wps-ap-timeout",
            "registrar-timeout failures=%" PRIu32 " timeouts=%" PRIu32 " restarts=%" PRIu32,
            failures,
            timeouts,
            restarts
        );
        return;
    }

    uint32_t failures;
    uint32_t timeouts;
    uint32_t restarts;
    esp_err_t restart_err;
    bool active;
    portENTER_CRITICAL(&s_state_lock);
    failures = s_wps_ap_failures;
    timeouts = s_wps_ap_timeouts;
    restarts = s_wps_ap_restarts;
    restart_err = s_last_wps_restart_err;
    active = s_wps_ap_active;
    portEXIT_CRITICAL(&s_state_lock);
    reply_err(
        "wps-ap-timeout",
        "%" PRIu32 "ms active=%d failures=%" PRIu32 " timeouts=%" PRIu32 " restarts=%" PRIu32 " restart_err=%s",
        timeout_ms,
        active ? 1 : 0,
        failures,
        timeouts,
        restarts,
        esp_err_to_name(restart_err)
    );
}

static void handle_smartconfig_ap_provision(const arg_pair_t *args, size_t arg_count) {
    const char *ssid = find_arg(args, arg_count, "ssid");
    const char *pass = find_arg(args, arg_count, "pass");
    const char *channel_text = find_arg(args, arg_count, "channel");
    const char *timeout_text = find_arg(args, arg_count, "timeout_ms");
    uint32_t channel = 0;
    uint32_t timeout_ms = DEFAULT_SMARTCONFIG_SEND_TIMEOUT_MS;
    if (ssid == NULL || channel_text == NULL || !parse_u32(channel_text, &channel) ||
        channel == 0 || channel > 14 ||
        (timeout_text != NULL && !parse_u32(timeout_text, &timeout_ms))) {
        reply_err("invalid-smartconfig-ap-provision", "expected ssid/pass/channel");
        return;
    }

    esp_err_t err = smartconfig_ap_provision(ssid, pass, (uint8_t)channel, timeout_ms);
    if (err == ESP_OK) {
        reply_ok("SMARTCONFIG_AP_PROVISION ssid=%s channel=%" PRIu32, ssid, channel);
    } else {
        reply_err(
            "smartconfig-ap-provision",
            "%s step=%s errno=%d",
            esp_err_to_name(err),
            s_last_smartconfig_step,
            s_last_smartconfig_errno
        );
    }
}

static void handle_espnow_tx(const arg_pair_t *args, size_t arg_count) {
    const char *channel_text = find_arg(args, arg_count, "channel");
    const char *peer_text = find_arg(args, arg_count, "peer");
    const char *encrypt_text = find_arg(args, arg_count, "encrypt");
    const char *lmk_text = find_arg(args, arg_count, "lmk");
    const char *pmk_text = find_arg(args, arg_count, "pmk");
    const char *payload_text = find_arg(args, arg_count, "payload");
    const char *timeout_text = find_arg(args, arg_count, "timeout_ms");

    uint32_t channel = 0;
    uint32_t encrypt = 0;
    uint32_t timeout_ms = DEFAULT_ESPNOW_TIMEOUT_MS;
    uint8_t peer[6];
    uint8_t lmk[ESP_NOW_KEY_LEN];
    uint8_t pmk[ESP_NOW_KEY_LEN];
    uint8_t payload[ESPNOW_MAX_PAYLOAD];
    size_t payload_len = 0;
    bool has_lmk = lmk_text != NULL && strcmp(lmk_text, "none") != 0;
    bool has_pmk = pmk_text != NULL && strcmp(pmk_text, "none") != 0;

    if (channel_text == NULL || !parse_u32(channel_text, &channel) || channel == 0 || channel > 14 ||
        peer_text == NULL || !parse_mac(peer_text, peer) || encrypt_text == NULL ||
        !parse_u32(encrypt_text, &encrypt) || payload_text == NULL ||
        !parse_hex_bytes(payload_text, payload, sizeof(payload), &payload_len) ||
        (has_lmk && !parse_hex_bytes(lmk_text, lmk, sizeof(lmk), NULL)) ||
        (has_pmk && !parse_hex_bytes(pmk_text, pmk, sizeof(pmk), NULL)) ||
        (timeout_text != NULL && !parse_u32(timeout_text, &timeout_ms))) {
        reply_err("invalid-espnow-tx", "expected channel/peer/encrypt/payload");
        return;
    }

    esp_err_t err = init_espnow((uint8_t)channel, pmk, has_pmk);
    if (err != ESP_OK) {
        reply_err("espnow-init", "%s", esp_err_to_name(err));
        return;
    }
    err = add_espnow_peer(peer, (uint8_t)channel, encrypt != 0, lmk, has_lmk);
    if (err != ESP_OK) {
        reply_err("espnow-peer", "%s", esp_err_to_name(err));
        return;
    }

    clear_async_state();
    err = esp_now_send(peer, payload, payload_len);
    if (err != ESP_OK) {
        reply_err("espnow-send", "%s", esp_err_to_name(err));
        return;
    }

    EventBits_t bits = xEventGroupWaitBits(
        s_events,
        ESPNOW_SEND_DONE_BIT,
        pdFALSE,
        pdTRUE,
        pdMS_TO_TICKS(timeout_ms)
    );
    if ((bits & ESPNOW_SEND_DONE_BIT) == 0) {
        reply_err("espnow-timeout", "%" PRIu32 "ms", timeout_ms);
        return;
    }

    esp_now_send_status_t status;
    bool valid;
    portENTER_CRITICAL(&s_state_lock);
    status = s_last_send_status;
    valid = s_last_send_valid;
    portEXIT_CRITICAL(&s_state_lock);
    if (!valid || status != ESP_NOW_SEND_SUCCESS) {
        reply_err("espnow-status", "%s", valid ? "failed" : "missing");
        return;
    }

    reply_ok("ESPNOW_TX status=success");
}

static void handle_espnow_rx(const arg_pair_t *args, size_t arg_count) {
    const char *channel_text = find_arg(args, arg_count, "channel");
    const char *peer_text = find_arg(args, arg_count, "peer");
    const char *lmk_text = find_arg(args, arg_count, "lmk");
    const char *pmk_text = find_arg(args, arg_count, "pmk");
    const char *timeout_text = find_arg(args, arg_count, "timeout_ms");
    uint32_t channel = 0;
    uint32_t timeout_ms = DEFAULT_ESPNOW_TIMEOUT_MS;
    uint8_t peer[6];
    uint8_t lmk[ESP_NOW_KEY_LEN];
    uint8_t pmk[ESP_NOW_KEY_LEN];
    bool have_peer = peer_text != NULL && strcmp(peer_text, "any") != 0;
    bool has_lmk = lmk_text != NULL && strcmp(lmk_text, "none") != 0;
    bool has_pmk = pmk_text != NULL && strcmp(pmk_text, "none") != 0;

    if (channel_text == NULL || !parse_u32(channel_text, &channel) || channel == 0 || channel > 14 ||
        (have_peer && !parse_mac(peer_text, peer)) ||
        (has_lmk && !parse_hex_bytes(lmk_text, lmk, sizeof(lmk), NULL)) ||
        (has_pmk && !parse_hex_bytes(pmk_text, pmk, sizeof(pmk), NULL)) ||
        (timeout_text != NULL && !parse_u32(timeout_text, &timeout_ms))) {
        reply_err("invalid-espnow-rx", "expected channel and optional peer");
        return;
    }

    esp_err_t err = init_espnow((uint8_t)channel, pmk, has_pmk);
    if (err != ESP_OK) {
        reply_err("espnow-init", "%s", esp_err_to_name(err));
        return;
    }
    if (have_peer) {
        err = add_espnow_peer(peer, (uint8_t)channel, has_lmk, lmk, has_lmk);
        if (err != ESP_OK) {
            reply_err("espnow-peer", "%s", esp_err_to_name(err));
            return;
        }
    }

    clear_async_state();
    EventBits_t bits = xEventGroupWaitBits(
        s_events,
        ESPNOW_RECV_DONE_BIT,
        pdFALSE,
        pdTRUE,
        pdMS_TO_TICKS(timeout_ms)
    );
    if ((bits & ESPNOW_RECV_DONE_BIT) == 0) {
        reply_err("espnow-timeout", "%" PRIu32 "ms", timeout_ms);
        return;
    }

    uint8_t peer_copy[6];
    uint8_t payload_copy[ESPNOW_MAX_PAYLOAD];
    int payload_len = 0;
    bool valid = false;
    portENTER_CRITICAL(&s_state_lock);
    memcpy(peer_copy, s_last_recv_peer, sizeof(peer_copy));
    memcpy(payload_copy, s_last_recv_payload, sizeof(payload_copy));
    payload_len = s_last_recv_len;
    valid = s_last_recv_valid;
    portEXIT_CRITICAL(&s_state_lock);
    if (!valid || payload_len < 0) {
        reply_err("espnow-rx", "missing");
        return;
    }

    char peer_text_out[18];
    char payload_text_out[ESPNOW_MAX_PAYLOAD * 2 + 1];
    format_mac(peer_copy, peer_text_out, sizeof(peer_text_out));
    hex_encode(payload_copy, (size_t)payload_len, payload_text_out, sizeof(payload_text_out));
    reply_ok("ESPNOW_RX peer=%s len=%d payload=%s", peer_text_out, payload_len, payload_text_out);
}

static void handle_promisc_traffic(const arg_pair_t *args, size_t arg_count) {
    const char *ssid = find_arg(args, arg_count, "ssid");
    const char *pass = find_arg(args, arg_count, "pass");
    const char *channel_text = find_arg(args, arg_count, "channel");
    const char *host_text = find_arg(args, arg_count, "host");
    const char *port_text = find_arg(args, arg_count, "port");
    const char *count_text = find_arg(args, arg_count, "count");
    const char *size_text = find_arg(args, arg_count, "size");
    const char *timeout_text = find_arg(args, arg_count, "timeout_ms");
    esp_ip4_addr_t host;
    uint32_t port = 0;
    uint32_t count = 0;
    uint32_t size = 256;
    uint32_t timeout_ms = DEFAULT_STA_TIMEOUT_MS;
    uint32_t channel = 0;
    if (host_text == NULL || !parse_ipv4(host_text, &host) || port_text == NULL ||
        !parse_u32(port_text, &port) || port > UINT16_MAX || count_text == NULL ||
        !parse_u32(count_text, &count) ||
        (size_text != NULL && !parse_u32(size_text, &size)) ||
        (channel_text != NULL && (!parse_u32(channel_text, &channel) || channel > 14)) ||
        (timeout_text != NULL && !parse_u32(timeout_text, &timeout_ms))) {
        reply_err("invalid-promisc-traffic", "expected ssid/host/port/count");
        return;
    }

    esp_err_t err = join_station_with_mode(ssid, pass, timeout_ms, (uint8_t)channel, true, false);
    if (err != ESP_OK) {
        reply_err("sta-join", "%s", esp_err_to_name(err));
        return;
    }
    err = udp_send_packets(host, (uint16_t)port, count, size);
    if (err != ESP_OK) {
        reply_err("promisc-traffic", "%s", esp_err_to_name(err));
        return;
    }
    reply_ok("PROMISC_TRAFFIC count=%" PRIu32, count);
}

static void dispatch_command(char *line) {
    char *tokens[17];
    arg_pair_t args[16];
    size_t token_count = split_args(line, tokens, sizeof(tokens) / sizeof(tokens[0]));
    if (token_count == 0) {
        return;
    }

    size_t arg_count = collect_pairs(&tokens[1], token_count - 1, args, sizeof(args) / sizeof(args[0]));
    const char *command = tokens[0];
    if (strcmp(command, "PING") == 0) {
        handle_ping();
    } else if (strcmp(command, "MAC") == 0) {
        handle_mac();
    } else if (strcmp(command, "RESET_WIFI") == 0) {
        handle_reset_wifi();
    } else if (strcmp(command, "SCAN_FIND") == 0) {
        handle_scan_find(args, arg_count);
    } else if (strcmp(command, "STA_JOIN") == 0) {
        handle_sta_join(args, arg_count);
    } else if (strcmp(command, "STA_JOIN_KEEP_AP") == 0) {
        handle_sta_join_keep_ap(args, arg_count);
    } else if (strcmp(command, "STA_DISCONNECT") == 0) {
        handle_sta_disconnect();
    } else if (strcmp(command, "AP_START") == 0) {
        handle_ap_start(args, arg_count);
    } else if (strcmp(command, "AP_INFO") == 0) {
        handle_ap_info();
    } else if (strcmp(command, "STA_WAIT_IP") == 0) {
        handle_sta_wait_ip(args, arg_count);
    } else if (strcmp(command, "STA_SET_STATIC") == 0) {
        handle_sta_set_static(args, arg_count);
    } else if (strcmp(command, "UDP_SEND") == 0) {
        handle_udp_send(args, arg_count);
    } else if (strcmp(command, "DHCP_REQUEST") == 0) {
        handle_dhcp_request(args, arg_count);
    } else if (strcmp(command, "TCP_CONNECT") == 0) {
        handle_tcp_connect(args, arg_count);
    } else if (strcmp(command, "TCP_SERVE_ONCE") == 0) {
        handle_tcp_serve_once(args, arg_count);
    } else if (strcmp(command, "RAW80211_TX") == 0) {
        handle_raw80211_tx(args, arg_count);
    } else if (strcmp(command, "WPS_AP_START") == 0) {
        handle_wps_ap_start(args, arg_count);
    } else if (strcmp(command, "WPS_AP_WAIT") == 0) {
        handle_wps_ap_wait(args, arg_count);
    } else if (strcmp(command, "SMARTCONFIG_AP_PROVISION") == 0) {
        handle_smartconfig_ap_provision(args, arg_count);
    } else if (strcmp(command, "ESPNOW_TX") == 0) {
        handle_espnow_tx(args, arg_count);
    } else if (strcmp(command, "ESPNOW_RX") == 0) {
        handle_espnow_rx(args, arg_count);
    } else if (strcmp(command, "PROMISC_TRAFFIC") == 0) {
        handle_promisc_traffic(args, arg_count);
    } else {
        reply_err("unknown-command", "%s", command);
    }
}

void app_main(void) {
    ESP_ERROR_CHECK(init_runtime());

    uint8_t mac[6];
    ESP_ERROR_CHECK(esp_read_mac(mac, ESP_MAC_WIFI_STA));
    char mac_text[18];
    format_mac(mac, mac_text, sizeof(mac_text));
    write_line("READY ", "companion esp32-pico-d4 mac=%s", mac_text);

    char line[COMMAND_BUFFER_SIZE];
    while (true) {
        if (read_line(line, sizeof(line))) {
            dispatch_command(line);
        }
    }
}
