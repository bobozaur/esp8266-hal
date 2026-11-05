// Copyright 2019 Espressif Systems (Shanghai) PTE LTD
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// Modified by esp8266-hal for its vendored Rust-first Wi-Fi runtime. See
// ../README.md for the pinned source and reproduction details.

#line 15
#include <string.h>

#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "freertos/queue.h"
#include "freertos/semphr.h"

#include "esp_err.h"
#include "esp_wifi.h"

#include "utils/includes.h"
#include "utils/common.h"
#include "utils/wpa_debug.h"
#include "common/wpa_ctrl.h"
#include "common/eapol_common.h"
#include "common/ieee802_11_defs.h"
#include "utils/state_machine.h"
#include "rsn_supp/wpa.h"

#include "crypto/crypto.h"

#include "utils/ext_password.h"
#include "tls/tls.h"
#include "eap_peer/eap_i.h"
#include "eap_peer/eap_config.h"
#include "eap_peer/eap.h"
#include "eap_peer/eap_tls.h"
#ifdef EAP_PEER_METHOD
#include "eap_peer/eap_methods.h"
#endif

#include "esp_wifi_driver.h"
#include "esp_private/wifi.h"
#include "esp_wpa_err.h"

/* The bounded crypto configuration fits 4 KiB; keep the unused SDK stack
 * allowance available to certificate and MPI allocations instead. */
#undef WPA2_TASK_STACK_SIZE
#define WPA2_TASK_STACK_SIZE 4096

#define WPA2_VERSION    "v2.0"

#define WPA2_STATE_LOCK() taskENTER_CRITICAL()
#define WPA2_STATE_UNLOCK() taskEXIT_CRITICAL()

static struct eap_sm *gEapSm = NULL;

static int eap_peer_sm_init(void);
static void eap_peer_sm_deinit(void);
static int eap_peer_sm_deinit_checked(void);
esp_err_t esp_wifi_sta_wpa2_ent_disable_fn(void *param);

static int eap_sm_rx_eapol_internal(u8 *src_addr, u8 *buf, u32 len, uint8_t *bssid);
static int wpa2_start_eapol_internal(void);
int wpa2_post(uint32_t sig, uint32_t par);

#ifdef USE_WPA2_TASK
static void *s_wpa2_task_hdl = NULL;
static void *s_wpa2_queue = NULL;
static wpa2_state_t s_wpa2_state = WPA2_STATE_DISABLED;
static void *s_wpa2_api_lock = NULL;
static void *s_wifi_wpa2_sync_sem = NULL;
static bool s_disable_time_check = true;
/* Admission and ownership are protected by short critical sections. Native
 * timer/association callbacks can run with scheduling suspended. */
static bool s_wpa2_closing;
static bool s_wpa2_cleanup_active;
static unsigned s_wpa2_inflight;

static bool wpa2_api_lock(void)
{
    if (s_wpa2_api_lock == NULL) {
        s_wpa2_api_lock = xSemaphoreCreateRecursiveMutex();
        if (!s_wpa2_api_lock) {
            wpa_printf(MSG_ERROR, "WPA2: failed to create wpa2 api lock");
            return false;
        }
    }

    return xSemaphoreTakeRecursive(s_wpa2_api_lock, portMAX_DELAY) == pdPASS;
}

static void wpa2_api_unlock(void)
{
    if (s_wpa2_api_lock) {
        xSemaphoreGiveRecursive(s_wpa2_api_lock);
    }
}

static bool inline wpa2_is_enabled(void)
{
    return (s_wpa2_state == WPA2_STATE_ENABLED);
}

static bool inline wpa2_is_disabled(void)
{
    return (s_wpa2_state == WPA2_STATE_DISABLED);
}

static void inline wpa2_set_state(wpa2_state_t state)
{
    s_wpa2_state = state;
}

static void wpa2_set_eap_state(wpa2_ent_eap_state_t state)
{
    if (!gEapSm) {
        return;
    }

    gEapSm->finish_state = state;
    esp_wifi_set_wpa2_ent_state_internal(state);
}

static bool wpa2_producer_enter(void)
{
    bool admitted;
    WPA2_STATE_LOCK();
    admitted = gEapSm && !s_wpa2_closing && !s_wpa2_cleanup_active;
    if (admitted)
        s_wpa2_inflight++;
    WPA2_STATE_UNLOCK();
    return admitted;
}

static void wpa2_producer_leave(void)
{
    WPA2_STATE_LOCK();
    s_wpa2_inflight--;
    WPA2_STATE_UNLOCK();
}

#define WPA_ADDR_LEN 6
struct wpa2_rx_param {
    uint8_t *bssid;
    u8 sa[WPA_ADDR_LEN];
    u8 *buf;
    int len;
    STAILQ_ENTRY(wpa2_rx_param) bqentry;
};
static STAILQ_HEAD(, wpa2_rx_param) s_wpa2_rxq;

static void wpa2_rxq_init(void)
{
    WPA2_STATE_LOCK();
    STAILQ_INIT(&s_wpa2_rxq);
    WPA2_STATE_UNLOCK();
}

static void wpa2_rxq_enqueue(struct wpa2_rx_param *param)
{
    WPA2_STATE_LOCK();
    STAILQ_INSERT_TAIL(&s_wpa2_rxq,param, bqentry);
    WPA2_STATE_UNLOCK();
}

