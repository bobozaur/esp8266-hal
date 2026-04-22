#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "esp_err.h"
#include "lwip/ip4_addr.h"
#include "lwip/netif.h"
#include "lwip/tcpip.h"
#include "netif/etharp.h"

static bool s_tcpip_inited = false;
static bool s_ap_netif_added = false;
static struct netif *s_ap_netif = NULL;

static err_t esp8266_hal_ap_linkoutput(struct netif *netif, struct pbuf *p)
{
    LWIP_UNUSED_ARG(netif);
    LWIP_UNUSED_ARG(p);
    return ERR_OK;
}

void dhcps_coarse_tmr(void)
{
}

static err_t esp8266_hal_ap_netif_init_fn(struct netif *netif)
{
    if (netif == NULL) {
        return ERR_ARG;
    }

    netif->name[0] = 'a';
    netif->name[1] = 'p';
    netif->output = etharp_output;
    netif->linkoutput = esp8266_hal_ap_linkoutput;
    netif->hwaddr_len = NETIF_MAX_HWADDR_LEN;
    netif->mtu = 1500;
    netif->flags = NETIF_FLAG_BROADCAST | NETIF_FLAG_ETHARP | NETIF_FLAG_ETHERNET;

    return ERR_OK;
}

esp_err_t esp8266_hal_ap_netif_init(void)
{
    if (!s_tcpip_inited) {
        tcpip_init(NULL, NULL);
        s_tcpip_inited = true;
    }

    return ESP_OK;
}

esp_err_t esp8266_hal_ap_netif_start(const uint8_t *mac)
{
    ip4_addr_t ip;
    ip4_addr_t gw;
    ip4_addr_t netmask;

    if (mac == NULL) {
        return ESP_ERR_INVALID_ARG;
    }

    esp8266_hal_ap_netif_init();

    if (s_ap_netif == NULL) {
        s_ap_netif = calloc(1, sizeof(*s_ap_netif));
        if (s_ap_netif == NULL) {
            return ESP_ERR_NO_MEM;
        }
    }

    memcpy(s_ap_netif->hwaddr, mac, NETIF_MAX_HWADDR_LEN);

    IP4_ADDR(&ip, 192, 168, 4, 1);
    IP4_ADDR(&gw, 192, 168, 4, 1);
    IP4_ADDR(&netmask, 255, 255, 255, 0);

    if (!s_ap_netif_added) {
        if (netif_add(
                s_ap_netif,
                &ip,
                &netmask,
                &gw,
                NULL,
                esp8266_hal_ap_netif_init_fn,
                tcpip_input
            ) == NULL) {
            return ESP_FAIL;
        }
        s_ap_netif_added = true;
    }

    netif_set_addr(s_ap_netif, &ip, &netmask, &gw);
    netif_set_link_up(s_ap_netif);
    netif_set_up(s_ap_netif);

    return ESP_OK;
}

esp_err_t esp8266_hal_ap_netif_stop(void)
{
    if (s_ap_netif_added && s_ap_netif != NULL) {
        netif_set_link_down(s_ap_netif);
        netif_set_down(s_ap_netif);
        netif_remove(s_ap_netif);
        s_ap_netif_added = false;
    }

    if (s_ap_netif != NULL) {
        free(s_ap_netif);
        s_ap_netif = NULL;
    }

    return ESP_OK;
}
