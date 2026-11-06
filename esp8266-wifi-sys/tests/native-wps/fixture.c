/* Optional native WPS regression fixture; see README.md for boundaries. */
#include "utils/includes.h"
#include "utils/common.h"
#include "common/eapol_common.h"
#include "eap_peer/eap_defs.h"
#include "eap_peer/eap_common.h"
#include "common/ieee802_11_defs.h"
#include "rsn_supp/wpa.h"
#include "wps/wps_i.h"
#include "esp_wifi_driver.h"
#include "esp_event.h"
#include "esp_private/wifi.h"
#include "crypto/dh_group5.h"

#define CHECK(c) do { \
    if (!(c)) \
        fixture_fail(__LINE__); \
} while (0)

extern void fixture_fail(unsigned);
extern void fixture_note(unsigned, unsigned);
extern void *fixture_guard(const void *, unsigned);
static void test_receive_and_scan(void);
extern void fixture_worker_create(void (*)(void *));
extern void fixture_worker_pump(void), fixture_worker_yield(void), fixture_worker_stop(void),
    fixture_worker_delete(void);
extern struct wps_sm *gWpsSm;
extern int wifi_wps_enable_internal(const esp_wps_config_t *), wifi_wps_disable_internal(void),
    wps_finish(void), wps_post(uint32_t, uint32_t), wifi_station_wps_start(void);
extern void wifi_station_wps_success_internal(void), wifi_station_wps_timeout_internal(void),
    wifi_station_wps_msg_timeout_internal(void);
/* Track ownership and check each allocation tail on release. */
static union {
    long long align;
    unsigned char data[16384];
} blocks[128];
static unsigned char live[128];
static size_t allocation_sizes[128];
static unsigned calls, fail_at, live_count, fail_all;
static void *alloc(size_t size)
{
    unsigned i;
    calls++;
    if (fail_all || calls == fail_at)
        return NULL;
    CHECK(size <= sizeof(blocks[0].data));
    for (i = 0; i < 128; i++) {
        if (!live[i]) {
            live[i] = 1;
            allocation_sizes[i] = size;
            live_count++;
            memset(blocks[i].data, 0xa7, sizeof(blocks[i].data));
            return blocks[i].data;
        }
    }
    fixture_fail(__LINE__);
    return NULL;
}

static void drop(void *ptr)
{
    unsigned i, j;
    if (!ptr)
        return;
    for (i = 0; i < 128; i++) {
        if (ptr == blocks[i].data) {
            CHECK(live[i]);
            for (j = allocation_sizes[i]; j < sizeof(blocks[i].data); j++)
                CHECK(blocks[i].data[j] == 0xa7);
            live[i] = 0;
            live_count--;
            return;
        }
    }
    fixture_fail(__LINE__);
}

/* SDK allocator boundary: fail one request or all requests deterministically. */
void *_heap_caps_malloc(size_t n, uint32_t caps, const char *f, size_t l)
{
    return alloc(n);
}

void *_heap_caps_zalloc(size_t n, uint32_t caps, const char *f, size_t l)
{
    void *p = alloc(n);
    if (p)
        memset(p, 0, n);
    return p;
}

void _heap_caps_free(void *p, const char *f, size_t l)
{
    drop(p);
}

unsigned int wps_generate_pin(void)
{
    return 12345670;
}

static int type, status, start_flag, config_reject, apply_count, successes, failures, timeouts,
    dh_fail;
static struct wifi_appie *ies[16];
static struct wps_funcs *callbacks;
static wifi_config_t applied;
int esp_wifi_get_wps_type_internal(void)
{
    return type;
}

int esp_wifi_set_wps_type_internal(uint32_t v)
{
    type = v;
    return 0;
}

int esp_wifi_get_wps_status_internal(void)
{
    return status;
}

int esp_wifi_set_wps_status_internal(uint32_t v)
{
    status = v;
    return 0;
}

esp_err_t esp_wifi_set_wps_start_flag_internal(bool v)
{
    start_flag = v;
    return 0;
}

int esp_wifi_get_macaddr_internal(uint8_t i, uint8_t *p)
{
    memset(p, 0x22, 6);
    return 0;
}

int esp_wifi_disarm_sta_connection_timer_internal(void)
{
    return 0;
}