static struct wpa2_rx_param * wpa2_rxq_dequeue(void)
{
    struct wpa2_rx_param *param = NULL;
    WPA2_STATE_LOCK();
    if ((param = STAILQ_FIRST(&s_wpa2_rxq)) != NULL) {
        STAILQ_REMOVE_HEAD(&s_wpa2_rxq, bqentry);
        STAILQ_NEXT(param,bqentry) = NULL;
    }
    WPA2_STATE_UNLOCK();
    return param;
}

static void wpa2_rxq_discard(struct wpa2_rx_param *param)
{
    struct wpa2_rx_param *entry;
    bool found = false;

    WPA2_STATE_LOCK();
    STAILQ_FOREACH(entry, &s_wpa2_rxq, bqentry) {
        if (entry == param) {
            STAILQ_REMOVE(&s_wpa2_rxq, param, wpa2_rx_param, bqentry);
            found = true;
            break;
        }
    }
    WPA2_STATE_UNLOCK();
    /* A worker may already have taken it while the producer was posting. */
    if (found) {
        os_free(param->buf);
        os_free(param);
    }
}

static void wpa2_rxq_deinit(void)
{
    struct wpa2_rx_param *param;
    while ((param = wpa2_rxq_dequeue()) != NULL) {
        os_free(param->buf);
        os_free(param);
    }
}

void wpa2_task(void *pvParameters)
{
    ETSEvent event;
    struct eap_sm *sm = pvParameters;

    for (;;) {
        if (xQueueReceive(s_wpa2_queue, &event, portMAX_DELAY) != pdPASS)
            continue;
        if (event.sig == SIG_WPA2_TASK_DEL)
            break;
        if (event.sig >= SIG_WPA2_MAX)
            continue;

        WPA2_STATE_LOCK();
        sm->wpa2_sig_cnt[event.sig]--;
        WPA2_STATE_UNLOCK();
        switch (event.sig) {
        case SIG_WPA2_START:
            wpa2_start_eapol_internal();
            break;
        case SIG_WPA2_RX: {
            struct wpa2_rx_param *param;
            while ((param = wpa2_rxq_dequeue()) != NULL) {
                eap_sm_rx_eapol_internal(param->sa, param->buf,
                                         param->len, param->bssid);
                os_free(param->buf);
                os_free(param);
            }
            break;
        }
        default:
            break;
        }
        /* Only ordinary posters consume this completion. DEL has no waiter
         * here, so it cannot steal a still-returning RX caller's completion. */
        xSemaphoreGive(s_wifi_wpa2_sync_sem);
    }

    /* No queue, peer or semaphore access after publishing exit. A higher
     * priority cleanup caller may now free them before this task deletes itself. */
    WPA2_STATE_LOCK();
    s_wpa2_task_hdl = NULL;
    WPA2_STATE_UNLOCK();
    vTaskDelete(NULL);
}

int wpa2_post(uint32_t sig, uint32_t par)
{
    ETSEvent event = { .sig = sig, .par = par };
    struct eap_sm *sm;
    int ret = ESP_FAIL;

    if (sig >= SIG_WPA2_TASK_DEL || !wpa2_producer_enter())
        return ESP_FAIL;
    WPA2_STATE_LOCK();
    sm = gEapSm;
    if (s_wpa2_closing) {
        WPA2_STATE_UNLOCK();
        goto out;
    }
    if (sm->wpa2_sig_cnt[sig]) {
        WPA2_STATE_UNLOCK();
        ret = ESP_OK;
        goto out;
    }
    sm->wpa2_sig_cnt[sig]++;
    /* At most one pending START and RX, plus DEL, fit the three-entry queue.
     * Admission and enqueue are atomic with respect to teardown. */
    if (xQueueSend(s_wpa2_queue, &event, 0) != pdPASS) {
        sm->wpa2_sig_cnt[sig]--;
        WPA2_STATE_UNLOCK();
        goto out;
    }
    WPA2_STATE_UNLOCK();
    if (xSemaphoreTake(s_wifi_wpa2_sync_sem, portMAX_DELAY) == pdPASS)
        ret = ESP_OK;
out:
    wpa2_producer_leave();
    return ret;
}

#endif /* USE_WPA2_TASK */

static void wpa2_sendto_wrapper(void *buffer, uint16_t len)
{
    esp_wifi_internal_tx(WIFI_IF_STA, buffer, len);
}

static inline int wpa2_sm_ether_send(struct eap_sm *sm, const u8 *dest, u16 proto,
                                     const u8 *data, size_t data_len)
{
    void *buffer = (void *)(data - sizeof(struct l2_ethhdr));
    struct l2_ethhdr *eth = NULL;

    if (!buffer) {
        wpa_printf(MSG_ERROR, "wpa2: invalid data");
        return ESP_FAIL;
    } else {
        eth = (struct l2_ethhdr *)buffer;
        memcpy(eth->h_dest, dest, ETH_ALEN);
        memcpy(eth->h_source, sm->ownaddr, ETH_ALEN);
        eth->h_proto = host_to_be16(proto);
        wpa2_sendto_wrapper(buffer, sizeof(struct l2_ethhdr) + data_len);
    }

    return ESP_OK;
}

