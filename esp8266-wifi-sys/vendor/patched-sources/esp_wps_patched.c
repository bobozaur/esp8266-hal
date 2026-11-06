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

#include "utils/includes.h"
#include "rsn_supp/wpa.h"
#include "utils/common.h"
#include "common/eapol_common.h"
#include "utils/wpa_debug.h"
#include "common/ieee802_11_defs.h"
#include "crypto/dh_group5.h"
#include "wps/wps_i.h"
#include "wps/wps_dev_attr.h"
#include "wps/wps_attr_parse.h"
#include "eap_peer/eap_defs.h"
#include "eap_peer/eap_common.h"
#include "esp_wifi_driver.h"
#undef WPS_TASK_STACK_SIZE
#define WPS_TASK_STACK_SIZE (3584 + TASK_STACK_SIZE_ADD)
static unsigned char esp8266_hal_wps_pin_override[9];
static unsigned char esp8266_hal_wps_target_ssid[32];
static unsigned char esp8266_hal_wps_target_ssid_len;
void esp8266_hal_wps_set_pin(const unsigned char *pin) {
    if (pin) {
        memcpy(esp8266_hal_wps_pin_override, pin, 8);
        esp8266_hal_wps_pin_override[8] = 0;
    } else {
        memset(esp8266_hal_wps_pin_override, 0, sizeof(esp8266_hal_wps_pin_override));
    }
}
void esp8266_hal_wps_set_target_ssid(const unsigned char *ssid, unsigned int len) {
    if (ssid && len <= sizeof(esp8266_hal_wps_target_ssid)) {
        memcpy(esp8266_hal_wps_target_ssid, ssid, len);
        esp8266_hal_wps_target_ssid_len = len;
    } else {
        memset(esp8266_hal_wps_target_ssid, 0, sizeof(esp8266_hal_wps_target_ssid));
        esp8266_hal_wps_target_ssid_len = 0;
    }
}
#include "esp_event.h"
#include "esp_wifi.h"
#include "esp_err.h"
#include "esp_private/wifi.h"

#define API_MUTEX_TAKE() do {\
    if (!s_wps_api_lock) {\
        s_wps_api_lock = xSemaphoreCreateRecursiveMutex();\
        if (!s_wps_api_lock) {\
            wpa_printf(MSG_ERROR, "wps api lock create failed");\
            return ESP_ERR_NO_MEM;\
        }\
    }\
    xSemaphoreTakeRecursive(s_wps_api_lock, portMAX_DELAY);\
} while(0)

#define API_MUTEX_GIVE() xSemaphoreGiveRecursive(s_wps_api_lock)
#define DATA_MUTEX_TAKE() xSemaphoreTakeRecursive(s_wps_data_lock, portMAX_DELAY)
#define DATA_MUTEX_GIVE() xSemaphoreGiveRecursive(s_wps_data_lock)

#define WPS_ADDR_LEN 6
#ifdef USE_WPS_TASK
struct wps_rx_param {
    u8 sa[WPS_ADDR_LEN];
    u32 len;
    STAILQ_ENTRY(wps_rx_param) bqentry;
    u8 buf[];
};
static STAILQ_HEAD(,wps_rx_param) s_wps_rxq;

typedef struct {
    void *arg;
    int ret; /* return value */
} wps_ioctl_param_t;

static TaskHandle_t s_wps_task_hdl = NULL;
static void *s_wps_queue = NULL;
static void *s_wps_api_lock = NULL;  /* Used in WPS public API only, never be freed */
static void *s_wps_api_sem = NULL;   /* Sync semaphore used between WPS publi API caller task and WPS task */
static void *s_wps_data_lock = NULL;
static void *s_wps_task_create_sem = NULL;
static bool s_wps_enabled = false;
static uint8_t s_wps_sig_cnt[SIG_WPS_NUM] = {0};

#endif

void wifi_wps_scan_done(void *arg, STATUS status);
void wifi_wps_scan(void);
int wifi_station_wps_start(void);
int wps_sm_rx_eapol_internal(u8 *src_addr, u8 *buf, u32 len);
void wifi_wps_start_internal(void);
int wifi_wps_enable_internal(const esp_wps_config_t *config);
int wifi_wps_disable_internal(void);
void wifi_station_wps_timeout_internal(void);
void wifi_station_wps_msg_timeout_internal(void);
void wifi_station_wps_success_internal(void);
static int wps_apply_credentials(void);
static void wps_station_cleanup(void);
static void wps_disarm_timers(struct wps_sm *sm);
void wifi_wps_scan_internal(void);
void wifi_station_wps_eapol_start_handle_internal(void);

struct wps_sm *gWpsSm = NULL;
static wps_factory_information_t *s_factory_info = NULL;
/* Reassembly belongs to the WPS attempt, including abort and teardown. */
static struct wpabuf *s_wps_fragment = NULL;

static void wps_clear_fragment(void)
{
    wpabuf_free(s_wps_fragment);
    s_wps_fragment = NULL;
}

static void wps_end_attempt(struct wps_sm *sm)
{
    wps_disarm_timers(sm);
    if (sm)
        sm->is_wps_scan = false;
    esp_wifi_set_wps_start_flag_internal(false);
    wps_clear_fragment();
}

#ifdef CONFIG_WPS_TESTING
int wps_version_number = 0x20;
int wps_testing_dummy_cred = 0;
#endif /* CONFIG_WPS_TESTING */

int wps_get_type(void)
{
    return esp_wifi_get_wps_type_internal();
}

int wps_set_type(uint32_t type)
{
    return esp_wifi_set_wps_type_internal(type);
}

int wps_get_status(void)
{
    return esp_wifi_get_wps_status_internal();
}

int wps_set_status(uint32_t status)
{
    return esp_wifi_set_wps_status_internal(status);
}

static void wps_rxq_init(void)
{
    DATA_MUTEX_TAKE();
    STAILQ_INIT(&s_wps_rxq);
    DATA_MUTEX_GIVE();
}

static void wps_rxq_enqueue(struct wps_rx_param *param)
{
    DATA_MUTEX_TAKE();
    STAILQ_INSERT_TAIL(&s_wps_rxq,param, bqentry);
    DATA_MUTEX_GIVE();
}

static struct wps_rx_param * wps_rxq_dequeue(void)
{
    struct wps_rx_param *param = NULL;
    DATA_MUTEX_TAKE();
    if ((param = STAILQ_FIRST(&s_wps_rxq)) != NULL) {
        STAILQ_REMOVE_HEAD(&s_wps_rxq, bqentry);
        STAILQ_NEXT(param,bqentry) = NULL;
    }
    DATA_MUTEX_GIVE();
    return param;
}

static void wps_rxq_deinit(void)
{
    struct wps_rx_param *param = NULL;
    DATA_MUTEX_TAKE();
    while ((param = STAILQ_FIRST(&s_wps_rxq)) != NULL) {
        STAILQ_REMOVE_HEAD(&s_wps_rxq, bqentry);
        STAILQ_NEXT(param,bqentry) = NULL;
        os_free(param);
    }
    DATA_MUTEX_GIVE();
}

#ifdef USE_WPS_TASK
void wps_task(void *pvParameters )
{
    ETSEvent event;
    ETSEvent *e = &event;
    wps_ioctl_param_t *param;
    bool del_task = false;

    xSemaphoreGive(s_wps_task_create_sem);

    wpa_printf(MSG_DEBUG, "wps_Task enter");
    for (;;) {
        if ( pdPASS == xQueueReceive(s_wps_queue, &event, portMAX_DELAY) ) {

            if ( (e->sig >= SIG_WPS_ENABLE) && (e->sig < SIG_WPS_NUM) ) {
                DATA_MUTEX_TAKE();
                if (s_wps_sig_cnt[e->sig]) {
                    s_wps_sig_cnt[e->sig]--;
                } else {
                    wpa_printf(MSG_ERROR, "wpsT: invalid sig cnt, sig=%d cnt=%d", e->sig, s_wps_sig_cnt[e->sig]);
                }
                DATA_MUTEX_GIVE();
            }

            wpa_printf(MSG_DEBUG, "wpsT: rx sig=%d", e->sig);

            switch (e->sig) {
            case SIG_WPS_ENABLE:
            case SIG_WPS_DISABLE:
            case SIG_WPS_START:
                param = (wps_ioctl_param_t *)e->par;
                if (!param) {
                    wpa_printf(MSG_ERROR, "wpsT: invalid param sig=%d", e->sig);
                    xSemaphoreGive(s_wps_api_sem);
                    break;
                }

                if (e->sig == SIG_WPS_ENABLE) {
                    param->ret = wifi_wps_enable_internal((esp_wps_config_t *)(param->arg));
                } else if (e->sig == SIG_WPS_DISABLE) {
                    param->ret = wifi_wps_disable_internal();
                    del_task = true;
                    s_wps_task_hdl = NULL;
                } else {
                    param->ret = wifi_station_wps_start();
                }

                xSemaphoreGive(s_wps_api_sem);
                break;

            case SIG_WPS_RX: {
                struct wps_rx_param *param = NULL;
                while ((param = wps_rxq_dequeue()) != NULL) {
                    wps_sm_rx_eapol_internal(param->sa, param->buf, param->len);
                    os_free(param);
                }
                break;
            }

            case SIG_WPS_TIMER_TIMEOUT:
                wifi_station_wps_timeout_internal();
                break;

            case SIG_WPS_TIMER_MSG_TIMEOUT:
                wifi_station_wps_msg_timeout_internal();
                break;

            case SIG_WPS_TIMER_SUCCESS_CB:
                wifi_station_wps_success_internal();
                break;

            case SIG_WPS_TIMER_SCAN:
                wifi_wps_scan_internal();
                break;

            case SIG_WPS_TIMER_EAPOL_START:
                wifi_station_wps_eapol_start_handle_internal();
                break;

            default:
                wpa_printf(MSG_ERROR, "wpsT: invalid sig=%d", e->sig);
                break;
            }

            if (del_task) {
                wpa_printf(MSG_DEBUG, "wpsT: delete task");
                break;
            }
        }
    }
    vTaskDelete(NULL);
}

