/* Compile with the pinned target compiler and headers. The byte image cross-
 * checks src/lib.rs's manually maintained packed-bitfield representation. */
#include <stddef.h>
#include <sys/reent.h>
#include <sys/time.h>
#include "esp_event_legacy.h"
#include "esp_timer.h"
#include "esp_wifi_types.h"

/* Rust owns complete closed streams: zero access flags make retained stdio
 * fail with EBADF before buffer allocation or operation callbacks. */
_Static_assert(sizeof(struct __sFILE) == 104, "newlib FILE size");
_Static_assert(__alignof__(struct __sFILE) == 4, "newlib FILE alignment");
_Static_assert(offsetof(struct __sFILE, _flags) == 12, "newlib FILE access flags");
_Static_assert(offsetof(struct __sFILE, _flags2) == 100, "newlib FILE extended flags");
_Static_assert(sizeof(struct _reent) == 240, "newlib reentrancy size");
_Static_assert(__alignof__(struct _reent) == 4, "newlib reentrancy alignment");
_Static_assert(offsetof(struct _reent, _errno) == 0, "newlib errno offset");
_Static_assert(offsetof(struct _reent, _stdin) == 4, "newlib stdin offset");
_Static_assert(offsetof(struct _reent, _stdout) == 8, "newlib stdout offset");
_Static_assert(offsetof(struct _reent, _stderr) == 12, "newlib stderr offset");
_Static_assert(offsetof(struct _reent, __sdidinit) == 24, "newlib initialization offset");
_Static_assert(sizeof(struct timeval) == 8, "newlib timeval size");
_Static_assert(__alignof__(struct timeval) == 4, "newlib timeval alignment");
_Static_assert(offsetof(struct timeval, tv_sec) == 0, "newlib seconds offset");
_Static_assert(offsetof(struct timeval, tv_usec) == 4, "newlib microseconds offset");

_Static_assert(sizeof(esp_timer_create_args_t) == 16, "timer arguments size");
_Static_assert(__alignof__(esp_timer_create_args_t) == 4, "timer arguments alignment");
_Static_assert(offsetof(esp_timer_create_args_t, callback) == 0, "timer callback offset");
_Static_assert(offsetof(esp_timer_create_args_t, arg) == 4, "timer context offset");
_Static_assert(offsetof(esp_timer_create_args_t, dispatch_method) == 8, "timer dispatch offset");
_Static_assert(offsetof(esp_timer_create_args_t, name) == 12, "timer name offset");

_Static_assert(sizeof(wifi_ap_record_t) == 80, "scan record size");
_Static_assert(__alignof__(wifi_ap_record_t) == 4, "scan record alignment");
_Static_assert(offsetof(wifi_ap_record_t, country) == 68, "scan flags precede country");
_Static_assert(sizeof(wifi_sta_config_t) == 132, "station configuration size");
_Static_assert(__alignof__(wifi_sta_config_t) == 4, "station configuration alignment");
_Static_assert(offsetof(wifi_sta_config_t, pmf_cfg) == 124, "station flags follow PMF");
_Static_assert(sizeof(wifi_sta_info_t) == 12, "station info size");
_Static_assert(__alignof__(wifi_sta_info_t) == 4, "station info alignment");
_Static_assert(sizeof(wifi_sta_list_t) == 124, "station list size");
_Static_assert(offsetof(wifi_sta_list_t, num) == 120, "station list count offset");
_Static_assert(sizeof(system_event_info_t) == 44, "event union size");
_Static_assert(__alignof__(system_event_info_t) == 4, "event union alignment");
_Static_assert(sizeof(system_event_t) == 48, "event size");
_Static_assert(offsetof(system_event_t, event_info) == 4, "event payload offset");

_Static_assert(sizeof(wifi_pkt_rx_ctrl_t) == 12, "RX control size");
_Static_assert(__alignof__(wifi_pkt_rx_ctrl_t) == 4, "RX control alignment");
_Static_assert(sizeof(wifi_promiscuous_pkt_t) == 12, "RX packet size");
_Static_assert(__alignof__(wifi_promiscuous_pkt_t) == 4, "RX packet alignment");
_Static_assert(offsetof(wifi_promiscuous_pkt_t, payload) == 12, "RX payload offset");

const wifi_ap_record_t abi_ap_record
    __attribute__((used, section(".rodata.abi_ap_record"))) = { .wps = 1 };
const wifi_sta_config_t abi_sta_config
    __attribute__((used, section(".rodata.abi_sta_config"))) = {
    .rm_enabled = 1, .btm_enabled = 1,
};
/* Preserve the header's byte-6 image. The retained libnet80211.a disagrees:
 * ieee80211_api.o:esp_wifi_ap_get_sta_list stores another byte at offset 6
 * (instruction 0xa5) and PHY flags at offset 7 (0xd9). Rust follows that
 * binary ABI; see vendor/README.md. Do not treat this image as driver output. */
const wifi_sta_info_t abi_sta_info
    __attribute__((used, section(".rodata.abi_sta_info"))) = {
    .phy_11b = 1, .phy_11g = 1, .phy_11n = 1,
};

const wifi_pkt_rx_ctrl_t abi_rx_control
    __attribute__((used, section(".rodata.abi_rx_control"))) = {
    .rssi = -42, .rate = 3, .is_group = 1, .sig_mode = 3,
    .legacy_length = 0x234, .damatch1 = 1, .bssidmatch1 = 1,
    .mcs = 5, .cwb = 1, .HT_length = 0x5678,
    .aggregation = 1, .stbc = 3, .sgi = 1,
    .rxend_state = 7, .ampdu_cnt = 3, .channel = 11, .noise_floor = -95,
};