u8 *wpa2_sm_alloc_eapol(struct eap_sm *sm, u8 type,
                        const void *data, u16 data_len,
                        size_t *msg_len, void **data_pos)
{
    void *buffer;
    struct ieee802_1x_hdr *hdr;

    *msg_len = sizeof(struct ieee802_1x_hdr) + data_len;
    /* XXX: reserve l2_ethhdr is enough */
    buffer = os_malloc(*msg_len + sizeof(struct l2_ethhdr));

    if (buffer == NULL) {
        return NULL;
    }

    hdr = (struct ieee802_1x_hdr *)((char *)buffer + sizeof(struct l2_ethhdr));
    hdr->version = 0x01;
    hdr->type = type;
    hdr->length = host_to_be16(data_len);

    if (data) {
        memcpy(hdr + 1, data, data_len);
    } else {
        memset(hdr + 1, 0, data_len);
    }

    if (data_pos) {
        *data_pos = hdr + 1;
    }

    return (u8 *) hdr;
}


void wpa2_sm_free_eapol(u8 *buffer)
{
    if (buffer != NULL) {
        buffer = buffer - sizeof(struct l2_ethhdr);
        os_free(buffer);
    }

}

int eap_sm_send_eapol(struct eap_sm *sm, struct wpabuf *resp)
{
    size_t outlen;
    int ret;
    u8 *outbuf = NULL;
    int stack_backed = 0;
    u8 stack_eapol[sizeof(struct l2_ethhdr) + sizeof(struct ieee802_1x_hdr) + 512];

    u8 bssid[6];
    ret = esp_wifi_get_assoc_bssid_internal(bssid);

    if (ret != 0) {
        wpa_printf(MSG_DEBUG, "bssid is empty \n");
        return WPA_ERR_INVALID_BSSID;
    }

    outbuf = wpa2_sm_alloc_eapol(sm, IEEE802_1X_TYPE_EAP_PACKET,
                                 wpabuf_head_u8(resp), wpabuf_len(resp),
                                 &outlen, NULL);
    if (!outbuf && wpabuf_len(resp) <= 512) {
        struct ieee802_1x_hdr *hdr;
        hdr = (struct ieee802_1x_hdr *)(stack_eapol + sizeof(struct l2_ethhdr));
        outlen = sizeof(struct ieee802_1x_hdr) + wpabuf_len(resp);
        hdr->version = 0x01;
        hdr->type = IEEE802_1X_TYPE_EAP_PACKET;
        hdr->length = host_to_be16(wpabuf_len(resp));
        memcpy(hdr + 1, wpabuf_head_u8(resp), wpabuf_len(resp));
        outbuf = (u8 *)hdr;
        stack_backed = 1;
    }
    if (!outbuf) {
        return ESP_ERR_NO_MEM;
    }

    ret = wpa2_sm_ether_send(sm, bssid, ETH_P_EAPOL, outbuf, outlen);
    if (!stack_backed) {
        wpa2_sm_free_eapol(outbuf);
    }
    if (ret) {
        return ESP_FAIL;
    }

    return ESP_OK;
}

int eap_sm_process_request(struct eap_sm *sm, struct wpabuf *reqData)
{
    size_t plen;
    u32 reqVendor, reqVendorMethod;
    u8 type, *pos;
    struct eap_hdr *ehdr;
    const struct eap_method *m = NULL;
    struct wpabuf *resp = NULL;
    struct eap_method_ret m_res;
    int ret = 0;

    if (reqData == NULL || wpabuf_len(reqData) < sizeof(*ehdr)) {
        return ESP_ERR_INVALID_ARG;
    }

    ehdr = (struct eap_hdr *)wpabuf_head(reqData);
    plen = be_to_host16(ehdr->length);
    if (plen < sizeof(*ehdr) + 1 || plen > wpabuf_len(reqData)) {
        return ESP_FAIL;
    }

    if (ehdr->identifier == sm->current_identifier) {
        /*Retransmit*/
        resp = sm->lastRespData;
        goto send_resp;
    }

    sm->current_identifier = ehdr->identifier;

    pos = (u8 *)(ehdr + 1);
    type = *pos++;
    if (type == EAP_TYPE_IDENTITY) {
        resp = (struct wpabuf *)eap_sm_build_identity_resp(sm, ehdr->identifier, 0);
        goto send_resp;
    } else if (type == EAP_TYPE_NOTIFICATION) {
        /*Ignore*/
        goto out;
    } else if (type == EAP_TYPE_EXPANDED) {
        if (plen < sizeof(*ehdr) + 8) {
            return ESP_FAIL;
        }
        reqVendor = WPA_GET_BE24(pos);
        pos += 3;
        reqVendorMethod = WPA_GET_BE32(pos);
    } else {
        reqVendor = EAP_VENDOR_IETF;
        reqVendorMethod = type;
    }

    if (sm->m && sm->m->process && sm->eap_method_priv &&
            reqVendor == sm->m->vendor &&
            reqVendorMethod == sm->m->method) {
        resp = sm->m->process(sm, sm->eap_method_priv,
                              &m_res, reqData);
    } else {
        m = eap_peer_get_eap_method(reqVendor, reqVendorMethod);
        if (m == NULL) {
            goto build_nak;
        }
        if (sm->m) {
            eap_deinit_prev_method(sm, "GET_METHOD");
        }
        sm->m = m;
        sm->eap_method_priv = sm->m->init(sm);
        if (sm->eap_method_priv == NULL) {
            wpa_printf(MSG_ERROR, "Method private structure allocated failure\n");
            sm->m = NULL;
            goto build_nak;
        }

        if (sm->m->process) {
            resp = sm->m->process(sm, sm->eap_method_priv, &m_res, reqData);
        }
    }

    if (sm->m->isKeyAvailable && sm->m->getKey &&
            sm->m->isKeyAvailable(sm, sm->eap_method_priv)) {
        if (sm->eapKeyData) {
            os_free(sm->eapKeyData);
        }
        sm->eapKeyData = sm->m->getKey(sm, sm->eap_method_priv,
                                       &sm->eapKeyDataLen);
    }
    goto send_resp;

build_nak:
    resp = (struct wpabuf *)eap_sm_build_nak(sm, type, ehdr->identifier);
    if (resp == NULL) {
        return ESP_FAIL;
    }
    ret = ESP_FAIL;

send_resp:
    if (resp == NULL) {
        wpa_printf(MSG_ERROR, "Response build fail, return.");
        return ESP_FAIL;
    }
    ret = eap_sm_send_eapol(sm, resp);
    if (ret == ESP_OK) {
        if (resp != sm->lastRespData) {
            wpabuf_free(sm->lastRespData);
            sm->lastRespData = resp;
        }
    } else {
        if (resp != sm->lastRespData) {
            wpabuf_free(resp);
        }
        wpabuf_free(sm->lastRespData);
        sm->lastRespData = NULL;

        if (ret == WPA_ERR_INVALID_BSSID) {
            ret = WPA2_ENT_EAP_STATE_FAIL;
            wpa2_set_eap_state(WPA2_ENT_EAP_STATE_FAIL);
        }
    }
out:
    return ret;
}

