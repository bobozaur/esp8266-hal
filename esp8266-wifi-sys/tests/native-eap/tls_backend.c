/* Actual TLS adapter and wpabuf code; mbedTLS is the controlled dependency.
 * Enter through public connection APIs and the registered BIO callbacks. */
#include "utils/includes.h"
#include "utils/common.h"
#include "utils/wpabuf.h"
#include "tls/tls.h"
#include "crypto/sha256.h"
#include "crypto/sha384.h"
#include "mbedtls/ssl.h"
#include "mbedtls/ssl_internal.h"
#include "mbedtls/entropy.h"
#include "mbedtls/ctr_drbg.h"
#include "mbedtls/asn1.h"

extern void fixture_fail(unsigned);
extern void *fixture_guard(const void *, unsigned);
extern void esp8266_hal_tls_set_input_owner(struct tls_connection *, struct wpabuf *);
#define CHECK(c) do { if (!(c)) fixture_fail(__LINE__); } while (0)

static union { long long align; unsigned char data[8192]; } blocks[8];
static size_t sizes[8];
static unsigned live[8], live_count, calls, fail_at;
static int export_handshake;
static void *allocate(size_t size)
{
    unsigned i;
    calls++;
    if (calls == fail_at)
        return NULL;
    CHECK(size <= sizeof(blocks[0].data));
    for (i = 0; i < 8; i++) {
        if (!live[i]) {
            live[i] = 1;
            live_count++;
            sizes[i] = size;
            memset(blocks[i].data, 0xa7, sizeof(blocks[i].data));
            return blocks[i].data;
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
    for (i = 0; i < 8; i++) {
        if (p != blocks[i].data)
            continue;
        CHECK(live[i]);
        for (j = sizes[i]; j < sizeof(blocks[i].data); j++)
            CHECK(blocks[i].data[j] == 0xa7);
        live[i] = 0;
        live_count--;
        return;
    }
    fixture_fail(__LINE__); /* Includes freeing a borrowed packet or interior pointer. */
}
void *_heap_caps_malloc(size_t n, uint32_t caps, const char *f, size_t l)
{ return allocate(n); }
void *_heap_caps_zalloc(size_t n, uint32_t caps, const char *f, size_t l)
{ void *p = allocate(n); if (p) memset(p, 0, n); return p; }
void _heap_caps_free(void *p, const char *f, size_t l) { release(p); }
void *_heap_caps_realloc(void *p, size_t n, uint32_t caps, const char *f, size_t l)
{
    unsigned i;
    void *q;
    if (!p) return allocate(n);
    for (i = 0; i < 8; i++) {
        if (p != blocks[i].data) continue;
        CHECK(live[i]);
        q = allocate(n);
        if (q) { memcpy(q, p, sizes[i] < n ? sizes[i] : n); release(p); }
        return q;
    }
    fixture_fail(__LINE__);
    return NULL;
}

/* Configuration/crypto dependencies do not parse certificates in these cases. */
#define CONTEXT_STUB(name, type) \
    void name##_init(type *p) { memset(p, 0, sizeof(*p)); } \
    void name##_free(type *p) {}
CONTEXT_STUB(mbedtls_ssl, mbedtls_ssl_context)
CONTEXT_STUB(mbedtls_ssl_config, mbedtls_ssl_config)
CONTEXT_STUB(mbedtls_entropy, mbedtls_entropy_context)
CONTEXT_STUB(mbedtls_ctr_drbg, mbedtls_ctr_drbg_context)
CONTEXT_STUB(mbedtls_x509_crt, mbedtls_x509_crt)
CONTEXT_STUB(mbedtls_pk, mbedtls_pk_context)
int mbedtls_ssl_config_defaults(mbedtls_ssl_config *p, int endpoint, int transport, int preset)
{ CHECK(endpoint == MBEDTLS_SSL_IS_CLIENT); p->endpoint = endpoint; return 0; }
void mbedtls_ssl_conf_cert_profile(mbedtls_ssl_config *p, const mbedtls_x509_crt_profile *profile) {}
void mbedtls_ssl_conf_sig_hashes(mbedtls_ssl_config *p, const int *hashes) {}
void mbedtls_ssl_conf_authmode(mbedtls_ssl_config *p, int mode) {}
void mbedtls_ssl_conf_ca_chain(mbedtls_ssl_config *p, mbedtls_x509_crt *ca, mbedtls_x509_crl *crl) {}
void mbedtls_ssl_conf_cbc_record_splitting(mbedtls_ssl_config *p, char split) {}
void mbedtls_ssl_conf_rng(mbedtls_ssl_config *p, int (*rng)(void *, unsigned char *, size_t), void *ctx) {}
int mbedtls_entropy_func(void *p, unsigned char *out, size_t n) { CHECK(0); return -1; }
int mbedtls_ctr_drbg_random(void *p, unsigned char *out, size_t n) { CHECK(0); return -1; }
int mbedtls_ctr_drbg_seed(mbedtls_ctr_drbg_context *p,
    int (*entropy)(void *, unsigned char *, size_t), void *ctx, const unsigned char *custom, size_t n)
{ return 0; }
int mbedtls_x509_crt_parse(mbedtls_x509_crt *p, const unsigned char *data, size_t n)
{ CHECK(0); return -1; }
int mbedtls_x509_crt_parse_der(mbedtls_x509_crt *p, const unsigned char *data, size_t n)
{ CHECK(0); return -1; }
int mbedtls_asn1_get_tag(unsigned char **p, const unsigned char *end, size_t *n, int tag)
{ CHECK(0); return -1; }
int mbedtls_pk_parse_key(mbedtls_pk_context *p, const unsigned char *key, size_t n,
    const unsigned char *password, size_t password_len)
{ CHECK(0); return -1; }
int mbedtls_ssl_conf_own_cert(mbedtls_ssl_config *p, mbedtls_x509_crt *cert, mbedtls_pk_context *key)
{ CHECK(0); return -1; }
int mbedtls_ssl_send_alert_message(mbedtls_ssl_context *p, unsigned char level, unsigned char message)
{ CHECK(0); return -1; }
int esp8266_hal_ssl_release_input(mbedtls_ssl_context *p)
{ CHECK(export_handshake && p->state == MBEDTLS_SSL_CLIENT_CERTIFICATE); return 0; }

static mbedtls_ssl_context *connection_ssl;
int mbedtls_ssl_setup(mbedtls_ssl_context *ssl, const mbedtls_ssl_config *conf)
{
    ssl->conf = conf;
    ssl->state = MBEDTLS_SSL_SERVER_CERTIFICATE;
    connection_ssl = ssl;
    return 0;
}
int mbedtls_ssl_session_reset(mbedtls_ssl_context *ssl)
{ ssl->state = MBEDTLS_SSL_SERVER_CERTIFICATE; return 0; }
void mbedtls_ssl_set_bio(mbedtls_ssl_context *ssl, void *ctx,
    mbedtls_ssl_send_t *send, mbedtls_ssl_recv_t *recv, mbedtls_ssl_recv_timeout_t *timeout)
{ ssl->p_bio = ctx; ssl->f_send = send; ssl->f_recv = recv; }

static const unsigned char bytes[] = "borrowed phase-two bytes survive partial TLS reads";
static unsigned consume, chunk, offset;
static int result;
int mbedtls_ssl_read(mbedtls_ssl_context *ssl, unsigned char *out, size_t n)
{
    unsigned before = calls, pos = 0;
    CHECK(n >= consume);
    CHECK(ssl->f_recv(ssl->p_bio, out, 0) == 0);
    while (pos < consume) {
        unsigned next = consume - pos;
        if (next > chunk) next = chunk;
        CHECK(ssl->f_recv(ssl->p_bio, out + pos, next) == (int)next);
        pos += next;
    }
    CHECK(calls == before); /* Borrowed input must never allocate or copy a tail. */
    CHECK(memcmp(out, bytes, consume) == 0);
    return result < 0 ? result : (int)consume;
}
int mbedtls_ssl_handshake_step(mbedtls_ssl_context *ssl)
{
    unsigned char data[64];
    int n;
    if (export_handshake) {
        CHECK(ssl->state == MBEDTLS_SSL_CLIENT_CERTIFICATE);
        ssl->state = MBEDTLS_SSL_HANDSHAKE_OVER;
        ssl->handshake = NULL; /* Export must use the adapter's saved randoms. */
        return 0;
    }
    n = ssl->f_recv(ssl->p_bio, data, chunk);
    CHECK(n > 0 && offset + n <= sizeof(bytes));
    CHECK(memcmp(data, bytes + offset, n) == 0);
    offset += n;
    return result;
}

static struct tls_connection *new_connection(void)
{
    struct tls_connection_params params;
    struct tls_connection *conn = tls_connection_init(NULL);
    CHECK(conn);
    memset(&params, 0, sizeof(params));
    CHECK(tls_connection_set_params(NULL, conn, &params) == 0);
    return conn;
}
static void check_no_input(void)
{
    unsigned char byte;
    CHECK(connection_ssl->f_recv(connection_ssl->p_bio, &byte, 1) == MBEDTLS_ERR_SSL_WANT_READ);
}

static u8 export_master[48], export_randoms[64], export_output[64];
static const char *export_label;
static unsigned prf_calls, expected_prf;
static int prf_result;
static int check_prf(unsigned algorithm, const u8 *secret, size_t secret_len,
                     const char *label, const u8 *seed, size_t seed_len,
                     u8 *out, size_t out_len)
{
    CHECK(algorithm == expected_prf);
    CHECK(secret_len == sizeof(export_master));
    CHECK(memcmp(secret, export_master, sizeof(export_master)) == 0);
    CHECK(strcmp(label, export_label) == 0);
    CHECK(seed_len == sizeof(export_randoms));
    CHECK(memcmp(seed, export_randoms, sizeof(export_randoms)) == 0);
    CHECK(out == export_output && out_len == sizeof(export_output));
    prf_calls++;
    if (prf_result == 0) memset(out, algorithm, out_len);
    return prf_result;
}
int tls_prf_sha1_md5(const u8 *secret, size_t secret_len, const char *label,
                    const u8 *seed, size_t seed_len, u8 *out, size_t out_len)
{ return check_prf(1, secret, secret_len, label, seed, seed_len, out, out_len); }
int tls_prf_sha256(const u8 *secret, size_t secret_len, const char *label,
                  const u8 *seed, size_t seed_len, u8 *out, size_t out_len)
{ return check_prf(2, secret, secret_len, label, seed, seed_len, out, out_len); }
int tls_prf_sha384(const u8 *secret, size_t secret_len, const char *label,
                  const u8 *seed, size_t seed_len, u8 *out, size_t out_len)
{ return check_prf(3, secret, secret_len, label, seed, seed_len, out, out_len); }

static void exporter(void)
{
    const struct { int version; mbedtls_md_type_t mac; unsigned prf; } profiles[] = {
        { MBEDTLS_SSL_MINOR_VERSION_1, MBEDTLS_MD_SHA1, 1 },
        { MBEDTLS_SSL_MINOR_VERSION_2, MBEDTLS_MD_SHA1, 1 },
        { MBEDTLS_SSL_MINOR_VERSION_3, MBEDTLS_MD_SHA1, 2 },
        { MBEDTLS_SSL_MINOR_VERSION_3, MBEDTLS_MD_SHA256, 2 },
        { MBEDTLS_SSL_MINOR_VERSION_3, MBEDTLS_MD_SHA384, 3 },
    };
    const char *labels[] = { "client EAP encryption", "ttls keying material" };
    mbedtls_ssl_handshake_params handshake;
    mbedtls_ssl_session session;
    mbedtls_ssl_transform transform;
    mbedtls_ssl_ciphersuite_t suite;
    struct tls_connection *conn = new_connection();
    struct wpabuf empty, *out;
    unsigned i, label, error, before;
    memset(&handshake, 0, sizeof(handshake));
    memset(&session, 0, sizeof(session));
    memset(&transform, 0, sizeof(transform));
    memset(&suite, 0, sizeof(suite));
    for (i = 0; i < sizeof(export_master); i++) export_master[i] = 0x90 + i;
    for (i = 0; i < sizeof(export_randoms); i++) export_randoms[i] = i;
    memcpy(handshake.randbytes, export_randoms, sizeof(export_randoms));
    memcpy(session.master, export_master, sizeof(export_master));
    connection_ssl->session = &session;
    transform.ciphersuite_info = &suite;
    export_label = labels[0];
    CHECK(tls_connection_export_key(NULL, conn, export_label,
          export_output, sizeof(export_output)) < 0);
    connection_ssl->transform = &transform;
    CHECK(tls_connection_export_key(NULL, conn, export_label,
          export_output, sizeof(export_output)) < 0);
    CHECK(prf_calls == 0);

    connection_ssl->handshake = &handshake;
    connection_ssl->state = MBEDTLS_SSL_CLIENT_CERTIFICATE;
    export_handshake = 1;
    wpabuf_set(&empty, NULL, 0);
    out = tls_connection_handshake(NULL, conn, &empty, NULL);
    CHECK(out && wpabuf_len(out) == 0 && tls_connection_established(NULL, conn));
    wpabuf_free(out);
    export_handshake = 0;
    memset(&handshake, 0, sizeof(handshake));
    before = calls;
    for (i = 0; i < sizeof(profiles) / sizeof(profiles[0]); i++) {
        connection_ssl->minor_ver = profiles[i].version;
        suite.mac = profiles[i].mac;
        expected_prf = profiles[i].prf;
        for (label = 0; label < sizeof(labels) / sizeof(labels[0]); label++) {
            export_label = labels[label];
            for (error = 0; error < 2; error++) {
                prf_result = error ? -17 : 0;
                memset(export_output, 0xa5, sizeof(export_output));
                CHECK(tls_connection_export_key(NULL, conn, export_label,
                      export_output, sizeof(export_output)) == prf_result);
                CHECK(export_output[0] == (error ? 0xa5 : expected_prf));
            }
        }
    }
    CHECK(prf_calls == 20 && calls == before);
    tls_connection_deinit(NULL, conn);
    CHECK(live_count == 0);
}

void fixture_tests(const char *name)
{
    if (strcmp(name, "tls-exporter") == 0) {
        exporter();
        return;
    }
    struct tls_connection *conn = new_connection();
    struct wpabuf view, *out, *owned;
    unsigned baseline = live_count, before, mode;
    wpabuf_set(&view, fixture_guard(bytes, sizeof(bytes)), sizeof(bytes));
    for (mode = 0; mode < 5; mode++) {
        consume = mode == 0 || mode == 4 ? sizeof(bytes) : 7;
        chunk = mode == 0 ? sizeof(bytes) : 3;
        result = mode == 2 ? MBEDTLS_ERR_SSL_INTERNAL_ERROR :
                 mode == 3 ? MBEDTLS_ERR_SSL_WANT_READ : 0;
        before = calls;
        fail_at = mode == 4 ? calls + 1 : 0;
        out = tls_connection_decrypt(NULL, conn, &view);
        CHECK(calls == before + (result < 0 ? 0 : 1));
        CHECK((out != NULL) == (mode < 2));
        if (out) {
            CHECK(wpabuf_len(out) == consume && memcmp(wpabuf_head(out), bytes, consume) == 0);
            wpabuf_free(out);
        }
        fail_at = 0;
        CHECK(live_count == baseline && memcmp(wpabuf_head(&view), bytes, sizeof(bytes)) == 0);
        check_no_input();
    }
    /* A complete borrowed input can be read in several callback chunks. */
    consume = sizeof(bytes); chunk = 3; result = 0;
    out = tls_connection_decrypt(NULL, conn, &view);
    CHECK(out && wpabuf_len(out) == sizeof(bytes));
    wpabuf_free(out);
    check_no_input();
    tls_connection_deinit(NULL, conn);
    CHECK(live_count == 0);

    /* Handshake reassembly transfers ownership; compaction failure is optional. */
    for (mode = 0; mode < 3; mode++) {
        conn = new_connection();
        baseline = live_count;
        owned = wpabuf_alloc_copy(bytes, sizeof(bytes));
        CHECK(owned);
        esp8266_hal_tls_set_input_owner(conn, owned);
        offset = 0; chunk = sizeof(bytes) - 7;
        result = mode == 2 ? MBEDTLS_ERR_SSL_INTERNAL_ERROR : 0;
        before = calls;
        fail_at = mode == 1 ? calls + 1 : 0;
        out = tls_connection_handshake(NULL, conn, owned, NULL);
        fail_at = 0;
        CHECK(calls == before + (mode == 2 ? 1 : 2));
        CHECK(offset == (mode == 2 ? chunk : sizeof(bytes)));
        if (out) wpabuf_free(out);
        CHECK(live_count == baseline);
        check_no_input();
        tls_connection_deinit(NULL, conn);
        CHECK(live_count == 0);
    }
    conn = new_connection();
    owned = wpabuf_alloc_copy(bytes, sizeof(bytes));
    CHECK(owned);
    esp8266_hal_tls_set_input_owner(conn, owned);
    CHECK(tls_connection_shutdown(NULL, conn) == 0);
    check_no_input();
    owned = wpabuf_alloc_copy(bytes, sizeof(bytes));
    CHECK(owned);
    esp8266_hal_tls_set_input_owner(conn, owned);
    tls_connection_deinit(NULL, conn); /* Deinit before the first read owns the flight. */
    tls_connection_deinit(NULL, NULL);
    CHECK(live_count == 0);
}