int esp_wifi_set_wps_cb_internal(struct wps_funcs *p)
{
    drop(callbacks);
    callbacks = p;
    return 0;
}

struct wifi_appie *esp_wifi_get_appie_internal(uint8_t t)
{
    return ies[t];
}

int esp_wifi_set_appie_internal(uint8_t t, uint8_t *p, uint16_t n, uint8_t flag)
{
    void *q;
    CHECK(flag == 0);
    q = alloc(n + 2);
    if (!q)
        return -1;
    memcpy(q, p, n);
    CHECK(!ies[t]);
    ies[t] = q;
    return 0;
}

int esp_wifi_unset_appie_internal(uint8_t t)
{
    drop(ies[t]);
    ies[t] = NULL;
    return 0;
}

esp_err_t esp_wifi_set_config(wifi_interface_t i, wifi_config_t *p)
{
    apply_count++;
    memcpy(&applied, p, sizeof(*p));
    return config_reject ? ESP_FAIL : ESP_OK;
}

esp_err_t esp_wifi_disconnect(void)
{
    return 0;
}

esp_err_t esp_wifi_connect(void)
{
    return 0;
}

esp_err_t esp_wifi_get_mode(wifi_mode_t *p)
{
    *p = WIFI_MODE_STA;
    return 0;
}

esp_err_t esp_wifi_get_promiscuous(bool *p)
{
    *p = false;
    return 0;
}

uint8_t esp_wifi_get_user_init_flag_internal(void)
{
    return 1;
}

esp_err_t esp_event_send(system_event_t *e)
{
    system_event_t expected = {
        0
    };
    expected.event_id = e->event_id;
    if (e->event_id == SYSTEM_EVENT_STA_WPS_ER_FAILED) {
        expected.event_info.sta_er_fail_reason = e->event_info.sta_er_fail_reason;
        failures++;
    } else if (e->event_id == SYSTEM_EVENT_STA_WPS_ER_SUCCESS) {
        successes++;
    } else if (e->event_id == SYSTEM_EVENT_STA_WPS_ER_TIMEOUT) {
        timeouts++;
    } else {
        return 0;
    }
    CHECK(memcmp(e, &expected, sizeof(expected)) == 0);
    return 0;
}

void uuid_gen_mac_addr(const u8 *m, u8 *u)
{
    memset(u, 0x44, 16);
}

void dh5_free(void *p)
{
    CHECK(!p);
}

int __wrap_wps_build_public_key(struct wps_data *w, struct wpabuf *m, wps_key_mode_t mode)
{
    return dh_fail ? -1 : 0;
}

/* Timer stubs require disarm-before-done, including partial initialization. */
static ETSTimer *timers[32];
static unsigned char armed[32];
static unsigned timer_index(ETSTimer *t)
{
    unsigned i;
    for (i = 0; i < 32; i++) {
        if (timers[i] == t)
            return i;
    }
    for (i = 0; i < 32; i++) {
        if (!timers[i]) {
            timers[i] = t;
            return i;
        }
    }
    fixture_fail(__LINE__);
    return 0;
}

void os_timer_disarm(ETSTimer *t)
{
    armed[timer_index(t)] = 0;
}

void os_timer_arm(ETSTimer *t, uint32_t n, bool repeat)
{
    armed[timer_index(t)] = 1;
}

void os_timer_setfn(ETSTimer *t, ETSTimerFunc *f, void *a)
{
    timer_index(t);
}

void ets_timer_done(ETSTimer *t)
{
    unsigned i = timer_index(t);
    CHECK(!armed[i]);
    timers[i] = NULL;
}

/* Cooperative value queue and task dependencies; scheduling lives in host.c. */
struct queue {
    unsigned capacity, item_size, count, head;
    unsigned char items[10][8];
};
static int send_fail;
static void *task_handle;
static struct queue *event_queue;
QueueHandle_t xQueueCreateMutex(uint8_t kind)
{
    struct queue *q = alloc(sizeof(*q));
    if (q)
        memset(q, 0, sizeof(*q));
    return q;
}

QueueHandle_t xQueueCreateCountingSemaphore(UBaseType_t max, UBaseType_t init)
{
    struct queue *q = xQueueCreateMutex(0);
    if (q)
        q->count = init;
    return q;
}