static int eap_sm_rx_eapol(u8 *src_addr, u8 *buf, u32 len, uint8_t *bssid)
{
#ifdef USE_WPA2_TASK
    struct wpa2_rx_param *param;
    int ret = ESP_ERR_NO_MEM;

    /* Cover allocation/enqueue as well as the synchronous completion wait. */
    if (!wpa2_producer_enter())
        return ESP_FAIL;
    param = os_zalloc(sizeof(*param));
    if (!param)
        goto out;
    param->buf = os_zalloc(len);
    if (!param->buf) {
        os_free(param);
        goto out;
    }
    param->bssid = bssid;
    memcpy(param->buf, buf, len);
    param->len = len;
    memcpy(param->sa, src_addr, WPA_ADDR_LEN);
    wpa2_rxq_enqueue(param);
    ret = wpa2_post(SIG_WPA2_RX, 0);
    if (ret != ESP_OK)
        wpa2_rxq_discard(param);
out:
    wpa2_producer_leave();
    return ret;
#else
    return eap_sm_rx_eapol_internal(src_addr, buf, len, bssid);
#endif
}

static int wpa2_ent_rx_eapol(u8 *src_addr, u8 *buf, u32 len, uint8_t *bssid)
{
    struct ieee802_1x_hdr *hdr;
    int ret = ESP_OK;

    if (!buf || len < sizeof(*hdr))
        return ESP_ERR_INVALID_ARG;
    hdr = (struct ieee802_1x_hdr *) buf;

    switch (hdr->type) {
	    case IEEE802_1X_TYPE_EAPOL_START:
	    case IEEE802_1X_TYPE_EAP_PACKET:
	    case IEEE802_1X_TYPE_EAPOL_LOGOFF:
		    ret = eap_sm_rx_eapol(src_addr, buf, len, bssid);
		    break;
	    case IEEE802_1X_TYPE_EAPOL_KEY:
            ret = wpa_sm_rx_eapol(src_addr, buf, len);
		    break;
	    default:
		wpa_printf(MSG_ERROR, "Unknown EAPOL packet type - %d\n", hdr->type);
		    break;
    }

	return ret;
}

