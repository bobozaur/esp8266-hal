/* Actual canonical request/reassembly code; controlled protocol dependencies. */
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "freertos/queue.h"
#include "freertos/semphr.h"
#include "esp_wifi.h"
#include "esp_wpa2.h"
#include "utils/includes.h"
#include "utils/common.h"
#include "utils/wpabuf.h"
#include "common/eapol_common.h"
#include "eap_peer/eap_i.h"
#include "eap_peer/eap.h"
#include "eap_peer/eap_config.h"
#include "eap_peer/eap_methods.h"
#include "eap_peer/eap_tls_common.h"
#include "tls/tls.h"
#include "esp_wifi_driver.h"
#include "esp_private/wifi.h"

extern void fixture_fail(unsigned);
extern void fixture_note_allocations(unsigned);
extern void *fixture_guard(const void *, unsigned);
extern int eap_sm_process_request(struct eap_sm *, struct wpabuf *);
#define CHECK(c) do { if (!(c)) fixture_fail(__LINE__); } while (0)

static union { long long align; unsigned char bytes[4096]; } blocks[64];
static size_t sizes[64];
static unsigned char live[64];
static unsigned calls, drops, live_count, fail_at, bssid_calls, decrypt_calls;
static unsigned handshake_calls;
static int tls_failed, method_active;
static struct wpabuf *method_response;

static void *allocate(size_t size)
{
    unsigned i;
    calls++;
    if (calls == fail_at)
        return NULL;
    CHECK(size <= sizeof(blocks[0].bytes));
    for (i = 0; i < 64; i++) {
        if (!live[i]) {
            live[i] = 1;
            sizes[i] = size;
            live_count++;
            memset(blocks[i].bytes, 0xa7, sizeof(blocks[i].bytes));
            return blocks[i].bytes;
        }
    }
    fixture_fail(__LINE__);
    return NULL;
}

static void release(void *p)
{
    unsigned i, j;
    if (!p)
        return;
    CHECK(!method_active);
    for (i = 0; i < 64; i++) {
        if (p != blocks[i].bytes)
            continue;
        CHECK(live[i]);
        for (j = sizes[i]; j < sizeof(blocks[i].bytes); j++)
            CHECK(blocks[i].bytes[j] == 0xa7);
        live[i] = 0;
        live_count--;
        drops++;
        return;
    }
    fixture_fail(__LINE__);
}

void *_heap_caps_malloc(size_t n, uint32_t caps, const char *f, size_t l)
{ return allocate(n); }
void *_heap_caps_zalloc(size_t n, uint32_t caps, const char *f, size_t l)
{
    void *p = allocate(n);
    if (p) memset(p, 0, n);
    return p;
}
void _heap_caps_free(void *p, const char *f, size_t l) { release(p); }
void *_heap_caps_realloc(void *p, size_t n, uint32_t caps, const char *f, size_t l)
{
    unsigned i;
    void *q;
    if (!p) return allocate(n);
    for (i = 0; i < 64; i++) {
        if (p != blocks[i].bytes) continue;
        CHECK(live[i]);
        q = allocate(n);
        if (q) { memcpy(q, p, sizes[i] < n ? sizes[i] : n); release(p); }
        return q;
    }
    fixture_fail(__LINE__);
    return NULL;
}

int esp_wifi_get_assoc_bssid_internal(uint8_t *bssid)
{ bssid_calls++; return -1; }
esp_err_t esp_wifi_set_wpa2_ent_state_internal(wpa2_ent_eap_state_t state) { return ESP_OK; }
int esp_wifi_internal_tx(wifi_interface_t interface, void *buffer, uint16_t len)
{ fixture_fail(__LINE__); return -1; }
static struct wpabuf *process(struct eap_sm *sm, void *priv,
                             struct eap_method_ret *ret, const struct wpabuf *request)
{ return method_response; }