QueueHandle_t xQueueGenericCreate(UBaseType_t n, UBaseType_t size, uint8_t kind)
{
    struct queue *q = xQueueCreateMutex(0);
    if (q) {
        CHECK(n == 10 && size == 8);
        q->capacity = n;
        q->item_size = size;
        event_queue = q;
    }
    return q;
}

BaseType_t xQueueGiveMutexRecursive(QueueHandle_t q)
{
    CHECK(q);
    return pdPASS;
}

BaseType_t xQueueTakeMutexRecursive(QueueHandle_t q, TickType_t ticks)
{
    CHECK(q);
    return pdPASS;
}

BaseType_t xQueueGenericSend(QueueHandle_t h, const void *item, TickType_t ticks, BaseType_t pos)
{
    struct queue *q = h;
    CHECK(q);
    if (!q->item_size) {
        q->count = 1;
        return pdPASS;
    }
    if (send_fail) {
        send_fail = 0;
        return pdFAIL;
    }
    CHECK(q->count < q->capacity);
    memcpy(q->items[(q->head + q->count) % 10], item, q->item_size);
    q->count++;
    return pdPASS;
}

BaseType_t xQueueSemaphoreTake(QueueHandle_t h, TickType_t ticks)
{
    struct queue *q = h;
    CHECK(q);
    if (!q->count)
        fixture_worker_pump();
    CHECK(q->count);
    q->count--;
    return pdPASS;
}

BaseType_t xQueueReceive(QueueHandle_t h, void *p, TickType_t ticks)
{
    struct queue *q = h;
    while (!q->count)
        fixture_worker_yield();
    memcpy(p, q->items[q->head], q->item_size);
    q->head = (q->head + 1) % 10;
    q->count--;
    return pdPASS;
}

void vQueueDelete(QueueHandle_t q)
{
    if (q == event_queue)
        event_queue = NULL;
    drop(q);
}

BaseType_t xTaskCreate(TaskFunction_t entry, const char *name, uint16_t depth, void *arg,
    UBaseType_t pri, TaskHandle_t *out)
{
    task_handle = alloc(16);
    if (!task_handle)
        return errCOULD_NOT_ALLOCATE_REQUIRED_MEMORY;
    *out = task_handle;
    fixture_worker_create(entry);
    return pdPASS;
}

void vTaskDelete(TaskHandle_t h)
{
    drop(task_handle);
    task_handle = NULL;
    if (h)
        fixture_worker_delete();
    else
        fixture_worker_stop();
}

int esp_wifi_promiscuous_scan_start(wifi_scan_config_t *c, void (*f)(void *, STATUS))
{
    return 0;
}

static void clean(void)
{
    CHECK(!gWpsSm && !callbacks && !ies[WIFI_APPIE_WPS_PR] && !ies[WIFI_APPIE_WPS_AR]);
}

static esp_wps_config_t cfg;
static void prepare_credentials(unsigned count)
{
    unsigned i;
    gWpsSm->is_wps_scan = true;
    gWpsSm->wps->state = WPS_FINISHED;
    gWpsSm->ap_cred_cnt = count;
    for (i = 0; i < count; i++) {
        memset(gWpsSm->ssid[i], 'A' + i, 4);
        gWpsSm->ssid_len[i] = 4;
        memset(gWpsSm->key[i], 'a' + i, 8);
        gWpsSm->key_len[i] = 8;
    }
    status = WPS_STATUS_PENDING;
    apply_count = successes = failures = timeouts = 0;
}