static int eap_sm_rx_eapol_internal(u8 *src_addr, u8 *buf, u32 len, uint8_t *bssid)
{
    struct eap_sm *sm = gEapSm;
    u32 plen, data_len;
    struct ieee802_1x_hdr *hdr;
    struct eap_hdr *ehdr;
    struct wpabuf req;
    u8 *tmp;
    int ret = ESP_FAIL;

    if (!sm) {
        return ESP_FAIL;
    }

    if (len < sizeof(*hdr) + sizeof(*ehdr)) {
#ifdef DEBUG_PRINT
        wpa_printf(MSG_DEBUG, "WPA: EAPOL frame too short to be a WPA "
                   "EAPOL-Key (len %lu, expecting at least %lu)",
                   (unsigned long) len,
                   (unsigned long) sizeof(*hdr) + sizeof(*ehdr));
#endif
        return ESP_FAIL;
    }

    tmp = buf;

    hdr = (struct ieee802_1x_hdr *) tmp;
    ehdr = (struct eap_hdr *) (hdr + 1);
    plen = be_to_host16(hdr->length);
    data_len = plen + sizeof(*hdr);

#ifdef DEBUG_PRINT
    wpa_printf(MSG_DEBUG, "IEEE 802.1X RX: version=%d type=%d length=%d\n",
               hdr->version, hdr->type, plen);
#endif
    if (hdr->version < EAPOL_VERSION) {
        /* TODO: backwards compatibility */
    }
    if (hdr->type != IEEE802_1X_TYPE_EAP_PACKET) {
#ifdef DEBUG_PRINT
        wpa_printf(MSG_DEBUG, "WPA2: EAP frame (type %u) discarded, "
                   "not a EAP PACKET frame", hdr->type);
#endif
        ret = -2;
        goto _out;
    }
    if (plen > len - sizeof(*hdr) || plen < sizeof(*ehdr)) {
#ifdef DEBUG_PRINT
        wpa_printf(MSG_DEBUG, "WPA2: EAPOL frame payload size %lu "
                   "invalid (frame size %lu)",
                   (unsigned long) plen, (unsigned long) len);
#endif
        ret = -2;
        goto _out;
    }

    wpa_hexdump(MSG_MSGDUMP, "WPA2: RX EAPOL-EAP PACKET", tmp, len);

    if (data_len < len) {
#ifdef DEBUG_PRINT
        wpa_printf(MSG_DEBUG, "WPA: ignoring %lu bytes after the IEEE "
                   "802.1X data\n", (unsigned long) len - data_len);
#endif
    }

#ifdef EAP_PEER_METHOD
    switch (ehdr->code) {
    case EAP_CODE_REQUEST:
        /* Handle EAP-reauthentication case */
        if (sm->finish_state == WPA2_ENT_EAP_STATE_SUCCESS) {
                wpa_printf(MSG_INFO, ">>>>>wpa2 EAP Re-authentication in progress\n");
		wpa2_set_eap_state(WPA2_ENT_EAP_STATE_IN_PROGRESS);
	}

        /* The WPA2 worker owns buf until this synchronous request returns. */
        wpabuf_set(&req, ehdr, plen);
        ret = eap_sm_process_request(sm, &req);
        break;
    case EAP_CODE_RESPONSE:
        /*Ignore*/
        break;
    case EAP_CODE_SUCCESS:
        if (sm->eapKeyData) {
            wpa_set_pmk(sm->eapKeyData, NULL, false);
            os_free(sm->eapKeyData);
            sm->eapKeyData = NULL;
            wpa_printf(MSG_INFO, ">>>>>wpa2 FINISH\n");
            ret = WPA2_ENT_EAP_STATE_SUCCESS;
            wpa2_set_eap_state(WPA2_ENT_EAP_STATE_SUCCESS);
	    eap_deinit_prev_method(sm, "EAP Success");
        } else {
            wpa_printf(MSG_INFO, ">>>>>wpa2 FAILED, receive EAP_SUCCESS but pmk is empty, potential attack!\n");
            ret = WPA2_ENT_EAP_STATE_FAIL;
            wpa2_set_eap_state(WPA2_ENT_EAP_STATE_FAIL);
        }
        break;
    case EAP_CODE_FAILURE:
        wpa_printf(MSG_INFO, ">>>>>wpa2 FAILED\n");
        ret = WPA2_ENT_EAP_STATE_FAIL;
        wpa2_set_eap_state(WPA2_ENT_EAP_STATE_FAIL);
        break;
    }
#endif
_out:
    return ret;
}

static int wpa2_start_eapol(void)
{
#ifdef USE_WPA2_TASK
    return wpa2_post(SIG_WPA2_START, 0);
#else
    return wpa2_start_eapol_internal();
#endif
}

static int wpa2_start_eapol_internal(void)
{
    struct eap_sm *sm = gEapSm;
    int ret = 0;
    u8 bssid[6];
    u8 *buf;
    size_t len;

    if (!sm) {
        return ESP_FAIL;
    }

    if (wpa_sta_cur_pmksa_matches_akm()) {
        wpa_printf(MSG_DEBUG,
                "RSN: PMKSA caching - do not send EAPOL-Start");
        return ESP_FAIL;
    }

    ret = esp_wifi_get_assoc_bssid_internal(bssid);
    if (ret != 0) {
        wpa_printf(MSG_ERROR, "bssid is empty!");
        return WPA_ERR_INVALID_BSSID;
    }

    buf = wpa2_sm_alloc_eapol(sm, IEEE802_1X_TYPE_EAPOL_START, (u8 *)"", 0, &len, NULL);
    if (!buf) {
        return ESP_FAIL;
    }

    wpa2_set_eap_state(WPA2_ENT_EAP_STATE_IN_PROGRESS);
    wpa2_sm_ether_send(sm, bssid, ETH_P_EAPOL, buf, len);
    wpa2_sm_free_eapol(buf);
    return ESP_OK;
}

/* Stop admission first. DEL owns no allocation and does not consume the normal
 * poster's completion semaphore. Suspended native callbacks may request exit,
 * but must leave the peer owned until the worker and its callers can run. */