static void short_requests(void)
{
    unsigned declared, physical;
    struct eap_sm sm;
    struct wpabuf request;
    unsigned char header[4] = { EAP_CODE_REQUEST, 1, 0, 0 };
    memset(&sm, 0, sizeof(sm));
    CHECK(eap_sm_process_request(&sm, NULL) != ESP_OK);
    for (physical = 0; physical < sizeof(header); physical++) {
        wpabuf_set(&request, fixture_guard(header, physical), physical);
        CHECK(eap_sm_process_request(&sm, &request) != ESP_OK);
        CHECK(calls == 0 && bssid_calls == 0);
    }
    for (declared = 0; declared <= sizeof(header) + 1; declared++) {
        header[3] = declared;
        wpabuf_set(&request, fixture_guard(header, sizeof(header)), sizeof(header));
        CHECK(eap_sm_process_request(&sm, &request) != ESP_OK);
        CHECK(calls == 0 && bssid_calls == 0);
    }
}

static void transmit_failure(int cached)
{
    struct eap_sm sm;
    struct eap_method method;
    struct wpabuf request;
    unsigned char data[] = { EAP_CODE_REQUEST, 7, 0, 5, EAP_TYPE_TLS };
    memset(&sm, 0, sizeof(sm));
    memset(&method, 0, sizeof(method));
    sm.lastRespData = wpabuf_alloc(1);
    CHECK(sm.lastRespData);
    if (cached) {
        sm.current_identifier = 7;
    } else {
        method.vendor = EAP_VENDOR_IETF;
        method.method = EAP_TYPE_TLS;
        method.process = process;
        sm.m = &method;
        sm.eap_method_priv = &sm;
        method_response = wpabuf_alloc(1);
        CHECK(method_response);
    }
    wpabuf_set(&request, data, sizeof(data));
    CHECK(eap_sm_process_request(&sm, &request) != ESP_OK);
    CHECK(bssid_calls == 1 && sm.lastRespData == NULL);
    CHECK(drops == (cached ? 1u : 2u) && live_count == 0);
}

struct wpabuf *tls_connection_decrypt(void *ctx, struct tls_connection *conn,
                                    const struct wpabuf *input)
{
    size_t i;
    const unsigned char *p = wpabuf_head_u8(input);
    decrypt_calls++;
    CHECK(wpabuf_len(input) == 1537);
    for (i = 0; i < wpabuf_len(input); i++) CHECK(p[i] == i % 251);
    return wpabuf_alloc_copy("x", 1);
}

struct wpabuf *tls_connection_handshake(void *ctx, struct tls_connection *conn,
                                      const struct wpabuf *input,
                                      struct wpabuf **application)
{
    handshake_calls++;
    CHECK(wpabuf_len(input) == 1 && wpabuf_head_u8(input)[0] == 0x16);
    CHECK(application && *application == NULL);
    return wpabuf_alloc(0);
}

int tls_connection_get_failed(void *ctx, struct tls_connection *conn)
{ return tls_failed; }
int tls_connection_established(void *ctx, struct tls_connection *conn)
{ fixture_fail(__LINE__); return 0; }
int esp8266_hal_tls_restore_deferred_input(struct tls_connection *conn)
{ return 0; }
void esp8266_hal_tls_set_input_owner(struct tls_connection *conn, struct wpabuf *owner)
{ fixture_fail(__LINE__); }

static void empty_output(void)
{
    struct eap_ssl_data tls;
    struct wpabuf *out;
    const unsigned char input[] = { 0x16 };
    unsigned failed;
    int result;

    for (failed = 0; failed < 2; failed++) {
        memset(&tls, 0, sizeof(tls));
        out = NULL;
        tls_failed = failed;
        result = eap_peer_tls_process_helper(NULL, &tls, EAP_TYPE_TLS, 0, 1,
                                             input, sizeof(input), &out);
        if (failed)
            CHECK(result < 0);
        else
            CHECK(result == 1);
        CHECK(out == NULL && tls.tls_out == NULL);
        CHECK(tls.tls_in == NULL && tls.tls_in_left == 0 && tls.tls_in_total == 0);
        CHECK(handshake_calls == failed + 1 && calls == failed + 1);
        CHECK(drops == calls && live_count == 0);
    }
}

