/* Compile with the pinned target compiler and headers. The byte image cross-
 * checks src/lib.rs's manually maintained packed-bitfield representation. */
#include <stddef.h>
#include "esp_wifi_types.h"

_Static_assert(sizeof(wifi_pkt_rx_ctrl_t) == 12, "RX control size");
_Static_assert(__alignof__(wifi_pkt_rx_ctrl_t) == 4, "RX control alignment");
_Static_assert(sizeof(wifi_promiscuous_pkt_t) == 12, "RX packet size");
_Static_assert(__alignof__(wifi_promiscuous_pkt_t) == 4, "RX packet alignment");
_Static_assert(offsetof(wifi_promiscuous_pkt_t, payload) == 12, "RX payload offset");

const wifi_pkt_rx_ctrl_t abi_rx_control
    __attribute__((used, section(".rodata.abi_rx_control"))) = {
    .rssi = -42, .rate = 3, .is_group = 1, .sig_mode = 3,
    .legacy_length = 0x234, .damatch1 = 1, .bssidmatch1 = 1,
    .mcs = 5, .cwb = 1, .HT_length = 0x5678,
    .aggregation = 1, .stbc = 3, .sgi = 1,
    .rxend_state = 7, .ampdu_cnt = 3, .channel = 11, .noise_floor = -95,
};