static int eap_peer_sm_deinit_checked(void)
{
    ETSEvent event = { .sig = SIG_WPA2_TASK_DEL, .par = 0 };
    struct eap_sm *sm;
    bool finished;

    WPA2_STATE_LOCK();
    if (s_wpa2_cleanup_active) {
        WPA2_STATE_UNLOCK();
        return ESP_ERR_INVALID_STATE;
    }
    sm = gEapSm;
    if (!sm) {
        WPA2_STATE_UNLOCK();
        return ESP_OK;
    }
    if (xTaskGetCurrentTaskHandle() == s_wpa2_task_hdl) {
        WPA2_STATE_UNLOCK();
        return ESP_ERR_INVALID_STATE;
    }
    s_wpa2_closing = true;
    if (s_wpa2_task_hdl && !sm->wpa2_sig_cnt[SIG_WPA2_TASK_DEL]) {
        sm->wpa2_sig_cnt[SIG_WPA2_TASK_DEL] = 1;
        if (xQueueSend(s_wpa2_queue, &event, 0) != pdPASS) {
            sm->wpa2_sig_cnt[SIG_WPA2_TASK_DEL] = 0;
            WPA2_STATE_UNLOCK();
            return ESP_FAIL;
        }
    }
    finished = !s_wpa2_task_hdl && s_wpa2_inflight == 0;
    if (!finished && xTaskGetSchedulerState() != taskSCHEDULER_RUNNING) {
        WPA2_STATE_UNLOCK();
        return ESP_ERR_INVALID_STATE;
    }
    s_wpa2_cleanup_active = true;
    WPA2_STATE_UNLOCK();

    while (!finished) {
        vTaskDelay(1);
        WPA2_STATE_LOCK();
        finished = !s_wpa2_task_hdl && s_wpa2_inflight == 0;
        WPA2_STATE_UNLOCK();
    }

    /* No worker or returning producer can observe these owners now. */
    wpa2_rxq_deinit();
    eap_deinit_prev_method(sm, "EAP deinit");
    eap_sm_abort(sm);
    eap_peer_config_deinit(sm);
    eap_peer_blob_deinit(sm);
    tls_deinit(sm->ssl_ctx);
    vQueueDelete(s_wpa2_queue);
    vSemaphoreDelete(s_wifi_wpa2_sync_sem);
    WPA2_STATE_LOCK();
    gEapSm = NULL;
    s_wpa2_queue = NULL;
    s_wifi_wpa2_sync_sem = NULL;
    s_wpa2_cleanup_active = false;
    s_wpa2_closing = wpa2_is_disabled();
    WPA2_STATE_UNLOCK();
    os_free(sm);
    return ESP_OK;
}

static int eap_peer_sm_init(void)
{
    struct eap_sm *sm = NULL;
    void *queue = NULL;
    void *completion = NULL;
    int ret;

    if (!wpa2_is_enabled())
        return ESP_ERR_INVALID_STATE;
    ret = eap_peer_sm_deinit_checked();
    if (ret != ESP_OK)
        return ret;
    WPA2_STATE_LOCK();
    if (!wpa2_is_enabled() || s_wpa2_cleanup_active || gEapSm) {
        WPA2_STATE_UNLOCK();
        return ESP_ERR_INVALID_STATE;
    }
    s_wpa2_cleanup_active = true;
    s_wpa2_closing = true;
    WPA2_STATE_UNLOCK();

    sm = os_zalloc(sizeof(*sm));
    if (!sm)
        goto no_mem;
    sm->current_identifier = 0xff;
    sm->finish_state = WPA2_ENT_EAP_STATE_NOT_START;
    ret = esp_wifi_get_mac(WIFI_IF_STA, sm->ownaddr);
    if (ret != ESP_OK)
        goto fail;
    if (eap_peer_blob_init(sm) != 0 ||
        eap_peer_config_init(sm, g_wpa_private_key_passwd,
                             g_wpa_private_key_passwd_len) != 0)
        goto no_mem;
    sm->ssl_ctx = tls_init();
    if (!sm->ssl_ctx)
        goto no_mem;
    queue = xQueueCreate(SIG_WPA2_MAX, sizeof(ETSEvent));
    if (!queue)
        goto no_mem;
    completion = xSemaphoreCreateCounting(1, 0);
    if (!completion)
        goto no_mem;

    /* The worker can run before xTaskCreate returns. Its argument and every
     * dependency are valid, while admission remains closed until publication. */
    wpa2_rxq_init();
    WPA2_STATE_LOCK();
    s_wpa2_queue = queue;
    s_wifi_wpa2_sync_sem = completion;
    WPA2_STATE_UNLOCK();
    if (xTaskCreate(wpa2_task, "wpa2T", WPA2_TASK_STACK_SIZE, sm, 2,
                    &s_wpa2_task_hdl) != pdPASS)
        goto no_mem;
    WPA2_STATE_LOCK();
    gEapSm = sm;
    s_wpa2_closing = false;
    s_wpa2_cleanup_active = false;
    WPA2_STATE_UNLOCK();
    esp_wifi_set_wpa2_ent_state_internal(WPA2_ENT_EAP_STATE_NOT_START);
    return ESP_OK;

no_mem:
    ret = ESP_ERR_NO_MEM;
fail:
    if (sm) {
        if (sm->ssl_ctx)
            tls_deinit(sm->ssl_ctx);
        eap_peer_config_deinit(sm);
        eap_peer_blob_deinit(sm);
        os_free(sm);
    }
    if (completion)
        vSemaphoreDelete(completion);
    if (queue)
        vQueueDelete(queue);
    WPA2_STATE_LOCK();
    s_wpa2_queue = NULL;
    s_wifi_wpa2_sync_sem = NULL;
    s_wpa2_task_hdl = NULL;
    s_wpa2_cleanup_active = false;
    s_wpa2_closing = false;
    WPA2_STATE_UNLOCK();
    return ret;
}