static void fragments(void)
{
    unsigned char input[1537];
    const unsigned lengths[] = { 507, 511, 511, 8 };
    struct eap_ssl_data tls;
    struct wpabuf fragment, *out = NULL;
    unsigned i, offset = 0;
    memset(&tls, 0, sizeof(tls));
    for (i = 0; i < sizeof(input); i++) input[i] = i % 251;
    tls.tls_in_total = tls.tls_in_left = sizeof(input);
    for (i = 0; i < 4; i++) {
        wpabuf_set(&fragment, input + offset, lengths[i]);
        CHECK(eap_peer_tls_decrypt(NULL, &tls, &fragment, &out) == (i < 3 ? 1 : 0));
        if (i < 3) {
            CHECK(calls == 1 && tls.tls_in->size == sizeof(input));
            CHECK(decrypt_calls == 0);
        }
        offset += lengths[i];
    }
    CHECK(decrypt_calls == 1 && calls == 2 && out != NULL);
    CHECK(tls.tls_in == NULL && tls.tls_in_left == 0 && tls.tls_in_total == 0);
    wpabuf_free(out);
    out = NULL;
    CHECK(live_count == 0);
    tls.tls_in_total = tls.tls_in_left = sizeof(input);
    fail_at = calls + 1;
    wpabuf_set(&fragment, input, lengths[0]);
    CHECK(eap_peer_tls_decrypt(NULL, &tls, &fragment, &out) == -1);
    CHECK(tls.tls_in == NULL && tls.tls_in_left == 0 && tls.tls_in_total == 0);
    CHECK(calls == 3 && decrypt_calls == 1 && live_count == 0);
}

/* Registered-callback lifecycle tests; real SDK eap.c owns config and blobs. */
extern void fixture_worker_create(void (*)(void *), void *);
extern void fixture_worker_pump(void), fixture_worker_yield(void), fixture_worker_stop(void);
extern int fixture_worker_alive(void), fixture_worker_is_current(void);

static struct wpa2_funcs *callbacks;
static void *worker_handle;
static unsigned critical_depth, queue_faults, semaphore_faults, task_faults;
static BaseType_t scheduler = taskSCHEDULER_RUNNING;
static unsigned method_calls, method_drops, completion_gives;
static int pause_method, inject_teardown, send_fail, mac_failure;
static unsigned mac_calls;
static struct eap_sm *method_sm;
static unsigned tls_contexts;
static int tls_context;

static const u8 station_mac[6] = { 2, 0, 0, 0, 0, 1 };
static const u8 identity[] = "fixture-anonymous";
static const u8 username[] = "fixture-user";
static const u8 password[] = "fixture-password";
static const u8 client_cert[] = { 0x30, 0 };
static const u8 private_key[] = { 0x30, 1, 0 };
static const u8 ca_cert[] = { 0x30, 2, 0, 0 };
static const u8 key_password[] = "key-password";

static void check_credentials(struct eap_sm *sm)
{
    const struct eap_peer_config *config = eap_get_config(sm);
    const struct wpa_config_blob *blob;
    CHECK(config);
    CHECK(!memcmp(sm->ownaddr, station_mac, sizeof(station_mac)));
    CHECK(config->anonymous_identity_len == sizeof(identity) - 1);
    CHECK(!memcmp(config->anonymous_identity, identity, sizeof(identity) - 1));
    CHECK(config->identity_len == sizeof(username) - 1);
    CHECK(!memcmp(config->identity, username, sizeof(username) - 1));
    CHECK(config->password_len == sizeof(password) - 1);
    CHECK(!memcmp(config->password, password, sizeof(password) - 1));
    CHECK(config->private_key_passwd == key_password);
    blob = eap_get_config_blob(sm, (const char *)config->client_cert);
    CHECK(blob && blob->data == client_cert && blob->len == sizeof(client_cert));
    blob = eap_get_config_blob(sm, (const char *)config->private_key);
    CHECK(blob && blob->data == private_key && blob->len == sizeof(private_key));
    blob = eap_get_config_blob(sm, (const char *)config->ca_cert);
    CHECK(blob && blob->data == ca_cert && blob->len == sizeof(ca_cert));
}

