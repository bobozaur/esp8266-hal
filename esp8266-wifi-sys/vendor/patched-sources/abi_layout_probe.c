#include <stddef.h>
#include "lwip/netif.h"
#include "lwip/pbuf.h"

#define LAYOUT_VALUE(name, value) char name[(value) + 1]

LAYOUT_VALUE(abi_sizeof_pbuf, sizeof(struct pbuf));
LAYOUT_VALUE(abi_alignof_pbuf, __alignof__(struct pbuf));
LAYOUT_VALUE(abi_offsetof_pbuf_l2_owner, offsetof(struct pbuf, l2_owner));
LAYOUT_VALUE(abi_offsetof_pbuf_l2_buf, offsetof(struct pbuf, l2_buf));

LAYOUT_VALUE(abi_sizeof_netif, sizeof(struct netif));
LAYOUT_VALUE(abi_alignof_netif, __alignof__(struct netif));
LAYOUT_VALUE(abi_offsetof_netif_state, offsetof(struct netif, state));
LAYOUT_VALUE(abi_offsetof_netif_client_data, offsetof(struct netif, client_data));
LAYOUT_VALUE(abi_offsetof_netif_dhcps_pcb, offsetof(struct netif, dhcps_pcb));
LAYOUT_VALUE(abi_offsetof_netif_mtu, offsetof(struct netif, mtu));
LAYOUT_VALUE(abi_offsetof_netif_hwaddr, offsetof(struct netif, hwaddr));
LAYOUT_VALUE(abi_offsetof_netif_igmp_mac_filter, offsetof(struct netif, igmp_mac_filter));
LAYOUT_VALUE(abi_offsetof_netif_loop_first, offsetof(struct netif, loop_first));
LAYOUT_VALUE(abi_offsetof_netif_l2_buffer_free_notify,
             offsetof(struct netif, l2_buffer_free_notify));
LAYOUT_VALUE(abi_offsetof_netif_last_ip_addr, offsetof(struct netif, last_ip_addr));