static void eap_peer_sm_deinit(void)
{
    if (eap_peer_sm_deinit_checked() != ESP_OK)
        wpa_printf(MSG_DEBUG, "WPA2: peer cleanup pending");
}

esp_err_t esp_wifi_sta_wpa2_ent_enable_fn(void *arg)
{
    struct wpa2_funcs *wpa2_cb;

    wpa_printf(MSG_INFO, "WPA2 ENTERPRISE VERSION: [%s] enable\n",
               WPA2_VERSION);

    wpa2_cb = (struct wpa2_funcs *)os_zalloc(sizeof(struct wpa2_funcs));
    if (wpa2_cb == NULL) {
        wpa_printf(MSG_ERROR, "WPA2: no mem for wpa2 cb\n");
        return ESP_ERR_NO_MEM;
    }

    wpa2_cb->wpa2_sm_rx_eapol = wpa2_ent_rx_eapol;
    wpa2_cb->wpa2_start = wpa2_start_eapol;
    wpa2_cb->wpa2_init = eap_peer_sm_init;
    wpa2_cb->wpa2_deinit = eap_peer_sm_deinit;

#ifdef EAP_PEER_METHOD
    if (eap_peer_register_methods()) {
        eap_peer_unregister_methods();
        os_free(wpa2_cb);
        return ESP_ERR_NO_MEM;
    }
#endif
    esp_wifi_register_wpa2_cb_internal(wpa2_cb);
    wpa_printf(MSG_DEBUG, "WPA2 ENTERPRISE CRYPTO INIT.\r\n");
    return ESP_OK;
}

esp_err_t esp_wifi_sta_wpa2_ent_enable(void)
{
    wifi_wpa2_param_t param;
    esp_err_t ret;

    if (!wpa2_api_lock())
        return ESP_ERR_NO_MEM;

    if (s_wpa2_closing) {
        wpa2_api_unlock();
        return ESP_ERR_INVALID_STATE;
    }

    if (wpa2_is_enabled()) {
        wpa_printf(MSG_INFO, "WPA2: already enabled");
        wpa2_api_unlock();
        return ESP_OK;
    }

    param.fn = (wifi_wpa2_fn_t)esp_wifi_sta_wpa2_ent_enable_fn;
    param.param = NULL;

    ret = esp_wifi_sta_wpa2_ent_enable_internal(&param);

    if (ESP_OK == ret) {
        wpa2_set_state(WPA2_STATE_ENABLED);
    } else {
        /* The native wrapper sets its enabled byte before invoking our
         * callback. Roll it back even when callback allocation failed. */
        param.fn = (wifi_wpa2_fn_t)esp_wifi_sta_wpa2_ent_disable_fn;
        if (esp_wifi_sta_wpa2_ent_disable_internal(&param) != ESP_OK)
            s_wpa2_closing = true;
        wpa_printf(MSG_ERROR, "failed to enable wpa2 ret=%d", ret);
    }

    wpa2_api_unlock();

    return ret;
}

esp_err_t esp_wifi_sta_wpa2_ent_disable_fn(void *param)
{
    int ret;

    wpa_printf(MSG_INFO, "WPA2 ENTERPRISE VERSION: [%s] disable\n", WPA2_VERSION);
    ret = eap_peer_sm_deinit_checked();
    if (ret != ESP_OK)
        return ret;
    esp_wifi_unregister_wpa2_cb_internal();

#ifdef EAP_PEER_METHOD
    eap_peer_unregister_methods();
#endif

    return ESP_OK;
}

esp_err_t esp_wifi_sta_wpa2_ent_disable(void)
{
    wifi_wpa2_param_t param;
    esp_err_t ret;

    if (!wpa2_api_lock())
        return ESP_ERR_NO_MEM;

    if (wpa2_is_disabled() && !s_wpa2_closing) {
        wpa_printf(MSG_INFO, "WPA2: already disabled");
        wpa2_api_unlock();
        return ESP_OK;
    }

    /* Stop before mutating the native enabled byte. A suspended native
     * teardown can leave an owned peer pending; do not pretend it is enabled
     * or allow a new association to overwrite it. */
    WPA2_STATE_LOCK();
    wpa2_set_state(WPA2_STATE_DISABLED);
    s_wpa2_closing = true;
    WPA2_STATE_UNLOCK();
    ret = eap_peer_sm_deinit_checked();
    if (ret == ESP_OK) {
        param.fn = (wifi_wpa2_fn_t)esp_wifi_sta_wpa2_ent_disable_fn;
        param.param = 0;
        ret = esp_wifi_sta_wpa2_ent_disable_internal(&param);
        if (ret == ESP_OK)
            s_wpa2_closing = false;
    }

    wpa2_api_unlock();

    return ret;
}

esp_err_t esp_wifi_sta_wpa2_ent_set_cert_key(const unsigned char *client_cert, int client_cert_len, const unsigned char *private_key, int private_key_len, const unsigned char *private_key_passwd, int private_key_passwd_len)
{
    if (client_cert && client_cert_len > 0) {
        g_wpa_client_cert = client_cert;
        g_wpa_client_cert_len = client_cert_len;
    }
    if (private_key && private_key_len > 0) {
        g_wpa_private_key = private_key;
        g_wpa_private_key_len = private_key_len;
    }
    if (private_key_passwd && private_key_passwd_len > 0) {
        g_wpa_private_key_passwd = private_key_passwd;
        g_wpa_private_key_passwd_len = private_key_passwd_len;
    }

    return ESP_OK;
}