static void *method_init(struct eap_sm *sm)
{
    check_credentials(sm);
    method_sm = sm;
    return sm;
}

static void method_deinit(struct eap_sm *sm, void *priv)
{
    CHECK(!method_active && sm == method_sm && priv == sm);
    method_drops++;
    method_sm = NULL;
}

static struct wpabuf *method_process(struct eap_sm *sm, void *priv,
                                    struct eap_method_ret *ret,
                                    const struct wpabuf *request)
{
    CHECK(priv == sm && sm == method_sm);
    check_credentials(sm);
    method_calls++;
    method_active = 1;
    if (pause_method)
        fixture_worker_yield();
    CHECK(method_sm == sm);
    check_credentials(sm);
    method_active = 0;
    return NULL; /* No crypto or outgoing frame needed to exercise ownership. */
}

int eap_peer_tls_register(void)
{
    struct eap_method *method = eap_peer_method_alloc(EAP_VENDOR_IETF, EAP_TYPE_TLS,
                                                      "fixture TLS");
    if (!method)
        return -1;
    method->init = method_init;
    method->deinit = method_deinit;
    method->process = method_process;
    return eap_peer_method_register(method);
}
int eap_peer_peap_register(void) { return 0; }
int eap_peer_ttls_register(void) { return 0; }
int eap_peer_mschapv2_register(void) { return 0; }

void *tls_init(void) { tls_contexts++; return &tls_context; }
void tls_deinit(void *ctx)
{
    CHECK(ctx == &tls_context && tls_contexts);
    tls_contexts--;
}

int esp_wifi_register_wpa2_cb_internal(struct wpa2_funcs *p)
{ CHECK(!callbacks); callbacks = p; return ESP_OK; }
int esp_wifi_unregister_wpa2_cb_internal(void)
{ release(callbacks); callbacks = NULL; return ESP_OK; }
esp_err_t esp_wifi_sta_wpa2_ent_enable_internal(wifi_wpa2_param_t *p)
{ return p->fn(p->param); }
esp_err_t esp_wifi_sta_wpa2_ent_disable_internal(wifi_wpa2_param_t *p)
{ return p->fn(p->param); }
esp_err_t esp_wifi_get_mac(wifi_interface_t interface, uint8_t address[6])
{
    CHECK(interface == WIFI_IF_STA);
    mac_calls++;
    if (mac_failure)
        return ESP_FAIL;
    memcpy(address, station_mac, sizeof(station_mac));
    return ESP_OK;
}
bool wpa_sta_cur_pmksa_matches_akm(void) { return false; }
int wpa_sm_rx_eapol(u8 *src, u8 *buf, u32 len)
{ fixture_fail(__LINE__); return ESP_FAIL; }
void wpa_set_pmk(uint8_t *pmk, const u8 *pmkid, bool cache)
{ fixture_fail(__LINE__); }

/* FreeRTOS dependencies copy values and pump only outside scheduler exclusion. */
struct fixture_queue {
    unsigned capacity, item_size, count, head;
    unsigned char items[8][sizeof(ETSEvent)];
};
static struct fixture_queue *event_queue, *completion_sem;