/* wps_post() is thread-safe
 *
 */
int wps_post(uint32_t sig, uint32_t par)
{
    ETSEvent event;

    if (sig < SIG_WPS_ENABLE || sig >= SIG_WPS_NUM || !s_wps_queue) {
        return ESP_FAIL;
    }
    DATA_MUTEX_TAKE();
    if (s_wps_sig_cnt[sig]) {
        DATA_MUTEX_GIVE();
        return ESP_OK;
    }
    s_wps_sig_cnt[sig]++;
    event.sig = sig;
    event.par = par;
    DATA_MUTEX_GIVE();

    /* One pending event per valid signal fits the ten-entry value queue. */
    if (xQueueSend(s_wps_queue, &event, 10 / portTICK_PERIOD_MS) != pdPASS) {
        DATA_MUTEX_TAKE();
        s_wps_sig_cnt[sig]--;
        DATA_MUTEX_GIVE();
        return ESP_FAIL;
    }
    return ESP_OK;
}
#endif

static void wps_sendto_wrapper(void *buffer, uint16_t len)
{
    esp_wifi_internal_tx(WIFI_IF_STA, buffer, len);
}

/*
* wps_sm_ether_send - Send Ethernet frame
* @wpa_s: Pointer to wpa_supplicant data
* @dest: Destination MAC address
* @proto: Ethertype in host byte order
* @buf: Frame payload starting from IEEE 802.1X header
* @len: Frame payload length
* Returns: >=0 on success, <0 on failure
*/
static inline int wps_sm_ether_send(struct wps_sm *sm, const u8 *dest, u16 proto,
                                    const u8 *data, size_t data_len)
{
    void *buffer = (void *)(data - sizeof(struct l2_ethhdr));
    struct l2_ethhdr *eth = (struct l2_ethhdr *)buffer;

    memcpy(eth->h_dest, dest, ETH_ALEN);
    memcpy(eth->h_source, sm->ownaddr, ETH_ALEN);
    eth->h_proto = host_to_be16(proto);

    wps_sendto_wrapper(buffer, sizeof(struct l2_ethhdr) + data_len);

    return ESP_OK;
}