void esp_wifi_sta_wpa2_ent_clear_cert_key(void)
{
    esp_wifi_unregister_wpa2_cb_internal();

    g_wpa_client_cert = NULL;
    g_wpa_client_cert_len = 0;
    g_wpa_private_key = NULL;
    g_wpa_private_key_len = 0;
    g_wpa_private_key_passwd = NULL;
    g_wpa_private_key_passwd_len = 0;
}

esp_err_t esp_wifi_sta_wpa2_ent_set_ca_cert(const unsigned char *ca_cert, int ca_cert_len)
{
    if (ca_cert && ca_cert_len > 0) {
        g_wpa_ca_cert = ca_cert;
        g_wpa_ca_cert_len = ca_cert_len;
    }

    return ESP_OK;
}

void esp_wifi_sta_wpa2_ent_clear_ca_cert(void)
{
    g_wpa_ca_cert = NULL;
    g_wpa_ca_cert_len = 0;
}

#define ANONYMOUS_ID_LEN_MAX 128
esp_err_t esp_wifi_sta_wpa2_ent_set_identity(const unsigned char *identity, int len)
{
    if (len <= 0 || len > ANONYMOUS_ID_LEN_MAX) {
        return ESP_ERR_INVALID_ARG;
    }

    if (g_wpa_anonymous_identity) {
        os_free(g_wpa_anonymous_identity);
        g_wpa_anonymous_identity = NULL;
    }

    g_wpa_anonymous_identity = (u8 *)os_zalloc(len);
    if (g_wpa_anonymous_identity == NULL) {
        return ESP_ERR_NO_MEM;
    }

    os_memcpy(g_wpa_anonymous_identity, identity, len);
    g_wpa_anonymous_identity_len = len;

    return ESP_OK;
}

void esp_wifi_sta_wpa2_ent_clear_identity(void)
{
    if (g_wpa_anonymous_identity) {
        os_free(g_wpa_anonymous_identity);
    }

    g_wpa_anonymous_identity = NULL;
    g_wpa_anonymous_identity_len = 0;
}

#define USERNAME_LEN_MAX 128
esp_err_t esp_wifi_sta_wpa2_ent_set_username(const unsigned char *username, int len)
{
    if (len <= 0 || len > USERNAME_LEN_MAX) {
        return ESP_ERR_INVALID_ARG;
    }

    if (g_wpa_username) {
        os_free(g_wpa_username);
        g_wpa_username = NULL;
    }

    g_wpa_username = (u8 *)os_zalloc(len);
    if (g_wpa_username == NULL) {
        return ESP_ERR_NO_MEM;
    }

    os_memcpy(g_wpa_username, username, len);
    g_wpa_username_len = len;

    return ESP_OK;
}

void esp_wifi_sta_wpa2_ent_clear_username(void)
{
    if (g_wpa_username) {
        os_free(g_wpa_username);
    }

    g_wpa_username = NULL;
    g_wpa_username_len = 0;
}

esp_err_t esp_wifi_sta_wpa2_ent_set_password(const unsigned char *password, int len)
{
    if (len <= 0) {
        return ESP_ERR_INVALID_ARG;
    }

    if (g_wpa_password) {
        os_free(g_wpa_password);
        g_wpa_password = NULL;
    }

    g_wpa_password = (u8 *)os_zalloc(len);
    if (g_wpa_password == NULL) {
        return ESP_ERR_NO_MEM;
    }

    os_memcpy(g_wpa_password, password, len);
    g_wpa_password_len = len;

    return ESP_OK;
}

void esp_wifi_sta_wpa2_ent_clear_password(void)
{
    if (g_wpa_password) {
        os_free(g_wpa_password);
    }
    g_wpa_password = NULL;
    g_wpa_password_len = 0;
}

esp_err_t esp_wifi_sta_wpa2_ent_set_new_password(const unsigned char *new_password, int len)
{
    if (len <= 0) {
        return ESP_ERR_INVALID_ARG;
    }

    if (g_wpa_new_password) {
        os_free(g_wpa_new_password);
        g_wpa_new_password = NULL;
    }

    g_wpa_new_password = (u8 *)os_zalloc(len);
    if (g_wpa_new_password == NULL) {
        return ESP_ERR_NO_MEM;
    }

    os_memcpy(g_wpa_new_password, new_password, len);
    g_wpa_password_len = len;

    return ESP_OK;
}

void esp_wifi_sta_wpa2_ent_clear_new_password(void)
{
    if (g_wpa_new_password) {
        os_free(g_wpa_new_password);
    }
    g_wpa_new_password = NULL;
    g_wpa_new_password_len = 0;
}

esp_err_t esp_wifi_sta_wpa2_ent_set_disable_time_check(bool disable)
{
    s_disable_time_check = disable;
    return ESP_OK;
}

bool wifi_sta_get_enterprise_disable_time_check(void)
{
    return s_disable_time_check;
}

esp_err_t esp_wifi_sta_wpa2_ent_get_disable_time_check(bool *disable)
{
    *disable = wifi_sta_get_enterprise_disable_time_check();
    return ESP_OK;
}