QueueHandle_t xQueueCreateMutex(uint8_t kind)
{
    struct fixture_queue *q = allocate(sizeof(*q));
    if (q)
        memset(q, 0, sizeof(*q));
    return q;
}
QueueHandle_t xQueueCreateCountingSemaphore(UBaseType_t max, UBaseType_t initial)
{
    struct fixture_queue *q = xQueueCreateMutex(0);
    if (q) {
        q->capacity = max;
        q->count = initial;
        completion_sem = q;
    } else {
        semaphore_faults++;
    }
    return q;
}
QueueHandle_t xQueueGenericCreate(UBaseType_t n, UBaseType_t size, uint8_t kind)
{
    struct fixture_queue *q = xQueueCreateMutex(0);
    if (q) {
        CHECK(n <= 8 && size == sizeof(ETSEvent));
        q->capacity = n;
        q->item_size = size;
        event_queue = q;
    } else {
        queue_faults++;
    }
    return q;
}
BaseType_t xQueueTakeMutexRecursive(QueueHandle_t q, TickType_t ticks)
{ CHECK(q); return pdPASS; }
BaseType_t xQueueGiveMutexRecursive(QueueHandle_t q)
{ CHECK(q); return pdPASS; }

BaseType_t xQueueGenericSend(QueueHandle_t handle, const void *item,
                            TickType_t ticks, BaseType_t position)
{
    struct fixture_queue *q = handle;
    CHECK(q);
    if (!q->item_size) {
        CHECK(q == completion_sem && q->count < q->capacity);
        q->count++;
        completion_gives++;
        return pdPASS;
    }
    if (send_fail) {
        send_fail = 0;
        return pdFAIL;
    }
    CHECK(((const ETSEvent *)item)->sig < SIG_WPA2_MAX);
    CHECK(q->count < q->capacity);
    memcpy(q->items[(q->head + q->count) % q->capacity], item, q->item_size);
    q->count++;
    return pdPASS;
}

static void suspended_teardown(void)
{
    unsigned allocations = calls, frees = drops, retained = live_count;
    CHECK(callbacks && worker_handle);
    scheduler = taskSCHEDULER_SUSPENDED;
    callbacks->wpa2_deinit();
    CHECK(calls == allocations && drops == frees && live_count == retained);
    CHECK(worker_handle && tls_contexts == 1);
    CHECK(callbacks->wpa2_init() != ESP_OK);
    CHECK(calls == allocations && drops == frees && live_count == retained);
    CHECK(callbacks->wpa2_start() != ESP_OK); /* Closing blocks new work. */
    CHECK(calls == allocations && drops == frees && live_count == retained);
    scheduler = taskSCHEDULER_RUNNING;
}

BaseType_t xQueueSemaphoreTake(QueueHandle_t handle, TickType_t ticks)
{
    struct fixture_queue *q = handle;
    unsigned pumps = 0;
    CHECK(q == completion_sem && !fixture_worker_is_current());
    while (!q->count) {
        CHECK(ticks && scheduler == taskSCHEDULER_RUNNING && !critical_depth);
        CHECK(++pumps < 8);
        fixture_worker_pump();
        if (inject_teardown && method_active) {
            inject_teardown = 0;
            suspended_teardown();
            CHECK(method_active && method_drops == 0);
            pause_method = 0;
        }
    }
    q->count--;
    return pdPASS;
}
BaseType_t xQueueReceive(QueueHandle_t handle, void *item, TickType_t ticks)
{
    struct fixture_queue *q = handle;
    CHECK(q == event_queue && q->item_size);
    while (!q->count) {
        if (!ticks)
            return pdFAIL;
        CHECK(fixture_worker_is_current() && !critical_depth);
        fixture_worker_yield();
    }
    memcpy(item, q->items[q->head], q->item_size);
    q->head = (q->head + 1) % q->capacity;
    q->count--;
    return pdPASS;
}
void vQueueDelete(QueueHandle_t handle)
{
    struct fixture_queue *q = handle;
    CHECK(!method_active && q->count == 0);
    if (q == event_queue) event_queue = NULL;
    if (q == completion_sem) completion_sem = NULL;
    release(q);
}
BaseType_t xTaskCreate(TaskFunction_t entry, const char *name, uint16_t depth,
                       void *arg, UBaseType_t priority, TaskHandle_t *out)
{
    CHECK(!worker_handle);
    worker_handle = allocate(16);
    if (!worker_handle) {
        task_faults++;
        return errCOULD_NOT_ALLOCATE_REQUIRED_MEMORY;
    }
    *out = worker_handle;
    fixture_worker_create(entry, arg);
    return pdPASS;
}
void vTaskDelete(TaskHandle_t handle)
{
    CHECK(!handle && fixture_worker_is_current() && !method_active);
    release(worker_handle);
    worker_handle = NULL;
    fixture_worker_stop();
}
TaskHandle_t xTaskGetCurrentTaskHandle(void)
{ return fixture_worker_is_current() ? worker_handle : (void *)1; }
BaseType_t xTaskGetSchedulerState(void) { return scheduler; }
void vTaskDelay(const TickType_t ticks)
{
    CHECK(ticks && scheduler == taskSCHEDULER_RUNNING && !critical_depth);
    CHECK(!fixture_worker_is_current() && fixture_worker_alive());
    fixture_worker_pump();
}
void vPortEnterCritical(void) { critical_depth++; }
void vPortExitCritical(void) { CHECK(critical_depth); critical_depth--; }