u8 *wps_sm_alloc_eapol(struct wps_sm *sm, u8 type,
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

    hdr->version = sm->eapol_version;
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


void wps_sm_free_eapol(u8 *buffer)
{
    if (buffer != NULL) {
        buffer = buffer - sizeof(struct l2_ethhdr);
        os_free(buffer);
    }
}


/**
 * wps_init - Initialize WPS Registration protocol data
 * @cfg: WPS configuration
 * Returns: Pointer to allocated data or %NULL on failure
 *
 * This function is used to initialize WPS data for a registration protocol
 * instance (i.e., each run of registration protocol as a Registrar of
 * Enrollee. The caller is responsible for freeing this data after the
 * registration run has been completed by calling wps_deinit().
 */
struct wps_data *wps_init(void)
{
    struct wps_sm *sm = gWpsSm;
    struct wps_data *data = (struct wps_data *)os_zalloc(sizeof(*data));
    const char *all_zero_pin = "00000000";

    if (data == NULL) {
        return NULL;
    }

    data->wps = sm->wps_ctx;

    if (IS_WPS_REGISTRAR(wps_get_type())) {
        data->registrar = 1;
    } else {
        data->registrar = 0;
    }

    data->registrar = 0; /* currently, we force to support enrollee only */

    if (data->registrar) {
        memcpy(data->uuid_r, sm->uuid, WPS_UUID_LEN);
    } else {
        memcpy(data->mac_addr_e, sm->dev->mac_addr, ETH_ALEN);
        memcpy(data->uuid_e, sm->uuid, WPS_UUID_LEN);
    }

    if (wps_get_type() == WPS_TYPE_PIN) {
        u32 spin = 0;
        data->dev_pw_id = DEV_PW_DEFAULT;
        data->dev_password_len = 8;
        data->dev_password = (u8 *) os_zalloc(data->dev_password_len + 1);
        if (data->dev_password == NULL) {
            os_free(data);
            return NULL;
        }

        if (esp8266_hal_wps_pin_override[0] != 0) {
            memcpy(data->dev_password, esp8266_hal_wps_pin_override, 8);
            data->dev_password[8] = 0;
        } else {
            spin = wps_generate_pin();
            sprintf((char *)data->dev_password, "%08d", spin);
        }
        wpa_hexdump_key(MSG_DEBUG, "WPS: AP PIN dev_password",
                        data->dev_password, data->dev_password_len);
        do {
            char tmpp[9];
            os_bzero(tmpp, 9);
            memcpy(tmpp, data->dev_password, 8);
            wpa_printf(MSG_DEBUG, "WPS PIN [%s]", tmpp);
            system_event_t evt = {0};
            evt.event_id = SYSTEM_EVENT_STA_WPS_ER_PIN;
            memcpy(evt.event_info.sta_er_pin.pin_code, data->dev_password, 8);
            esp_event_send(&evt);
        } while (0);
    } else if (wps_get_type() == WPS_TYPE_PBC) {
        data->pbc = 1;
        /* Use special PIN '00000000' for PBC */
        data->dev_pw_id = DEV_PW_PUSHBUTTON;
        if (data->dev_password) {
            os_free(data->dev_password);
        }
        data->dev_password = (u8 *) os_zalloc(9);
        if (data->dev_password == NULL) {
            os_free(data);
            return NULL;
        } else {
            strncpy((char *)data->dev_password, all_zero_pin, 9);
        }
        data->dev_password_len = 8;
    }

#ifdef CONFIG_WPS_NFC
    if (cfg->wps->ap && !cfg->registrar && cfg->wps->ap_nfc_dev_pw_id) {
        data->dev_pw_id = cfg->wps->ap_nfc_dev_pw_id;
        os_free(data->dev_password);
        data->dev_password =
            os_malloc(wpabuf_len(cfg->wps->ap_nfc_dev_pw));
        if (data->dev_password == NULL) {
            os_free(data);
            return NULL;
        }
        memcpy(data->dev_password,
               wpabuf_head(cfg->wps->ap_nfc_dev_pw),
               wpabuf_len(cfg->wps->ap_nfc_dev_pw));
        data->dev_password_len = wpabuf_len(cfg->wps->ap_nfc_dev_pw);
    }
#endif /* CONFIG_WPS_NFC */
    data->wps->config_methods = WPS_CONFIG_PUSHBUTTON | WPS_CONFIG_DISPLAY;
#ifdef CONFIG_WPS2
    data->wps->config_methods |= (WPS_CONFIG_VIRT_PUSHBUTTON | WPS_CONFIG_PHY_DISPLAY);
#endif

    data->state = data->registrar ? RECV_M1 : SEND_M1;

    return data;
}


/**
 * wps_deinit - Deinitialize WPS Registration protocol data
 * @data: WPS Registration protocol data from wps_init()
 */
void wps_deinit(void)
{
    struct wps_data *data = gWpsSm->wps;

#ifdef CONFIG_WPS_NFC
    if (data->registrar && data->nfc_pw_token)
        wps_registrar_remove_nfc_pw_token(data->wps->registrar,
                                          data->nfc_pw_token);
#endif /* CONFIG_WPS_NFC */

    if (data->wps_pin_revealed) {
        wpa_printf(MSG_DEBUG,  "WPS: Full PIN information revealed and "
                   "negotiation failed");
    } else if (data->registrar)
        wpa_printf(MSG_DEBUG,  "WPS: register information revealed and "
                   "negotiation failed");
    wpabuf_free(data->dh_privkey);

#ifdef DESP8266_WORKAROUND
    /*
     * due to the public key calculated when wps start, it will not calculate anymore even when we build M1 message, also calculate the key need take a long time
     * which would cause WPS fail, so we clean the key after WPS finished .
     */
    data->dh_privkey = NULL;
#endif //DESP32_WORKAROUND

    wpabuf_free(data->dh_pubkey_e);
    wpabuf_free(data->dh_pubkey_r);
    wpabuf_free(data->last_msg);
    os_free(data->dev_password);
    dh5_free(data->dh_ctx);
    wps_dev_deinit(&data->peer_dev);
#ifdef CONFIG_WPS_NFC
    os_free(data->nfc_pw_token);
#endif
    os_free(data);
}

static int
wps_build_ic_appie_wps_pr(void)
{
    struct wps_sm *sm = gWpsSm;
    struct wpabuf *ie;
    int ret;
    int password_id = wps_get_type() == WPS_TYPE_PBC ? DEV_PW_PUSHBUTTON : DEV_PW_DEFAULT;

    ie = wps_build_probe_req_ie(password_id, sm->dev, sm->uuid,
                              WPS_REQ_ENROLLEE, 0, NULL);
    if (!ie)
        return ESP_ERR_NO_MEM;
    ret = esp_wifi_set_appie_internal(WIFI_APPIE_WPS_PR,
                                     (uint8_t *)wpabuf_head(ie), wpabuf_len(ie), 0);
    wpabuf_free(ie);
    return ret;
}

static int
wps_build_ic_appie_wps_ar(void)
{
    struct wpabuf *ie = wps_build_assoc_req_ie(WPS_REQ_ENROLLEE);
    int ret;

    if (!ie)
        return ESP_ERR_NO_MEM;
    ret = esp_wifi_set_appie_internal(WIFI_APPIE_WPS_AR,
                                     (uint8_t *)wpabuf_head(ie), wpabuf_len(ie), 0);
    wpabuf_free(ie);
    return ret;
}

static int wps_selected_registrar(const struct wpabuf *ie, const u8 *bssid)
{
    struct wps_parse_attr *attr = os_malloc(sizeof(*attr));
    struct wps_sm *sm = gWpsSm;
    int selected = 0;
    int i;

    if (!attr)
        return -1;
    if (wps_parse_msg(ie, attr) < 0) {
        os_free(attr);
        return -1;
    }
    /* Pinned PIN-on/STRICT-off PBC || PIN accepts either password-ID class. */
    if (attr->selected_registrar && *attr->selected_registrar) {
        selected = 1;
    } else if (sm->ignore_sel_reg) {
        selected = 1;
        for (i = 0; i < WPS_MAX_DIS_AP_NUM; i++) {
            if (memcmp(sm->dis_ap_list[i].bssid, bssid, ETH_ALEN) == 0) {
                selected = 0;
                break;
            }
        }
    }
    os_free(attr);
    return selected;
}

static bool
wps_parse_scan_result(struct wps_scan_ie *scan)
{
    struct wps_sm *sm = gWpsSm;
    wifi_mode_t op_mode = 0;

    if (!sm || !sm->is_wps_scan || !scan || !scan->ssid ||
        scan->ssid[1] > sizeof(sm->config.ssid))
        return false;
#ifdef WPS_DEBUG
    char tmp[32];

    os_bzero(tmp, 32);
    strncpy(tmp, (char *)&scan->ssid[2], (int)scan->ssid[1]);
    wpa_printf(MSG_DEBUG, "wps parse scan: %s", tmp);
#endif

    if (wps_get_type() == WPS_TYPE_DISABLE
            || (wps_get_status() != WPS_STATUS_DISABLE
                && wps_get_status() != WPS_STATUS_SCANNING)
       ) {
        return false;
    }

    esp_wifi_get_mode(&op_mode);
    if ((op_mode == WIFI_MODE_STA || op_mode == WIFI_MODE_APSTA) && scan->wps) {
        struct wpabuf view;
        int selected;
        bool esp8266_hal_wps_selected;

        if (scan->wps[1] < 4)
            return false;
        wpabuf_set(&view, scan->wps + 6, scan->wps[1] - 4);
        selected = wps_selected_registrar(&view, scan->bssid);
        if (selected < 0)
            return false;
        esp8266_hal_wps_selected = selected != 0;
        if (!esp8266_hal_wps_selected && esp8266_hal_wps_target_ssid_len != 0 &&
                scan->ssid[1] == esp8266_hal_wps_target_ssid_len &&
                memcmp(&scan->ssid[2], esp8266_hal_wps_target_ssid, esp8266_hal_wps_target_ssid_len) == 0) {
            esp8266_hal_wps_selected = true;
        }
        if (esp8266_hal_wps_selected) {

            if (sm->is_wps_scan == false) {
                return false;
            }
            if (memcmp(sm->config.bssid, scan->bssid, ETH_ALEN) != 0 ) {
                sm->discover_ssid_cnt++;
            }

            if (!scan->rsn && !scan->wpa && (scan->capinfo & WIFI_CAPINFO_PRIVACY)) {
                wpa_printf(MSG_ERROR, "WEP not suppported in WPS");

                return false;
            }

            esp_wifi_enable_sta_privacy_internal();
            memset(sm->config.ssid, 0, sizeof(sm->config.ssid));
            memcpy(sm->config.ssid, &scan->ssid[2], scan->ssid[1]);
            if (scan->bssid) {
                memcpy(gWpsSm->bssid, scan->bssid, ETH_ALEN);
                memcpy(sm->config.bssid, scan->bssid, ETH_ALEN);
                sm->config.bssid_set = 1;
            } else {
            }
            wpa_printf(MSG_DEBUG, "wps discover [%s]", (char *)sm->config.ssid);
            sm->scan_cnt = 0;

            sm->channel = scan->chan;

            return true;
        }
    }

    return false;
}

int wps_send_eap_identity_rsp(u8 id)
{
    struct wps_sm *sm = gWpsSm;
    struct wpabuf *eap_buf = NULL;
    u8 bssid[6];
    u8 *buf = NULL;
    size_t len;
    int ret = ESP_OK;

    wpa_printf(MSG_DEBUG, "wps send eapol id rsp");
    eap_buf = eap_msg_alloc(EAP_VENDOR_IETF, EAP_TYPE_IDENTITY, sm->identity_len,
                            EAP_CODE_RESPONSE, id);
    if (!eap_buf) {
        ret = ESP_FAIL;
        goto _err;
    }

    ret = esp_wifi_get_assoc_bssid_internal(bssid);
    if (ret != 0) {
        wpa_printf(MSG_ERROR, "bssid is empty!");
        ret = ESP_FAIL;
        goto _err;
    }

    wpabuf_put_data(eap_buf, sm->identity, sm->identity_len);

    buf = wps_sm_alloc_eapol(sm, IEEE802_1X_TYPE_EAP_PACKET, wpabuf_head_u8(eap_buf), wpabuf_len(eap_buf), &len, NULL);
    if (!buf) {
        ret = ESP_ERR_NO_MEM;
        goto _err;
    }

    ret = wps_sm_ether_send(sm, bssid, ETH_P_EAPOL, buf, len);
    if (ret) {
        ret = ESP_FAIL;
        goto _err;
    }

_err:
    wps_sm_free_eapol(buf);
    wpabuf_free(eap_buf);
    return ret;
}

int wps_send_frag_ack(u8 id)
{
    struct wps_sm *sm = gWpsSm;
    struct wpabuf *eap_buf = NULL;
    u8 bssid[6];
    u8 *buf;
    size_t len;
    int ret = 0;
    enum wsc_op_code opcode = WSC_FRAG_ACK;

    wpa_printf(MSG_DEBUG, "send frag ack id:%d", id);

    if (!sm) {
        return ESP_FAIL;
    }

    ret = esp_wifi_get_assoc_bssid_internal(bssid);
    if (ret != 0) {
        wpa_printf(MSG_ERROR, "bssid is empty!");
        return ret;
    }

    eap_buf = eap_msg_alloc(EAP_VENDOR_WFA, 0x00000001, 2, EAP_CODE_RESPONSE, id);
    if (!eap_buf) {
        ret = ESP_ERR_NO_MEM;
        goto _err;
    }

    wpabuf_put_u8(eap_buf, opcode);
    wpabuf_put_u8(eap_buf, 0x00); /* flags */

    buf = wps_sm_alloc_eapol(sm, IEEE802_1X_TYPE_EAP_PACKET, wpabuf_head_u8(eap_buf), wpabuf_len(eap_buf), &len, NULL);
    if (!buf) {
        ret = ESP_ERR_NO_MEM;
        goto _err;
    }

    ret = wps_sm_ether_send(sm, bssid, ETH_P_EAPOL, buf, len);
    wps_sm_free_eapol(buf);
    if (ret) {
        ret = ESP_ERR_NO_MEM;
        goto _err;
    }

_err:
    wpabuf_free(eap_buf);
    return ret;
}

int wps_enrollee_process_msg_frag(struct wpabuf **buf, int tot_len, u8 *frag_data, int frag_len, u8 flag)
{
    struct wps_sm *sm = gWpsSm;
    u8 identifier;

    if (!sm) {
        return ESP_FAIL;
    }

    identifier = sm->current_identifier;

    if (buf == NULL || frag_data == NULL || frag_len < 0 || tot_len < 0) {
        wpa_printf(MSG_ERROR, "fun:%s. line:%d, frag buf or frag data is null", __FUNCTION__, __LINE__);
        return ESP_FAIL;
    }

    if (*buf == NULL) {
        if (0 == (flag & WPS_MSG_FLAG_LEN) || tot_len < frag_len ||
            ((flag & WPS_MSG_FLAG_MORE) && tot_len == frag_len)) {
            wpa_printf(MSG_ERROR, "fun:%s. line:%d, flag error:%02x", __FUNCTION__, __LINE__, flag);
            return ESP_FAIL;
        }

        *buf = wpabuf_alloc(tot_len);
        if (*buf == NULL) {
            return ESP_ERR_NO_MEM;
        }

        wpabuf_put_data(*buf, frag_data, frag_len);
        return wps_send_frag_ack(identifier);
    }

    if (flag & WPS_MSG_FLAG_LEN) {
        wpa_printf(MSG_ERROR, "fun:%s. line:%d, flag error:%02x", __FUNCTION__, __LINE__, flag);
        return ESP_FAIL;
    }

    if ((size_t)frag_len > wpabuf_tailroom(*buf) ||
        ((flag & WPS_MSG_FLAG_MORE) && (size_t)frag_len == wpabuf_tailroom(*buf)))
        return ESP_FAIL;
    wpabuf_put_data(*buf, frag_data, frag_len);

    if (flag & WPS_MSG_FLAG_MORE) {
        return wps_send_frag_ack(identifier);
    }

    return ESP_OK;
}

int wps_process_wps_mX_req(u8 *ubuf, int len, enum wps_process_res *res)
{
    struct wps_sm *sm = gWpsSm;
    struct eap_expand *expanded;
    u8 *payload;
    u8 flags;
    int payload_len;
    int total;
    int ret = ESP_FAIL;

    if (res)
        *res = WPS_FAILURE;
    if (!sm || !ubuf || len < (int)sizeof(*expanded) + 1)
        goto fail;
    expanded = (struct eap_expand *)ubuf;
    flags = ubuf[sizeof(*expanded)];
    payload = ubuf + sizeof(*expanded) + 1;
    payload_len = len - sizeof(*expanded) - 1;
    total = payload_len;
    if (flags & WPS_MSG_FLAG_LEN) {
        if (payload_len < 2)
            goto fail;
        total = WPA_GET_BE16(payload);
        payload += 2;
        payload_len -= 2;
    }
    if ((flags & WPS_MSG_FLAG_MORE) || s_wps_fragment) {
        ret = wps_enrollee_process_msg_frag(&s_wps_fragment, total,
                                          payload, payload_len, flags);
        if (ret != ESP_OK)
            goto fail;
        if (flags & WPS_MSG_FLAG_MORE) {
            if (res)
                *res = WPS_FRAGMENT;
            return ESP_OK;
        }
        if (wpabuf_len(s_wps_fragment) != wpabuf_size(s_wps_fragment)) {
            ret = ESP_FAIL;
            goto fail;
        }
    } else {
        if (total != payload_len)
            goto fail;
        s_wps_fragment = wpabuf_alloc_copy(payload, payload_len);
        if (!s_wps_fragment) {
            ret = ESP_ERR_NO_MEM;
            goto fail;
        }
    }
    ets_timer_disarm(&sm->wps_msg_timeout_timer);
    if (res)
        *res = wps_enrollee_process_msg(sm->wps, expanded->opcode, s_wps_fragment);
    else
        wps_enrollee_process_msg(sm->wps, expanded->opcode, s_wps_fragment);
    wps_clear_fragment();
    return ESP_OK;
fail:
    wps_clear_fragment();
    return ret;
}

int wps_send_wps_mX_rsp(u8 id)
{
    struct wps_sm *sm = gWpsSm;
    struct wpabuf *eap_buf = NULL;
    struct wpabuf *wps_buf = NULL;
    u8 bssid[6];
    u8 *buf = NULL;
    size_t len;
    int ret = 0;
    enum wsc_op_code opcode;

    wpa_printf(MSG_DEBUG, "wps send wps mX rsp");

    if (!sm) {
        return ESP_FAIL;
    }

    ret = esp_wifi_get_assoc_bssid_internal(bssid);
    if (ret != 0) {
        wpa_printf(MSG_ERROR, "bssid is empty!");
        return ret;
    }

    wps_buf = (struct wpabuf *)wps_enrollee_get_msg(sm->wps, &opcode);
    if (!wps_buf) {
        ret = ESP_FAIL;
        goto _err;
    }

    eap_buf = eap_msg_alloc(EAP_VENDOR_WFA, 0x00000001, wpabuf_len(wps_buf) + 2, EAP_CODE_RESPONSE, id);
    if (!eap_buf) {
        ret = ESP_FAIL;
        goto _err;
    }

    wpabuf_put_u8(eap_buf, opcode);
    wpabuf_put_u8(eap_buf, 0x00); /* flags */
    wpabuf_put_data(eap_buf, wpabuf_head_u8(wps_buf), wpabuf_len(wps_buf));


    wpabuf_free(wps_buf);
    wps_buf = NULL;

    buf = wps_sm_alloc_eapol(sm, IEEE802_1X_TYPE_EAP_PACKET, wpabuf_head_u8(eap_buf), wpabuf_len(eap_buf), &len, NULL);
    if (!buf) {
        ret = ESP_FAIL;
        goto _err;
    }

    ret = wps_sm_ether_send(sm, bssid, ETH_P_EAPOL, buf, len);
    if (ret) {
        ret = ESP_FAIL;
        goto _err;
    }

_err:
    wps_sm_free_eapol(buf);
    wpabuf_free(wps_buf);
    wpabuf_free(eap_buf);
    return ret;
}



int wps_tx_start(void)
{
    struct wps_sm *sm = gWpsSm;
    u8 bssid[6];
    u8 *buf;
    size_t len;
    int ret = 0;

    if (!sm || !sm->is_wps_scan) {
        return ESP_FAIL;
    }

    ret = esp_wifi_get_assoc_bssid_internal(bssid);
    if (ret != 0) {
        wpa_printf(MSG_ERROR, "bssid is empty!");
        return ret;
    }


    wpa_printf(MSG_DEBUG,  "WPS: Send EAPOL START.");
    buf = wps_sm_alloc_eapol(sm, IEEE802_1X_TYPE_EAPOL_START, (u8 *)"", 0, &len, NULL);
    if (!buf) {
        return ESP_ERR_NO_MEM;
    }

    wps_sm_ether_send(sm, bssid, ETH_P_EAPOL, buf, len);
    wps_sm_free_eapol(buf);

    ets_timer_arm(&sm->wps_eapol_start_timer, 3000, 0);

    return ESP_OK;
}

int wps_start_pending(void)
{
    if (!gWpsSm) {
        return ESP_FAIL;
    }

    wpa_printf(MSG_DEBUG, "wps start pending");
    return wps_tx_start();
}

int wps_stop_process(system_event_sta_wps_fail_reason_t reason_code)
{
    struct wps_sm *sm = gWpsSm;

    if (!sm || !sm->is_wps_scan) {
        return ESP_FAIL;
    }

    wps_set_status(WPS_STATUS_DISABLE);
    sm->scan_cnt = 0;
    sm->discover_ssid_cnt = 0;
    sm->wps->state = SEND_M1;
    os_bzero(sm->bssid, ETH_ALEN);
    os_bzero(sm->ssid, sizeof(sm->ssid));
    os_bzero(sm->ssid_len, sizeof(sm->ssid_len));
    os_bzero((u8 *)&sm->config, sizeof(wifi_sta_config_t));
    sm->ap_cred_cnt = 0;

    wps_end_attempt(sm);

    esp_wifi_disconnect();

    wpa_printf(MSG_DEBUG, "Write wps_fail_information");
    system_event_t evt = {0};
    evt.event_id = SYSTEM_EVENT_STA_WPS_ER_FAILED;
    evt.event_info.sta_er_fail_reason = reason_code;
    esp_event_send(&evt);
    return ESP_OK;
}

int wps_finish(void)
{
    struct wps_sm *sm = gWpsSm;

    if (!sm || !sm->wps)
        return ESP_FAIL;
    if (sm->wps->state == WPS_FINISHED)
        return wps_apply_credentials();
    wps_stop_process(WPS_FAIL_REASON_NORMAL);
    return ESP_FAIL;
}

/* Add current ap to discard ap list */
void wps_add_discard_ap(u8 *bssid)
{
    struct wps_sm *sm = gWpsSm;
    u8 cnt = sm->discard_ap_cnt;

    if (!gWpsSm || !bssid) {
        return;
    }

    if (sm->discard_ap_cnt < WPS_MAX_DIS_AP_NUM) {
        sm->discard_ap_cnt++;
    } else {
        for (cnt = 0; cnt < WPS_MAX_DIS_AP_NUM - 2; cnt++) {
            memcpy(sm->dis_ap_list[cnt].bssid, sm->dis_ap_list[cnt + 1].bssid, 6);
        }
        sm->discard_ap_cnt = WPS_MAX_DIS_AP_NUM;
    }
    memcpy(sm->dis_ap_list[cnt].bssid, bssid, 6);
}

int wps_start_msg_timer(void)
{
    struct wps_sm *sm = gWpsSm;
    uint32_t msg_timeout;
    int ret = ESP_FAIL;

    if (!gWpsSm) {
        return ESP_FAIL;
    }

    if (sm->wps->state == WPS_FINISHED) {
        msg_timeout = 100;
        wpa_printf(MSG_DEBUG, "start msg timer WPS_FINISHED %d ms", msg_timeout);
        ets_timer_disarm(&sm->wps_msg_timeout_timer);
        ets_timer_arm(&sm->wps_msg_timeout_timer, msg_timeout, 0);
        ret = 0;
    } else if (sm->wps->state == RECV_M2) {
        msg_timeout = 5000;
        wpa_printf(MSG_DEBUG, "start msg timer RECV_M2 %d ms", msg_timeout);
        ets_timer_disarm(&sm->wps_msg_timeout_timer);
        ets_timer_arm(&sm->wps_msg_timeout_timer, msg_timeout, 0);
        ret = 0;
    }
    return ret;
}

/**
 * wps_sm_rx_eapol - Process received WPA EAPOL frames
 * @sm: Pointer to WPA state machine data from wpa_sm_init()
 * @src_addr: Source MAC address of the EAPOL packet
 * @buf: Pointer to the beginning of the EAPOL data (EAPOL header)
 * @len: Length of the EAPOL frame
 * Returns: 1 = WPA EAPOL-Key processed, ESP_OK = not a WPA EAPOL-Key, ESP_FAIL failure
 *
 * This function is called for each received EAPOL frame. Other than EAPOL-Key
 * frames can be skipped if filtering is done elsewhere. wpa_sm_rx_eapol() is
 * only processing WPA and WPA2 EAPOL-Key frames.
 *
 * The received EAPOL-Key packets are validated and valid packets are replied
 * to. In addition, key material (PTK, GTK) is configured at the end of a
 * successful key handshake.
 * buf begin from version, so remove mac header ,snap header and ether_type
 */
int wps_sm_rx_eapol(u8 *src_addr, u8 *buf, u32 len)
{
    if (!gWpsSm) {
        return ESP_FAIL;
    }

#ifdef USE_WPS_TASK
    {
        struct wps_rx_param *param;
        /* Header and bytes are released together by the worker or teardown. */
        if (len > SIZE_MAX - sizeof(*param)) {
            return ESP_ERR_NO_MEM;
        }
        param = os_malloc(sizeof(*param) + len);
        if (!param) {
            return ESP_ERR_NO_MEM;
        }
        memcpy(param->buf, buf, len);
        param->len = len;
        memcpy(param->sa, src_addr, WPS_ADDR_LEN);

        wps_rxq_enqueue(param);
        return wps_post(SIG_WPS_RX, 0);
    }
#else
    return wps_sm_rx_eapol_internal(src_addr, buf, len);
#endif
}

int wps_sm_rx_eapol_internal(u8 *src_addr, u8 *buf, u32 len)
{
    struct wps_sm *sm = gWpsSm;
    u32 plen, data_len, eap_len;
    struct ieee802_1x_hdr *hdr;
    struct eap_hdr *ehdr;
    u8 *tmp;
    u8 eap_code;
    u8 eap_type;
    int ret = ESP_FAIL;
    enum wps_process_res res = WPS_DONE;

    if (!sm || !sm->is_wps_scan) {
        return ESP_FAIL;
    }

    if (len < sizeof(*hdr) + sizeof(*ehdr)) {
#ifdef DEBUG_PRINT
        wpa_printf(MSG_DEBUG,  "WPA: EAPOL frame too short to be a WPA "
                   "EAPOL-Key (len %lu, expecting at least %lu)",
                   (unsigned long) len,
                   (unsigned long) sizeof(*hdr) + sizeof(*ehdr));
#endif
        return ESP_OK;
    }

    tmp = buf;

    hdr = (struct ieee802_1x_hdr *) tmp;
    ehdr = (struct eap_hdr *) (hdr + 1);
    plen = be_to_host16(hdr->length);
    data_len = plen + sizeof(*hdr);
    eap_len = be_to_host16(ehdr->length);

#ifdef DEBUG_PRINT
    wpa_printf(MSG_DEBUG, "IEEE 802.1X RX: version=%d type=%d length=%d",
               hdr->version, hdr->type, plen);
#endif

    if (hdr->version < EAPOL_VERSION) {
        /* TODO: backwards compatibility */
    }
    if (hdr->type != IEEE802_1X_TYPE_EAP_PACKET) {
#ifdef DEBUG_PRINT
        wpa_printf(MSG_DEBUG, "WPS: EAP frame (type %u) discarded, "
                   "not a EAP PACKET frame", hdr->type);
#endif
        ret = 0;
        goto out;
    }
    if (plen > len - sizeof(*hdr) || plen < sizeof(*ehdr)) {
#ifdef DEBUG_PRINT
        wpa_printf(MSG_DEBUG, "WPA: EAPOL frame payload size %lu "
                   "invalid (frame size %lu)",
                   (unsigned long) plen, (unsigned long) len);
#endif
        ret = 0;
        goto out;
    }

    wpa_hexdump(MSG_MSGDUMP, "WPA: RX EAPOL-EAP PACKET", tmp, len);

    if (data_len < len) {
#ifdef DEBUG_PRINT
        wpa_printf(MSG_DEBUG, "WPA: ignoring %lu bytes after the IEEE "
                   "802.1X data", (unsigned long) len - data_len);
#endif
    }

    if (eap_len != plen) {
#ifdef DEBUG_PRINT
        wpa_printf(MSG_DEBUG, "WPA: EAPOL length %lu "
                   "invalid (eapol length %lu)",
                   (unsigned long) eap_len, (unsigned long) plen);
#endif
        ret = 0;
        goto out;
    }

    eap_code = ehdr->code;
    switch (eap_code) {
    case EAP_CODE_SUCCESS:
        wpa_printf(MSG_DEBUG, "error: receive eapol success frame!");
        ret = 0;
        break;
    case EAP_CODE_FAILURE:
        wpa_printf(MSG_DEBUG, "receive eap code failure!");
        return wps_finish();
    case EAP_CODE_RESPONSE:
        wpa_printf(MSG_DEBUG, "error: receive eapol response frame!");
        ret = 0;
        break;
    case EAP_CODE_REQUEST: {
        if (plen <= sizeof(*ehdr)) {
            ret = ESP_FAIL;
            goto out;
        }
        eap_type = ((u8 *)ehdr)[sizeof(*ehdr)];
        switch (eap_type) {
        case EAP_TYPE_IDENTITY:
            wpa_printf(MSG_DEBUG, "=========identity===========");
            sm->current_identifier = ehdr->identifier;
            ets_timer_disarm(&sm->wps_eapol_start_timer);
            wpa_printf(MSG_DEBUG,  "WPS: Build EAP Identity.");
            ret = wps_send_eap_identity_rsp(ehdr->identifier);
            ets_timer_arm(&sm->wps_eapol_start_timer, 3000, 0);
            break;
        case EAP_TYPE_EXPANDED:
            wpa_printf(MSG_DEBUG, "=========expanded plen[%d], %d===========", plen, sizeof(*ehdr));
            if (ehdr->identifier == sm->current_identifier) {
                ret = 0;
                wpa_printf(MSG_DEBUG, "wps: ignore overlap identifier");
                goto out;
            }
            sm->current_identifier = ehdr->identifier;

            tmp = (u8 *)(ehdr + 1) + 1;
            ret = wps_process_wps_mX_req(tmp, plen - sizeof(*ehdr) - 1, &res);
            if (ret == 0 && res != WPS_FAILURE && res != WPS_IGNORE && res != WPS_FRAGMENT) {
                ret = wps_send_wps_mX_rsp(ehdr->identifier);
                if (ret == 0) {
                    wpa_printf(MSG_DEBUG, "sm->wps->state = %d", sm->wps->state);
                    wps_start_msg_timer();
                }
            } else if (ret == 0 && res == WPS_FRAGMENT) {
                wpa_printf(MSG_DEBUG, "wps frag, continue...");
                ret = ESP_OK;
            } else if (res == WPS_IGNORE) {
                wpa_printf(MSG_DEBUG, "IGNORE overlap Mx");
                ret = ESP_OK; /* IGNORE the overlap */
            } else {
                ret = ESP_FAIL;
            }
            break;
        default:
            break;
        }
        break;
    }
    default:
        break;
    }
out:
    if (ret != 0 || res == WPS_FAILURE) {
        system_event_sta_wps_fail_reason_t reason =
            (system_event_sta_wps_fail_reason_t)((((u32)sm->wps->state & 0xff) << 16) |
                                                 (((u32)res & 0xff) << 8) |
                                                 ((u32)(-ret) & 0xff));
        wps_stop_process(reason);
    }
    return ret;
}

int wps_set_default_factory(void)
{
    if (!s_factory_info) {
        s_factory_info = os_zalloc(sizeof(wps_factory_information_t));
        if (!s_factory_info) {
            wpa_printf(MSG_ERROR, "wps factory info malloc failed");
            return ESP_ERR_NO_MEM;
        }
    }

    sprintf(s_factory_info->manufacturer, "ESPRESSIF");
    sprintf(s_factory_info->model_name, "ESPRESSIF IOT");
    sprintf(s_factory_info->model_number, "ESP8266");
    sprintf(s_factory_info->device_name, "ESP8266 STATION");

    return ESP_OK;
}

int wps_set_factory_info(const esp_wps_config_t *config)
{
    int ret;

    ret = wps_set_default_factory();
    if (ret != 0) {
        return ret;
    }

    if (config->factory_info.manufacturer[0] != 0) {
        memcpy(s_factory_info->manufacturer, config->factory_info.manufacturer, WPS_MAX_MANUFACTURER_LEN - 1);
    }

    if (config->factory_info.model_number[0] != 0) {
        memcpy(s_factory_info->model_number, config->factory_info.model_number, WPS_MAX_MODEL_NUMBER_LEN - 1);
    }

    if (config->factory_info.model_name[0] != 0) {
        memcpy(s_factory_info->model_name, config->factory_info.model_name, WPS_MAX_MODEL_NAME_LEN - 1);
    }

    if (config->factory_info.device_name[0] != 0) {
        memcpy(s_factory_info->device_name, config->factory_info.device_name, WPS_MAX_DEVICE_NAME_LEN - 1);
    }

    wpa_printf(MSG_INFO, "manufacturer: %s, model number: %s, model name: %s, device name: %s", s_factory_info->manufacturer,
               s_factory_info->model_number, s_factory_info->model_name, s_factory_info->device_name);

    return ESP_OK;
}


int wps_dev_init(void)
{
    int ret = 0;
    struct wps_sm *sm = gWpsSm;
    struct wps_device_data *dev = NULL;

    if (!sm || !sm->wps_ctx) {
        ret = ESP_FAIL;
        goto _out;
    }

    dev = &sm->wps_ctx->dev;
    sm->dev = dev;

    if (!dev) {
        ret = ESP_FAIL;
        goto _out;
    }
    dev->config_methods = WPS_CONFIG_VIRT_PUSHBUTTON | WPS_CONFIG_PHY_DISPLAY;
    dev->rf_bands = WPS_RF_24GHZ;

    WPA_PUT_BE16(dev->pri_dev_type, WPS_DEV_COMPUTER);
    WPA_PUT_BE32(dev->pri_dev_type + 2, WPS_DEV_OUI_WFA);
    WPA_PUT_BE16(dev->pri_dev_type + 6, WPS_DEV_COMPUTER_PC);

    if (!s_factory_info) {
        ret = wps_set_default_factory();
        if (ret != 0) {
            goto _out;
        }
    }

    dev->manufacturer = (char *)os_zalloc(WPS_MAX_MANUFACTURER_LEN);
    if (!dev->manufacturer) {
        ret = ESP_FAIL;
        goto _out;
    }
    memcpy(dev->manufacturer, s_factory_info->manufacturer, sizeof(s_factory_info->manufacturer));

    dev->model_name = (char *)os_zalloc(WPS_MAX_MODEL_NAME_LEN);
    if (!dev->model_name) {
        ret = ESP_FAIL;
        goto _out;
    }
    memcpy(dev->model_name, s_factory_info->model_name, sizeof(s_factory_info->model_name));

    dev->model_number = (char *)os_zalloc(WPS_MAX_MODEL_NAME_LEN);
    if (!dev->model_number) {
        ret = ESP_FAIL;
        goto _out;
    }
    memcpy(dev->model_number, s_factory_info->model_number, sizeof(s_factory_info->model_number));

    dev->device_name = (char *)os_zalloc(WPS_MAX_DEVICE_NAME_LEN);
    if (!dev->device_name) {
        ret = ESP_FAIL;
        goto _out;
    }
    memcpy(dev->device_name, s_factory_info->device_name, sizeof(s_factory_info->device_name));

    dev->serial_number = (char *)os_zalloc(16);
    if (!dev->serial_number) {
        ret = ESP_FAIL;
        goto _out;
    }
    sprintf(dev->serial_number, "%02x%02x%02x%02x%02x%02x",
            sm->ownaddr[0], sm->ownaddr[1], sm->ownaddr[2],
            sm->ownaddr[3], sm->ownaddr[4], sm->ownaddr[5]);

    uuid_gen_mac_addr(sm->ownaddr, sm->uuid);
    memcpy(dev->mac_addr, sm->ownaddr, ETH_ALEN);

    return ESP_OK;

_out:
    /* wifi_station_wps_init owns partial-device cleanup. */
    return ret;
}


int wps_dev_deinit(struct wps_device_data *dev)
{
    if (!dev)
        return ESP_OK;

    os_free(dev->manufacturer);
    os_free(dev->model_name);
    os_free(dev->model_number);
    os_free(dev->device_name);
    os_free(dev->serial_number);
    dev->manufacturer = NULL;
    dev->model_name = NULL;
    dev->model_number = NULL;
    dev->device_name = NULL;
    dev->serial_number = NULL;
    return ESP_OK;
}

void
wifi_station_wps_timeout_internal(void)
{
    struct wps_sm *sm = gWpsSm;

    if (!sm || !sm->is_wps_scan) {
        return;
    }

    wps_end_attempt(sm);
    sm->wps->state = SEND_M1;

    wps_set_status(WPS_STATUS_DISABLE);

    system_event_t evt = {0};
    evt.event_id = SYSTEM_EVENT_STA_WPS_ER_TIMEOUT;
    esp_event_send(&evt);

}

void wifi_station_wps_timeout(void)
{
#ifdef USE_WPS_TASK
    wps_post(SIG_WPS_TIMER_TIMEOUT, 0);
    return;
#else
    wifi_station_wps_timeout_internal();
#endif
}

void
wifi_station_wps_msg_timeout_internal(void)
{
    struct wps_sm *sm = gWpsSm;

    if (!sm || !sm->is_wps_scan) {
        return;
    }

    if (sm->wps->state == WPS_FINISHED) {
        wpa_printf(MSG_DEBUG, "wps msg timeout WPS_FINISHED");
        wps_finish();
    } else if (sm->wps->state == RECV_M2) {
        wpa_printf(MSG_DEBUG, "wps msg timeout RECV_M2");
        wpa_printf(MSG_DEBUG, "wps recev m2/m2d timeout------>");
        wps_add_discard_ap(sm->config.bssid);
        wps_stop_process(WPS_FAIL_REASON_RECV_M2D);
    }
}

void wifi_station_wps_msg_timeout(void)
{
#ifdef USE_WPS_TASK
    wps_post(SIG_WPS_TIMER_MSG_TIMEOUT, 0);
    return;
#else
    wifi_station_wps_msg_timeout_internal();
#endif
}

static int wps_apply_credentials(void)
{
    struct wps_sm *sm = gWpsSm;
    wifi_config_t config = {0};
    system_event_t event = {0};
    int ret = ESP_FAIL;

    if (!sm || !sm->wps)
        return ESP_FAIL;
    if (wps_get_status() == WPS_STATUS_SUCCESS)
        return ESP_OK;
    if (!sm->is_wps_scan)
        return ESP_FAIL;
    if (sm->wps->state != WPS_FINISHED || sm->ap_cred_cnt < 1 ||
        sm->ap_cred_cnt > 3 || sm->ssid_len[0] == 0 ||
        sm->ssid_len[0] > sizeof(config.sta.ssid) ||
        sm->key_len[0] > sizeof(config.sta.password)) {
        goto fail;
    }
    /* Preserve the SDK's effective first-credential selection for multi-AP WPS. */
    memcpy(config.sta.ssid, sm->ssid[0], sm->ssid_len[0]);
    memcpy(config.sta.password, sm->key[0], sm->key_len[0]);
    memcpy(config.sta.bssid, sm->bssid, ETH_ALEN);
    ret = esp_wifi_set_config(WIFI_IF_STA, &config);
    if (ret != ESP_OK)
        goto fail;
    wps_end_attempt(sm);
    wps_set_status(WPS_STATUS_SUCCESS);
    event.event_id = SYSTEM_EVENT_STA_WPS_ER_SUCCESS;
    esp_event_send(&event);
    return ESP_OK;
fail:
    wps_stop_process(WPS_FAIL_REASON_NORMAL);
    return ret;
}

void wifi_station_wps_success_internal(void)
{
    (void)wps_apply_credentials();
}

void wifi_station_wps_success(void)
{
#ifdef USE_WPS_TASK
    wps_post(SIG_WPS_TIMER_SUCCESS_CB, 0);
    return;
#else
    wifi_station_wps_success_internal();
#endif
}

void wifi_station_wps_eapol_start_handle_internal(void)
{
    wpa_printf(MSG_DEBUG, "Resend EAPOL-Start.");
    wps_tx_start();
}

void wifi_station_wps_eapol_start_handle(void)
{
#ifdef USE_WPS_TASK
    wps_post(SIG_WPS_TIMER_EAPOL_START, 0);
    return;
#else
    wifi_station_wps_eapol_start_handle_internal();
#endif
}

int
wifi_station_wps_init(void)
{
    struct wps_funcs *wps_cb;
    struct wps_sm *sm = NULL;
    uint8_t mac[6];

    if (gWpsSm) {
        goto _out;
    }

    wpa_printf(MSG_DEBUG, "wifi sta wps init");

    gWpsSm = (struct wps_sm *)os_zalloc(sizeof(struct wps_sm));   /* alloc Wps_sm */
    if (!gWpsSm) {
        goto _err;
    }

    sm = gWpsSm;
    memset(sm, 0x00, sizeof(struct wps_sm));

    esp_wifi_get_macaddr_internal(WIFI_IF_STA, mac);
    memcpy(sm->ownaddr, mac, ETH_ALEN);

    sm->discover_ssid_cnt = 0;
    sm->ignore_sel_reg = false;
    sm->discard_ap_cnt = 0;
    memset(&sm->dis_ap_list, 0, WPS_MAX_DIS_AP_NUM * sizeof(struct discard_ap_list_t));
    memset(&sm->config, 0x00, sizeof(wifi_sta_config_t));
    sm->eapol_version = 0x1;
    sm->identity_len = 29;
    memcpy(sm->identity, WPS_EAP_EXT_VENDOR_TYPE, sm->identity_len);

    sm->is_wps_scan = false;

    sm->wps_ctx = (struct wps_context *)os_zalloc(sizeof(struct wps_context)); /* alloc wps_ctx */
    if (!sm->wps_ctx) {
        goto _err;
    }

    if (wps_dev_init() != 0) {
        goto _err;
    }

    if ((sm->wps = wps_init()) == NULL) {         /* alloc wps_data */
        goto _err;
    }

    /**************80211 reference***************/

    if (esp_wifi_get_appie_internal(WIFI_APPIE_WPS_PR) == NULL) {            /* alloc probe req wps ie */
        if (wps_build_ic_appie_wps_pr() != ESP_OK)
            goto _err;
    }

    if (esp_wifi_get_appie_internal(WIFI_APPIE_WPS_AR) == NULL) {           /* alloc assoc req wps ie */
        if (wps_build_ic_appie_wps_ar() != ESP_OK)
            goto _err;
    }

    ets_timer_disarm(&sm->wps_timeout_timer);
    ets_timer_setfn(&sm->wps_timeout_timer, (ETSTimerFunc *)wifi_station_wps_timeout, NULL);
    ets_timer_disarm(&sm->wps_msg_timeout_timer);
    ets_timer_setfn(&sm->wps_msg_timeout_timer, (ETSTimerFunc *)wifi_station_wps_msg_timeout, NULL);
    ets_timer_disarm(&sm->wps_success_cb_timer);
    ets_timer_setfn(&sm->wps_success_cb_timer, (ETSTimerFunc *)wifi_station_wps_success, NULL);
    ets_timer_disarm(&sm->wps_scan_timer);
    ets_timer_setfn(&sm->wps_scan_timer, (ETSTimerFunc *)wifi_wps_scan, NULL);
    ets_timer_disarm(&sm->wps_eapol_start_timer);
    ets_timer_setfn(&sm->wps_eapol_start_timer, (ETSTimerFunc *)wifi_station_wps_eapol_start_handle, NULL);

    sm->scan_cnt = 0;

    wps_cb = os_malloc(sizeof(struct wps_funcs));
    if (wps_cb == NULL) {
        goto _err;
    } else {
        wps_cb->wps_parse_scan_result = wps_parse_scan_result;
        wps_cb->wifi_station_wps_start = wifi_station_wps_start;
        wps_cb->wps_sm_rx_eapol = wps_sm_rx_eapol;
        wps_cb->wps_start_pending = wps_start_pending;
        esp_wifi_set_wps_cb_internal(wps_cb);
    }

    return ESP_OK;

_err:
    wps_station_cleanup();
    return ESP_FAIL;
_out:
    return ESP_FAIL;
}

static void wps_disarm_timers(struct wps_sm *sm)
{
    if (!sm)
        return;
    ets_timer_disarm(&sm->wps_success_cb_timer);
    ets_timer_disarm(&sm->wps_timeout_timer);
    ets_timer_disarm(&sm->wps_msg_timeout_timer);
    ets_timer_disarm(&sm->wps_scan_timer);
    ets_timer_disarm(&sm->wps_eapol_start_timer);
    esp_wifi_disarm_sta_connection_timer_internal();
}

int wps_delete_timer(void)
{
    struct wps_sm *sm = gWpsSm;

    if (!sm)
        return ESP_OK;
    wps_disarm_timers(sm);
    ets_timer_done(&sm->wps_success_cb_timer);
    ets_timer_done(&sm->wps_timeout_timer);
    ets_timer_done(&sm->wps_msg_timeout_timer);
    ets_timer_done(&sm->wps_scan_timer);
    ets_timer_done(&sm->wps_eapol_start_timer);
    return ESP_OK;
}

static void wps_station_cleanup(void)
{
    struct wps_sm *sm = gWpsSm;

    /* The native callback setter owns its table; detach before freeing context. */
    esp_wifi_set_wps_cb_internal(NULL);
    esp_wifi_unset_appie_internal(WIFI_APPIE_WPS_PR);
    esp_wifi_unset_appie_internal(WIFI_APPIE_WPS_AR);
    wps_end_attempt(sm);
    wps_delete_timer();
    if (sm) {
        if (sm->wps) {
            wps_deinit();
            sm->wps = NULL;
        }
        wps_dev_deinit(sm->dev);
        sm->dev = NULL;
        os_free(sm->wps_ctx);
        sm->wps_ctx = NULL;
        os_free(sm);
        gWpsSm = NULL;
    }
    /* Factory data exists before the state machine/device can be allocated. */
    os_free(s_factory_info);
    s_factory_info = NULL;
}

int
wifi_station_wps_deinit(void)
{
    wps_station_cleanup();
    return ESP_OK;
}

int
wps_station_wps_register_cb(wps_st_cb_t cb)
{
    if (!gWpsSm) {
        return ESP_FAIL;
    }

    gWpsSm->st_cb = cb;
    return ESP_OK;
}

struct wps_sm *
wps_sm_get(void)
{
    return gWpsSm;
}

int
wps_ssid_save(u8 *ssid, u8 ssid_len, u8 idx)
{
    if (!ssid || !gWpsSm || idx > 2 || ssid_len > sizeof(gWpsSm->ssid[idx])) {
        return ESP_FAIL;
    }

    memset(gWpsSm->ssid[idx], 0x00, sizeof(gWpsSm->ssid[idx]));
    memcpy(gWpsSm->ssid[idx], ssid, ssid_len);
    gWpsSm->ssid_len[idx] = ssid_len;
    gWpsSm->ap_cred_cnt++;

    return ESP_OK;
}

int
wps_key_save(char *key, u8 key_len, u8 idx)
{
    if (!key || !gWpsSm || idx > 2 || key_len > sizeof(gWpsSm->key[idx])) {
        return ESP_FAIL;
    }

    memset(gWpsSm->key[idx], 0x00, sizeof(gWpsSm->key[idx]));
    memcpy(gWpsSm->key[idx], key, key_len);
    gWpsSm->key_len[idx] = key_len;

    return ESP_OK;
}

void
wifi_wps_scan_done(void *arg, STATUS status)
{
    struct wps_sm *sm = gWpsSm;
    wifi_config_t wifi_config;

    if (wps_get_type() == WPS_TYPE_DISABLE) {
        return;
    }

    if (!sm || !sm->is_wps_scan) {
        return;
    }

    if (sm->discover_ssid_cnt == 1) {
        wps_set_status(WPS_STATUS_PENDING);
    } else if (sm->discover_ssid_cnt == 0)  {
        wps_set_status(WPS_STATUS_SCANNING);
    } else {
        wpa_printf(MSG_INFO, "PBC session overlap!");
        wps_set_status(WPS_STATUS_DISABLE);
    }

    wpa_printf(MSG_DEBUG, "wps scan_done discover_ssid_cnt = %d", sm->discover_ssid_cnt);

    sm->discover_ssid_cnt = 0;

    if (wps_get_status() == WPS_STATUS_PENDING) {
        esp_wifi_disconnect();

        memcpy(&wifi_config.sta, &sm->config, sizeof(wifi_sta_config_t));
        esp_wifi_set_config(0, &wifi_config);

        wpa_printf(MSG_DEBUG, "WPS: neg start");
        esp_wifi_connect();
    } else if (wps_get_status() == WPS_STATUS_SCANNING) {
        if (sm->scan_cnt < WPS_IGNORE_SEL_REG_MAX_CNT) {
            sm->ignore_sel_reg = true;
        }
        ets_timer_arm(&sm->wps_scan_timer, 100, 0);
    } else {
        return;
    }
}

void
wifi_wps_scan_internal(void)
{
    struct wps_sm *sm = gWpsSm;

    if (!sm || !sm->is_wps_scan)
        return;
    sm->scan_cnt++;
    wpa_printf(MSG_DEBUG, "wifi_wps_scan : %d", sm->scan_cnt);

    typedef void (* scan_done_cb_t)(void *arg, STATUS status);
    extern int esp_wifi_promiscuous_scan_start(wifi_scan_config_t *config, scan_done_cb_t cb);
    esp_wifi_promiscuous_scan_start(NULL, wifi_wps_scan_done);
}

void wifi_wps_scan(void)
{
#ifdef USE_WPS_TASK
    wps_post(SIG_WPS_TIMER_SCAN, 0);
    return;
#else
    wifi_wps_scan_internal();
#endif
}

int wifi_station_wps_start(void)
{
    struct wps_sm *sm = wps_sm_get();

    if (!sm) {
        wpa_printf(MSG_ERROR, "WPS: wps not initial");
        return ESP_FAIL;
    }

    ets_timer_arm(&sm->wps_timeout_timer, 120000, 0); /* 120s total */

    switch (wps_get_status()) {
    case WPS_STATUS_DISABLE: {
        wps_clear_fragment();
        sm->is_wps_scan = true;

        if (wps_build_public_key(sm->wps, NULL, WPS_CALC_KEY_PRE_CALC) != 0) {
            sm->is_wps_scan = false;
            ets_timer_disarm(&sm->wps_timeout_timer);
            esp_wifi_set_wps_start_flag_internal(false);
            return ESP_ERR_NO_MEM;
        }

        wifi_wps_scan();


        break;
    }
    case WPS_STATUS_SCANNING:
        sm->scan_cnt = 0;
        ets_timer_disarm(&sm->wps_timeout_timer);
        ets_timer_arm(&sm->wps_timeout_timer, 120000, 0); /* 120s total */
        break;
    default:
        break;
    }
    esp_wifi_set_wps_start_flag_internal(true);
    return ESP_OK;
}

int wps_task_deinit(void)
{
    wpa_printf(MSG_DEBUG, "wps task deinit");

    if (s_wps_api_sem) {
        vSemaphoreDelete(s_wps_api_sem);
        s_wps_api_sem = NULL;
        wpa_printf(MSG_DEBUG, "wps task deinit: free api sem");
    }

    if (s_wps_task_create_sem) {
        vSemaphoreDelete(s_wps_task_create_sem);
        s_wps_task_create_sem = NULL;
        wpa_printf(MSG_DEBUG, "wps task deinit: free task create sem");
    }

    if (s_wps_queue) {
        vQueueDelete(s_wps_queue);
        s_wps_queue = NULL;
        wpa_printf(MSG_DEBUG, "wps task deinit: free queue");
    }

    if (s_wps_task_hdl) {
        vTaskDelete(s_wps_task_hdl);
        s_wps_task_hdl = NULL;
        wpa_printf(MSG_DEBUG, "wps task deinit: free task");
    }

    if (STAILQ_FIRST(&s_wps_rxq) != NULL){
        wps_rxq_deinit();
    }

    if (s_wps_data_lock) {
        vSemaphoreDelete(s_wps_data_lock);
        s_wps_data_lock = NULL;
        wpa_printf(MSG_DEBUG, "wps task deinit: free data lock");
    }

    return ESP_OK;
}

int wps_task_init(void)
{
    int ret = 0;

    /* Call wps_task_deinit() first in case esp_wifi_wps_disable() fails
     */
    wps_task_deinit();

    s_wps_data_lock = xSemaphoreCreateRecursiveMutex();
    if (!s_wps_data_lock) {
        wpa_printf(MSG_ERROR, "wps task init: failed to alloc data lock");
        goto _wps_no_mem;
    }

    s_wps_api_sem = xSemaphoreCreateCounting(1, 0);
    if (!s_wps_api_sem) {
        wpa_printf(MSG_ERROR, "wps task init: failed to create api sem");
        goto _wps_no_mem;
    }

    s_wps_task_create_sem = xSemaphoreCreateCounting(1, 0);
    if (!s_wps_task_create_sem) {
        wpa_printf(MSG_ERROR, "wps task init: failed to create task sem");
        goto _wps_no_mem;
    }

    os_bzero(s_wps_sig_cnt, SIG_WPS_NUM);
    s_wps_queue = xQueueCreate(SIG_WPS_NUM, sizeof(ETSEvent));
    if (!s_wps_queue) {
        wpa_printf(MSG_ERROR, "wps task init: failed to alloc queue");
        goto _wps_no_mem;
    }

    wps_rxq_init();

    ret = xTaskCreate(wps_task, "wpsT", WPS_TASK_STACK_SIZE, NULL, 2, &s_wps_task_hdl);
    if (pdPASS != ret) {
        wpa_printf(MSG_ERROR, "wps enable: failed to create task");
        goto _wps_no_mem;
    }

    xSemaphoreTake(s_wps_task_create_sem, portMAX_DELAY);
    vSemaphoreDelete(s_wps_task_create_sem);
    s_wps_task_create_sem = NULL;

    wpa_printf(MSG_DEBUG, "wifi wps enable: task prio:%d, stack:%d", 2, WPS_TASK_STACK_SIZE);
    return ESP_OK;

_wps_no_mem:
    wps_task_deinit();
    return ESP_ERR_NO_MEM;
}

int wps_post_block(uint32_t sig, void *arg)
{
    wps_ioctl_param_t param;

    param.ret = ESP_FAIL;
    param.arg = arg;

    if (ESP_OK != wps_post(sig, (uint32_t)&param)) {
        return ESP_FAIL;
    }

    if (pdPASS == xSemaphoreTake(s_wps_api_sem, portMAX_DELAY)) {
        return param.ret;
    } else {
        return ESP_FAIL;
    }
}

int wps_check_wifi_mode(void)
{
    bool sniffer = false;
    wifi_mode_t mode;
    int ret;

    ret = esp_wifi_get_mode(&mode);
    if (ESP_OK != ret) {
        wpa_printf(MSG_ERROR, "wps check wifi mode: failed to get wifi mode ret=%d", ret);
        return ESP_FAIL;
    }

    ret = esp_wifi_get_promiscuous(&sniffer);
    if (ESP_OK != ret) {
        wpa_printf(MSG_ERROR, "wps check wifi mode: failed to get sniffer mode ret=%d", ret);
        return ESP_FAIL;
    }

    if (mode == WIFI_MODE_AP || mode == WIFI_MODE_NULL || sniffer == true) {
        wpa_printf(MSG_ERROR, "wps check wifi mode: wrong wifi mode=%d sniffer=%d", mode, sniffer);
        return ESP_ERR_WIFI_MODE;
    }

    return ESP_OK;
}

int esp_wifi_wps_enable(const esp_wps_config_t *config)
{
    int ret;

    if (ESP_OK != wps_check_wifi_mode()) {
        return ESP_ERR_WIFI_MODE;
    }

    API_MUTEX_TAKE();
    if (s_wps_enabled) {
        API_MUTEX_GIVE();
        wpa_printf(MSG_DEBUG, "wps enable: already enabled");
        return ESP_OK;
    }

#ifdef USE_WPS_TASK
    ret = wps_task_init();
    if (ESP_OK != ret) {
        API_MUTEX_GIVE();
        return ret;
    }

    ret = wps_post_block(SIG_WPS_ENABLE, (esp_wps_config_t *)config);
    if (ESP_OK != ret) {
        wps_task_deinit();
        API_MUTEX_GIVE();
        return ret;
    }

    s_wps_enabled = true;
    wpa_printf(MSG_DEBUG, "wifi wps task: prio:%d, stack:%d\n", 2, WPS_TASK_STACK_SIZE);
    API_MUTEX_GIVE();
    return ret;
#else
    ret = wifi_wps_enable_internal(config);
    API_MUTEX_GIVE();
    return ret;
#endif
}

int wifi_wps_enable_internal(const esp_wps_config_t *config)
{
    int ret = 0;

    wpa_printf(MSG_DEBUG, "ESP WPS crypto initialize!");
    if (config->wps_type == WPS_TYPE_DISABLE) {
        wpa_printf(MSG_ERROR, "wps enable: invalid wps type");
        return ESP_ERR_WIFI_WPS_TYPE;
    }

    /* currently , we don't support REGISTRAR */
    if (IS_WPS_REGISTRAR(config->wps_type)) {
        wpa_printf(MSG_ERROR, "wps enable: not support registrar");
        return ESP_ERR_WIFI_WPS_TYPE;
    }

    wpa_printf(MSG_DEBUG, "Set factory information.");
    ret = wps_set_factory_info(config);
    if (ret != 0) {
        return ret;
    }

    wpa_printf(MSG_INFO, "wifi_wps_enable\n");

    wps_set_type(config->wps_type);
    wps_set_status(WPS_STATUS_DISABLE);

    ret = wifi_station_wps_init();

    if (ret != 0) {
        wps_set_type(WPS_STATUS_DISABLE);
        wps_set_status(WPS_STATUS_DISABLE);
        return ESP_FAIL;
    }

    return ESP_OK;
}

int wifi_wps_disable_internal(void)
{
    wps_set_status(WPS_STATUS_DISABLE);
    wifi_station_wps_deinit();
    return ESP_OK;
}

int esp_wifi_wps_disable(void)
{
    int ret = 0;

    if (ESP_OK != wps_check_wifi_mode()) {
        return ESP_ERR_WIFI_MODE;
    }

    API_MUTEX_TAKE();

    if (!s_wps_enabled) {
        wpa_printf(MSG_DEBUG, "wps disable: already disabled");
        API_MUTEX_GIVE();
        return ESP_OK;
    }

    wpa_printf(MSG_INFO, "wifi_wps_disable\n");
    wps_set_type(WPS_TYPE_DISABLE); /* Notify WiFi task */

    /* Call wps_delete_timer to delete all WPS timer, no timer will call wps_post()
     * to post message to wps_task once this function returns.
     */
    wps_delete_timer();

#ifdef USE_WPS_TASK
    ret = wps_post_block(SIG_WPS_DISABLE, 0);
#else
    ret = wifi_wps_disable_internal();
#endif

    if (ESP_OK != ret) {
        wpa_printf(MSG_ERROR, "wps disable: failed to disable wps, ret=%d", ret);
        API_MUTEX_GIVE();
        return ret;
    }

    esp_wifi_disconnect();
    esp_wifi_set_wps_start_flag_internal(false);
    wps_task_deinit();
    s_wps_enabled = false;
    API_MUTEX_GIVE();
    return ESP_OK;
}

int esp_wifi_wps_start(int timeout_ms)
{
    int ret = ESP_OK;

    if (ESP_OK != wps_check_wifi_mode()) {
        return ESP_ERR_WIFI_MODE;
    }

    API_MUTEX_TAKE();

    if (!s_wps_enabled) {
        wpa_printf(MSG_ERROR, "wps start: wps not enabled");
        API_MUTEX_GIVE();
        return ESP_ERR_WIFI_WPS_SM;
    }

    if (wps_get_type() == WPS_TYPE_DISABLE || (wps_get_status() != WPS_STATUS_DISABLE && wps_get_status() != WPS_STATUS_SCANNING)) {
        API_MUTEX_GIVE();
        return ESP_ERR_WIFI_WPS_TYPE;
    }

    if (esp_wifi_get_user_init_flag_internal() == 0) {
        API_MUTEX_GIVE();
        return ESP_ERR_WIFI_STATE;
    }

    wpa_printf(MSG_DEBUG, "wps scan");

#ifdef USE_WPS_TASK
    ret = wps_post_block(SIG_WPS_START, 0);
#else
    ic_pp_post(SIG_PP_WPS, 0);
#endif

    API_MUTEX_GIVE();
    return ret;
}

bool
wifi_set_wps_cb(wps_st_cb_t cb)
{
    wifi_mode_t mode;

    esp_wifi_get_mode(&mode);
    if (mode == WIFI_MODE_AP || mode == WIFI_MODE_NULL) {
        return false;
    }

    if (wps_station_wps_register_cb(cb) == 0) {
        return true;
    }

    return false;
}