/* Lifecycle faults, terminal publication, retries and control-message ownership. */
void fixture_tests(void)
{
    unsigned total, i, before, mode;
    u8 packet[8] = {
        0
    }, mac[6] = {
        0
    };
    memset(&cfg, 0, sizeof(cfg));
    cfg.wps_type = WPS_TYPE_PBC;
    fail_at = 1;
    CHECK(esp_wifi_wps_enable(&cfg) != 0);
    clean();
    CHECK(live_count == 0);
    fail_at = 0;
    CHECK(esp_wifi_wps_enable(&cfg) == 0);
    CHECK(gWpsSm && callbacks);
    CHECK(esp_wifi_wps_disable() == 0);
    clean();
    CHECK(live_count == 1);
    for (mode = WPS_TYPE_PBC; mode <= WPS_TYPE_PIN; mode++) {
        cfg.wps_type = mode;
        memset(cfg.factory_info.manufacturer, 'm', sizeof(cfg.factory_info.manufacturer) -1);
        cfg.factory_info.manufacturer[0] = '%';
        calls = 0;
        CHECK(esp_wifi_wps_enable(&cfg) == 0);
        total = calls;
        CHECK(memcmp(gWpsSm->dev->manufacturer, cfg.factory_info.manufacturer,
            sizeof(cfg.factory_info.manufacturer)) == 0);
        CHECK(esp_wifi_wps_disable() == 0);
        CHECK(live_count == 1);
        fixture_note(mode, total);
        for (i = 1; i <= total; i++) {
            calls = 0;
            fail_at = i;
            CHECK(esp_wifi_wps_enable(&cfg) != 0);
            clean();
            CHECK(live_count == 1);
            fail_at = 0;
            CHECK(esp_wifi_wps_enable(&cfg) == 0);
            CHECK(esp_wifi_wps_disable() == 0);
            clean();
            CHECK(live_count == 1);
        }
    }
    /* Completion applies credential zero exactly once, without allocating. */
    cfg.wps_type = WPS_TYPE_PBC;
    for (i = 1; i <= 3; i += 2) {
        CHECK(esp_wifi_wps_enable(&cfg) == 0);
        prepare_credentials(i);
        before = calls;
        CHECK(wps_finish() == 0);
        CHECK(calls == before && apply_count == 1 && successes == 1 && failures == 0);
        CHECK(applied.sta.ssid[0] == 'A' && applied.sta.password[0] == 'a');
        wifi_station_wps_timeout_internal();
        wifi_station_wps_msg_timeout_internal();
        wifi_station_wps_success_internal();
        CHECK(apply_count == 1 && successes == 1 && failures == 0 && timeouts == 0);
        CHECK(esp_wifi_wps_disable() == 0);
        clean();
    }
    CHECK(esp_wifi_wps_enable(&cfg) == 0);
    prepare_credentials(1);
    config_reject = 1;
    CHECK(wps_finish() != 0);
    CHECK(apply_count == 1 && failures == 1 && successes == 0);
    wifi_station_wps_timeout_internal();
    wifi_station_wps_msg_timeout_internal();
    wifi_station_wps_success_internal();
    CHECK(failures == 1 && timeouts == 0 && successes == 0);
    config_reject = 0;
    CHECK(esp_wifi_wps_disable() == 0);
    clean();
    CHECK(esp_wifi_wps_enable(&cfg) == 0);
    prepare_credentials(0);
    CHECK(wps_finish() != 0);
    CHECK(!apply_count && failures == 1 && !successes);
    CHECK(esp_wifi_wps_disable() == 0);
    clean();
    CHECK(esp_wifi_wps_enable(&cfg) == 0);
    prepare_credentials(1);
    gWpsSm->ssid_len[0] = 33;
    CHECK(wps_finish() != 0);
    CHECK(!apply_count && failures == 1 && !successes);
    CHECK(esp_wifi_wps_disable() == 0);
    clean();
    CHECK(esp_wifi_wps_enable(&cfg) == 0);
    prepare_credentials(1);
    CHECK(callbacks->wps_sm_rx_eapol(mac, packet, sizeof(packet)) == 0);
    CHECK(wps_post(SIG_WPS_TIMER_TIMEOUT, 0) == 0);
    CHECK(wps_post(SIG_WPS_TIMER_MSG_TIMEOUT, 0) == 0);
    CHECK(wps_finish() == 0);
    fail_all = 1;
    CHECK(esp_wifi_wps_disable() == 0);
    fail_all = 0;
    CHECK(successes == 1 && !failures && !timeouts);
    clean();
    CHECK(live_count == 1);
    CHECK(esp_wifi_wps_enable(&cfg) == 0);
    dh_fail = 1;
    CHECK(esp_wifi_wps_start(0) == ESP_ERR_NO_MEM);
    CHECK(!gWpsSm->is_wps_scan && !start_flag && !armed[timer_index(&gWpsSm->wps_timeout_timer)]);
    dh_fail = 0;
    CHECK(esp_wifi_wps_start(0) == 0);
    CHECK(gWpsSm->is_wps_scan && start_flag);
    before = calls;
    CHECK(wps_post(SIG_WPS_TIMER_SCAN, 123) == 0);
    CHECK(wps_post(SIG_WPS_TIMER_SCAN, 456) == 0);
    CHECK(event_queue->count == 1 && calls == before);
    CHECK(((ETSEvent *) event_queue->items[event_queue->head])->par == 123);
    CHECK(wps_post(0, 0) != 0 && wps_post(SIG_WPS_NUM, 0) != 0);
    send_fail = 1;
    CHECK(esp_wifi_wps_disable() != 0);
    CHECK(gWpsSm && callbacks);
    fail_all = 1;
    CHECK(esp_wifi_wps_disable() == 0);
    fail_all = 0;
    clean();
    CHECK(live_count == 1);
    /* Each queued packet owns one allocation; a failed post is drained by the
     * next RX event or normal teardown, without a second payload owner. */
    CHECK(esp_wifi_wps_enable(&cfg) == 0);
    total = live_count;
    before = calls;
    fail_at = calls + 1;
    CHECK(callbacks->wps_sm_rx_eapol(mac, packet, sizeof(packet)) == ESP_ERR_NO_MEM);
    fail_at = 0;
    CHECK(calls == before + 1 && live_count == total);
    before = calls;
    send_fail = 1;
    CHECK(callbacks->wps_sm_rx_eapol(mac, packet, sizeof(packet)) != ESP_OK);
    CHECK(calls == before + 1 && live_count == total + 1);
    before = calls;
    CHECK(callbacks->wps_sm_rx_eapol(mac, packet, sizeof(packet)) == ESP_OK);
    CHECK(calls == before + 1 && live_count == total + 2);
    fixture_worker_pump();
    CHECK(live_count == total);
    CHECK(esp_wifi_wps_disable() == 0);
    clean();
    CHECK(live_count == 1);
    test_receive_and_scan();
}