static unsigned credential_baseline;
static void prepare_lifecycle(void)
{
    CHECK(esp_wifi_sta_wpa2_ent_set_identity(identity, sizeof(identity) - 1) == ESP_OK);
    CHECK(esp_wifi_sta_wpa2_ent_set_username(username, sizeof(username) - 1) == ESP_OK);
    CHECK(esp_wifi_sta_wpa2_ent_set_password(password, sizeof(password) - 1) == ESP_OK);
    CHECK(esp_wifi_sta_wpa2_ent_set_ca_cert(ca_cert, sizeof(ca_cert)) == ESP_OK);
    CHECK(esp_wifi_sta_wpa2_ent_set_cert_key(client_cert, sizeof(client_cert),
          private_key, sizeof(private_key), key_password, sizeof(key_password)) == ESP_OK);
    /* Establish the deliberately boot-owned API mutex before measuring cleanup. */
    CHECK(esp_wifi_sta_wpa2_ent_enable() == ESP_OK && callbacks);
    CHECK(esp_wifi_sta_wpa2_ent_disable() == ESP_OK && !callbacks);
    credential_baseline = live_count;
}
static void enable_initialized(void)
{
    CHECK(esp_wifi_sta_wpa2_ent_enable() == ESP_OK && callbacks);
    CHECK(callbacks->wpa2_init() == ESP_OK);
    CHECK(event_queue && completion_sem && worker_handle && tls_contexts == 1);
}
static void disable_cleanly(void)
{
    unsigned allocations = calls;
    CHECK(esp_wifi_sta_wpa2_ent_disable() == ESP_OK);
    CHECK(calls == allocations);
    CHECK(!callbacks && !worker_handle && !event_queue && !completion_sem);
    CHECK(tls_contexts == 0 && !method_active && !method_sm);
    CHECK(live_count == credential_baseline);
}
static void receive_method(void)
{
    u8 address[6] = { 2, 0, 0, 0, 0, 1 };
    u8 frame[] = { 1, IEEE802_1X_TYPE_EAP_PACKET, 0, 5,
                   EAP_CODE_REQUEST, 7, 0, 5, EAP_TYPE_TLS };
    CHECK(callbacks->wpa2_sm_rx_eapol(address, frame, sizeof(frame), address) == ESP_OK);
}