extern int wps_process_wps_mX_req(u8 *, int, enum wps_process_res *), wps_send_wps_mX_rsp(u8),
    wps_send_eap_identity_rsp(u8), wps_stop_process(system_event_sta_wps_fail_reason_t),
        wps_sm_rx_eapol_internal(u8 *, u8 *, u32);
extern void esp8266_hal_wps_set_target_ssid(const unsigned char *, unsigned);
static unsigned processed, sends, assoc_fail;
static u8 processed_bytes[16];
static size_t processed_len;
bool esp_wifi_enable_sta_privacy_internal(void)
{
    return true;
}

int esp_wifi_internal_tx(wifi_interface_t i, void *p, uint16_t n)
{
    sends++;
    return 0;
}

int esp_wifi_get_assoc_bssid_internal(uint8_t *p)
{
    memset(p, 0x33, 6);
    return assoc_fail ? -1 : 0;
}

struct wpabuf *wps_enrollee_get_msg(struct wps_data *w, enum wsc_op_code *op)
{
    *op = WSC_MSG;
    return wpabuf_alloc_copy("body", 4);
}

enum wps_process_res wps_enrollee_process_msg(struct wps_data *w, enum wsc_op_code op,
    const struct wpabuf *msg)
{
    processed++;
    processed_len = wpabuf_len(msg);
    CHECK(processed_len <= sizeof(processed_bytes));
    memcpy(processed_bytes, wpabuf_head(msg), processed_len);
    return WPS_CONTINUE;
}

static int input(const u8 *p, unsigned n, int length)
{
    enum wps_process_res result;
    return wps_process_wps_mX_req(fixture_guard(p, n), length, &result);
}