static void lifecycle_init(void)
{
    unsigned first, sites, index, retained;
    prepare_lifecycle();
    CHECK(esp_wifi_sta_wpa2_ent_enable() == ESP_OK && callbacks);
    retained = live_count;
    first = mac_calls;
    mac_failure = 1;
    CHECK(callbacks->wpa2_init() == ESP_FAIL);
    CHECK(mac_calls == first + 1 && live_count == retained && tls_contexts == 0);
    CHECK(!event_queue && !completion_sem && !worker_handle);
    mac_failure = 0;
    first = calls;
    CHECK(callbacks->wpa2_init() == ESP_OK);
    sites = calls - first;
    callbacks->wpa2_deinit();
    CHECK(live_count == retained && tls_contexts == 0 && !worker_handle);
    for (index = 1; index <= sites; index++) {
        fail_at = calls + index;
        CHECK(callbacks->wpa2_init() != ESP_OK);
        CHECK(calls == fail_at);
        fail_at = 0;
        CHECK(live_count == retained && tls_contexts == 0);
        CHECK(!event_queue && !completion_sem && !worker_handle);
        CHECK(callbacks->wpa2_init() == ESP_OK);
        callbacks->wpa2_deinit();
        CHECK(live_count == retained && tls_contexts == 0 && !worker_handle);
    }
    CHECK(queue_faults && semaphore_faults && task_faults);
    fixture_note_allocations(sites);
    disable_cleanly();
}
static void lifecycle_queue(void)
{
    unsigned first, attempts, before_bssid;
    prepare_lifecycle();
    enable_initialized();
    first = calls;
    before_bssid = bssid_calls;
    send_fail = 1;
    CHECK(callbacks->wpa2_start() != ESP_OK);
    CHECK(calls == first && bssid_calls == before_bssid && event_queue->count == 0);
    for (attempts = 0; attempts < 3; attempts++)
        CHECK(callbacks->wpa2_start() == ESP_OK);
    CHECK(calls == first && bssid_calls == before_bssid + 3);
    CHECK(completion_gives == 3 && event_queue->count == 0 && completion_sem->count == 0);
    disable_cleanly();
}
static void lifecycle_suspended(void)
{
    prepare_lifecycle();
    enable_initialized();
    suspended_teardown();
    disable_cleanly();
    enable_initialized();
    disable_cleanly();
}
static void lifecycle_active(void)
{
    prepare_lifecycle();
    enable_initialized();
    pause_method = inject_teardown = 1;
    receive_method();
    CHECK(!inject_teardown && method_calls == 1 && !method_active);
    CHECK(completion_gives == 1 && completion_sem->count == 0);
    disable_cleanly();
    CHECK(method_drops == 1);
}
static void lifecycle_credentials(void)
{
    unsigned cycle;
    u8 *anonymous, *user, *pass;
    prepare_lifecycle();
    anonymous = g_wpa_anonymous_identity;
    user = g_wpa_username;
    pass = g_wpa_password;
    for (cycle = 0; cycle < 3; cycle++) {
        enable_initialized();
        receive_method(); /* Real SDK config/blob accessors check every credential. */
        disable_cleanly();
        CHECK(g_wpa_anonymous_identity == anonymous && g_wpa_username == user);
        CHECK(g_wpa_password == pass);
    }
    CHECK(method_calls == 3 && method_drops == 3);
}

void fixture_tests(const char *name)
{
    if (strcmp(name, "short") == 0) short_requests();
    else if (strcmp(name, "cached") == 0) transmit_failure(1);
    else if (strcmp(name, "distinct") == 0) transmit_failure(0);
    else if (strcmp(name, "fragments") == 0) fragments();
    else if (strcmp(name, "empty-output") == 0) empty_output();
    else if (strcmp(name, "lifecycle-init") == 0) lifecycle_init();
    else if (strcmp(name, "lifecycle-queue") == 0) lifecycle_queue();
    else if (strcmp(name, "lifecycle-suspended") == 0) lifecycle_suspended();
    else if (strcmp(name, "lifecycle-active") == 0) lifecycle_active();
    else if (strcmp(name, "lifecycle-credentials") == 0) lifecycle_credentials();
    else fixture_fail(__LINE__);
}