/* Real parser/buffer helpers, with controlled enrollee crypto/TX boundaries. */
static void test_receive_and_scan(void)
{
    unsigned i, base, before, tx, handled;
    u8 first[] = {
        0, 0, 0, 0, 0, 0, 0, WSC_MSG, WPS_MSG_FLAG_LEN | WPS_MSG_FLAG_MORE, 0, 4, 1, 2
    };
    u8 last[] = {
        0, 0, 0, 0, 0, 0, 0, WSC_MSG, 0, 3, 4
    };
    u8 overflow[] = {
        0, 0, 0, 0, 0, 0, 0, WSC_MSG, 0, 3, 4, 5
    };
    u8 full[] = {
        0, 0, 0, 0, 0, 0, 0, WSC_MSG, WPS_MSG_FLAG_LEN | WPS_MSG_FLAG_MORE, 0, 4, 1, 2, 3, 4
    };
    u8 oversized[] = {
        0, 0, 0, 0, 0, 0, 0, WSC_MSG, WPS_MSG_FLAG_LEN, 0, 4, 1
    };
    /* Exact-length guard pages reject over-reads as well as malformed results. */
    CHECK(sizeof(struct eap_expand) == 8);
    CHECK(esp_wifi_wps_enable(&cfg) == 0);
    CHECK(esp_wifi_wps_start(0) == 0);
    base = live_count;
    handled = processed;
    tx = sends;
    for (i = 0; i < 9; i++) {
        CHECK(input(first, i, i) != 0);
        CHECK(live_count == base && processed == handled && sends == tx);
    }
    for (i = 9; i < 11; i++) {
        CHECK(input(first, i, i) != 0);
        CHECK(live_count == base && processed == handled && sends == tx);
    }
    CHECK(input(first, 0, -1) != 0);
    CHECK(input(oversized, sizeof(oversized), sizeof(oversized)) != 0);
    CHECK(input(full, sizeof(full), sizeof(full)) != 0);
    CHECK(live_count == base && processed == handled && sends == tx);
    for (i = 1; i <= 3; i++) {
        fail_at = calls + i;
        handled = processed;
        CHECK(input(first, sizeof(first), sizeof(first)) != 0);
        fail_at = 0;
        CHECK(live_count == base && processed == handled);
    }
    {
        u8 short_total[sizeof(first)];
        memcpy(short_total, first, sizeof(first));
        short_total[10] = 1;
        CHECK(input(short_total, sizeof(short_total), sizeof(short_total)) != 0);
        CHECK(live_count == base && processed == handled);
    }
    CHECK(input(first, sizeof(first), sizeof(first)) == 0);
    CHECK(live_count == base + 1);
    CHECK(input(overflow, sizeof(overflow), sizeof(overflow)) != 0);
    CHECK(live_count == base && processed == handled);
    CHECK(input(first, sizeof(first), sizeof(first)) == 0);
    CHECK(input(last, sizeof(last), sizeof(last)) == 0);
    CHECK(processed == handled + 1 && processed_len == 4 && memcmp(processed_bytes, "\1\2\3\4",
        4) == 0);
    CHECK(live_count == base);
    for (i = 0; i < 2; i++) {
        CHECK(input(first, sizeof(first), sizeof(first)) == 0);
        if (i)
            wifi_station_wps_timeout_internal();
        else
            CHECK(wps_stop_process(WPS_FAIL_REASON_NORMAL) == 0);
        CHECK(live_count == base && !gWpsSm->is_wps_scan);
        CHECK(esp_wifi_wps_start(0) == 0);
        CHECK(input(first, sizeof(first), sizeof(first)) == 0);
        CHECK(input(last, sizeof(last), sizeof(last)) == 0);
        CHECK(processed_len == 4 && memcmp(processed_bytes, "\1\2\3\4", 4) == 0);
        CHECK(live_count == base);
    }
    CHECK(input(first, sizeof(first), sizeof(first)) == 0);
    CHECK(esp_wifi_wps_disable() == 0);
    clean();
    CHECK(live_count == 1);
    CHECK(esp_wifi_wps_enable(&cfg) == 0);
    CHECK(esp_wifi_wps_start(0) == 0);
    base = live_count;
    for (i = 1; i <= 3; i++) {
        fail_at = calls + i;
        tx = sends;
        CHECK(wps_send_wps_mX_rsp(9) != 0);
        fail_at = 0;
        CHECK(live_count == base && sends == tx);
    }
    CHECK(wps_send_wps_mX_rsp(9) == 0);
    CHECK(live_count == base);
    assoc_fail = 1;
    CHECK(wps_send_eap_identity_rsp(9) != 0);
    assoc_fail = 0;
    CHECK(live_count == base);
    CHECK(wps_send_eap_identity_rsp(9) == 0);
    CHECK(live_count == base);
    apply_count = successes = failures = timeouts = 0;
    {
        u8 req[] = {
            1, IEEE802_1X_TYPE_EAP_PACKET, 0, 4, EAP_CODE_REQUEST, 9, 0, 4
        };
        before = failures;
        handled = processed;
        CHECK(wps_sm_rx_eapol_internal(NULL, fixture_guard(req, sizeof(req)), sizeof(req)) != 0);
        CHECK(failures == before + 1 && !gWpsSm->is_wps_scan && processed == handled);
        CHECK(wps_post(SIG_WPS_TIMER_TIMEOUT, 0) == 0);
        CHECK(wps_post(SIG_WPS_TIMER_SCAN, 0) == 0);
        CHECK(wps_post(SIG_WPS_TIMER_MSG_TIMEOUT, 0) == 0);
        CHECK(callbacks->wps_sm_rx_eapol(req, req, sizeof(req)) == 0);
        fixture_worker_pump();
        CHECK(failures == before + 1 && timeouts == 0 && successes == 0 && apply_count == 0);
    }
    for (i = 0; i < 11; i++) {
        u8 req[20] = {
            1, IEEE802_1X_TYPE_EAP_PACKET, 0, 0, EAP_CODE_REQUEST, 0, 0, 0, EAP_TYPE_EXPANDED
        };
        CHECK(esp_wifi_wps_start(0) == 0);
        req[3] = req[7] = 5 + i;
        req[5] = 10 + i;
        memcpy(req + 9, first, i);
        before = failures;
        handled = processed;
        tx = sends;
        CHECK(wps_sm_rx_eapol_internal(NULL, fixture_guard(req, 9 + i), 9 + i) != 0);
        CHECK(failures == before + 1 && !gWpsSm->is_wps_scan && processed == handled
            && sends == tx);
    }
    CHECK(esp_wifi_wps_disable() == 0);
    clean();
    CHECK(live_count == 1);
    CHECK(esp_wifi_wps_enable(&cfg) == 0);
    CHECK(esp_wifi_wps_start(0) == 0);
    {
        struct wps_scan_ie scan = {
            0
        };
        u8 ssid[] = {
            0, 4, 't', 'e', 's', 't'
        };
        u8 bssid[6] = {
            9, 8, 7, 6, 5, 4
        };
        u8 ie[32] = {
            221, 9, 0, 0x50, 0xf2, 4, 0x10, 0x41, 0, 1, 1
        };
        scan.ssid = ssid;
        scan.bssid = bssid;
        scan.wps = ie;
        status = WPS_STATUS_SCANNING;
        for (i = 0; i < 3; i++) {
            ie[1] = i == 2 ? 9 : 15;
            ie[11] = 0x10;
            ie[12] = 0x12;
            ie[13] = 0;
            ie[14] = 2;
            ie[15] = 0;
            ie[16] = i ? 0 : 4;
            memcpy(gWpsSm->dis_ap_list[0].bssid, bssid, 6);
            gWpsSm->ignore_sel_reg = false;
            base = live_count;
            before = calls;
            CHECK(callbacks->wps_parse_scan_result(&scan));
            CHECK(calls == before + 1 && live_count == base);
            fail_at = calls + 1;
            before = gWpsSm->discover_ssid_cnt;
            CHECK(!callbacks->wps_parse_scan_result(&scan));
            CHECK(gWpsSm->discover_ssid_cnt == before && live_count == base);
            fail_at = 0;
        }
        ie[1] = 9;
        ie[10] = 0;
        gWpsSm->ignore_sel_reg = false;
        CHECK(!callbacks->wps_parse_scan_result(&scan));
        gWpsSm->ignore_sel_reg = true;
        CHECK(!callbacks->wps_parse_scan_result(&scan));
        memset(gWpsSm->dis_ap_list, 0, sizeof(gWpsSm->dis_ap_list));
        CHECK(callbacks->wps_parse_scan_result(&scan));
        esp8266_hal_wps_set_target_ssid(ssid + 2, 4);
        gWpsSm->ignore_sel_reg = false;
        CHECK(callbacks->wps_parse_scan_result(&scan));
        ie[9] = 20;
        CHECK(!callbacks->wps_parse_scan_result(&scan));
        ie[9] = 1;
        fail_at = calls + 1;
        CHECK(!callbacks->wps_parse_scan_result(&scan));
        fail_at = 0;
        esp8266_hal_wps_set_target_ssid(NULL, 0);
    }
    CHECK(esp_wifi_wps_disable() == 0);
    clean();
    CHECK(live_count == 1);
    fixture_note(3, 1);
}
