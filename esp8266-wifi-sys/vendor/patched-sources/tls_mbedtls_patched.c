/**
 * Copyright 2020 Espressif Systems (Shanghai) PTE LTD
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 *
 * Modified by esp8266-hal for its vendored Rust-first Wi-Fi runtime. See
 * ../README.md for the pinned source and reproduction details.
 */

#line 17
#include "utils/includes.h"
#include "utils/common.h"

#include "tls/tls.h"
#include "crypto/sha1.h"
#include "crypto/md5.h"
#include "crypto/sha256.h"
#include "crypto/sha384.h"
#include "mbedtls/asn1.h"
#include "mbedtls/ssl_internal.h"
#include "mbedtls/ctr_drbg.h"
#include "mbedtls/entropy.h"
#include "mbedtls/debug.h"
#ifdef ESPRESSIF_USE
#include "mbedtls/esp_debug.h"
#include "mbedtls/esp_config.h"
#else
#include "mbedtls/config.h"
#endif

#define TLS_RANDOM_LEN 32
#define TLS_MASTER_SECRET_LEN 48

/* Throw a compilation error if basic requirements in mbedtls are not enabled */
#if !defined(MBEDTLS_SSL_TLS_C)
#error "TLS not enabled in mbedtls config"
#endif

#if !defined(MBEDTLS_SHA256_C)
#error "SHA256 is disabled in mbedtls config"
#endif

#if !defined(MBEDTLS_AES_C)
#error "AES support is disabled in mbedtls config"
#endif

uint32_t tls_instance_count;
struct tls_data {
	/* Data for mbedlts */
	struct wpabuf *in_data;
	const u8 *in_ext_data;
	size_t in_ext_len;
	size_t in_ext_pos;
	struct wpabuf *in_ext_owner;
	int in_buf_restore_pending;
	/* Data from mbedtls */
	struct wpabuf *out_data;
};

typedef struct tls_context {
	mbedtls_ssl_context ssl;            /*!< TLS/SSL context */
	mbedtls_entropy_context entropy;    /*!< mbedTLS entropy context structure */
	mbedtls_ctr_drbg_context ctr_drbg;  /*!< mbedTLS ctr drbg context structure */
	mbedtls_ssl_config conf;            /*!< TLS/SSL config to be shared structures */
	mbedtls_x509_crt cacert;            /*!< Container for X.509 CA certificate */
	mbedtls_x509_crt *cacert_ptr;       /*!< Pointer to the cacert being used. */
	mbedtls_x509_crt clientcert;        /*!< Container for X.509 client certificate */
	mbedtls_pk_context clientkey;       /*!< Private key of client certificate */
	/* EAP configuration borrows static application material for this lifetime.
	 * Parse it only after server verification has released its CA chain. */
	const unsigned char *client_cert_blob;
	size_t client_cert_blob_len;
	const unsigned char *private_key_blob;
	size_t private_key_blob_len;
	const char *private_key_passwd;
} tls_context_t;

struct tls_connection {
	tls_context_t *tls;
	struct tls_data tls_io_data;
	unsigned char randbytes[2 * TLS_RANDOM_LEN];
	int failed;
};

void esp8266_hal_tls_set_input_owner(struct tls_connection *conn,
					     struct wpabuf *owner)
{
	if (conn == NULL)
		return;
	wpabuf_free(conn->tls_io_data.in_ext_owner);
	conn->tls_io_data.in_ext_owner = owner;
}

static void tls_mbedtls_cleanup(tls_context_t *tls)
{
	if (!tls) {
		return;
	}
	tls->cacert_ptr = NULL;
	mbedtls_x509_crt_free(&tls->cacert);
	mbedtls_x509_crt_free(&tls->clientcert);
	mbedtls_pk_free(&tls->clientkey);
	mbedtls_entropy_free(&tls->entropy);
	mbedtls_ssl_config_free(&tls->conf);
	mbedtls_ctr_drbg_free(&tls->ctr_drbg);
	mbedtls_ssl_free(&tls->ssl);
}

static int tls_mbedtls_write(void *ctx, const unsigned char *buf, size_t len)
{
	struct tls_connection *conn = (struct tls_connection *)ctx;
	struct tls_data *data = &conn->tls_io_data;

	if (data->out_data) {
		size_t used = wpabuf_len(data->out_data);
		size_t needed = used + len;
		if (needed < used || needed > SIZE_MAX - 255)
			return MBEDTLS_ERR_SSL_ALLOC_FAILED;
		/* Small record appends must not copy the whole certificate flight
		 * repeatedly. Bound spare capacity to 255 bytes on this small heap. */
		if (len > wpabuf_tailroom(data->out_data) &&
		    wpabuf_resize(&data->out_data,
				  ((needed + 255) & ~(size_t)255) - used) < 0)
			return MBEDTLS_ERR_SSL_ALLOC_FAILED;
	} else {
		data->out_data = wpabuf_alloc(len);
	}
	if (!data->out_data) {
		return MBEDTLS_ERR_SSL_ALLOC_FAILED;
	}

	wpabuf_put_data(data->out_data, buf, len);

	return len;
}

static int tls_mbedtls_read(void *ctx, unsigned char *buf, size_t len)
{
	struct tls_connection *conn = (struct tls_connection *)ctx;
	struct tls_data *data = &conn->tls_io_data;
	struct wpabuf *local_buf;
	size_t copied = 0;

	while (copied < len) {
		size_t available;
		size_t chunk;

		if (data->in_ext_data) {
			available = data->in_ext_len - data->in_ext_pos;
			chunk = len - copied;
			if (chunk > available)
				chunk = available;
			os_memcpy(buf + copied, data->in_ext_data + data->in_ext_pos,
				  chunk);
			copied += chunk;
			data->in_ext_pos += chunk;
			if (data->in_ext_pos >= data->in_ext_len) {
				data->in_ext_data = NULL;
				data->in_ext_len = 0;
				data->in_ext_pos = 0;
				wpabuf_free(data->in_ext_owner);
				data->in_ext_owner = NULL;
			} else if (data->in_ext_owner &&
				   data->in_ext_pos > data->in_ext_len - data->in_ext_pos) {
				/* Release consumed flight bytes before certificate parsing.
				 * Keeping the smaller tail avoids retaining two full copies. */
				size_t remaining = data->in_ext_len - data->in_ext_pos;
				struct wpabuf *tail = wpabuf_alloc_copy(
					data->in_ext_data + data->in_ext_pos, remaining);
				if (tail) {
					wpabuf_free(data->in_ext_owner);
					data->in_ext_owner = tail;
					data->in_ext_data = wpabuf_head_u8(tail);
					data->in_ext_len = remaining;
					data->in_ext_pos = 0;
				}
			}
			continue;
		}

		break;
	}

	if (copied == len)
		return copied;

	if (!data->in_data) {
		if (copied > 0) {
			wpa_printf(MSG_ERROR, "don't have suffient data\n");
			return copied;
		}
		return MBEDTLS_ERR_SSL_WANT_READ;
	}

	if (len - copied > wpabuf_len(data->in_data)) {
		wpa_printf(MSG_ERROR, "don't have suffient data\n");
		len = copied + wpabuf_len(data->in_data);
	}

	if (len > copied) {
		size_t data_len = len - copied;
		const u8 *head = wpabuf_head_u8(data->in_data);

		if (data_len < wpabuf_len(data->in_data)) {
			local_buf = wpabuf_alloc_copy(head + data_len,
						      wpabuf_len(data->in_data) -
						      data_len);
			if (local_buf == NULL)
				return copied > 0 ? copied : MBEDTLS_ERR_SSL_ALLOC_FAILED;
			os_memcpy(buf + copied, head, data_len);
			wpabuf_free(data->in_data);
			data->in_data = local_buf;
		} else {
			os_memcpy(buf + copied, head, data_len);
			wpabuf_free(data->in_data);
			data->in_data = NULL;
		}
		copied += data_len;
	}

	return copied;
}

static int esp8266_hal_x509_blob_is_pem(const unsigned char *blob, size_t len)
{
	return len >= 10 && os_memcmp(blob, "-----BEGIN", 10) == 0;
}

static int esp8266_hal_x509_crt_parse_blob(mbedtls_x509_crt *crt,
					    const unsigned char *blob,
					    size_t len)
{
	unsigned char *cursor;
	const unsigned char *end;
	int ret;

	if (crt == NULL || blob == NULL || len == 0)
		return MBEDTLS_ERR_X509_BAD_INPUT_DATA;

	if (esp8266_hal_x509_blob_is_pem(blob, len)) {
		ret = mbedtls_x509_crt_parse(crt, blob, len);
		/* Positive means only part of the PEM bundle parsed successfully. */
		return ret > 0 ? MBEDTLS_ERR_X509_INVALID_FORMAT : ret;
	}

	/* The SDK parser appends one owned certificate, ignoring trailing DER.
	 * Bound each call to one complete SEQUENCE and account for every byte. */
	cursor = (unsigned char *)blob;
	end = blob + len;
	while (cursor < end) {
		const unsigned char *start = cursor;
		size_t cert_len;

		ret = mbedtls_asn1_get_tag(&cursor, end, &cert_len,
					 MBEDTLS_ASN1_CONSTRUCTED | MBEDTLS_ASN1_SEQUENCE);
		if (ret != 0)
			return MBEDTLS_ERR_X509_INVALID_FORMAT + ret;

		ret = mbedtls_x509_crt_parse_der(crt, start,
					       (size_t)(cursor - start) + cert_len);
		if (ret != 0)
			return ret;
		cursor += cert_len;
	}

	return 0;
}

static void esp8266_hal_tls_drop_verified_ca(tls_context_t *tls)
{
	if (tls == NULL || tls->cacert_ptr == NULL)
		return;
	if (tls->ssl.conf == NULL || tls->ssl.conf->endpoint != MBEDTLS_SSL_IS_CLIENT)
		return;
	if (tls->ssl.state <= MBEDTLS_SSL_SERVER_CERTIFICATE)
		return;

	mbedtls_ssl_conf_ca_chain(&tls->conf, NULL, NULL);
	mbedtls_x509_crt_free(&tls->cacert);
	tls->cacert_ptr = NULL;
}

static int set_pki_context(tls_context_t *tls)
{
	int ret;

	if (tls->client_cert_blob == NULL || tls->private_key_blob  == NULL) {
		wpa_printf(MSG_ERROR, "%s: config not correct", __func__);
		return -1;
	}

	mbedtls_x509_crt_init(&tls->clientcert);
	mbedtls_pk_init(&tls->clientkey);

	ret = esp8266_hal_x509_crt_parse_blob(&tls->clientcert,
						 tls->client_cert_blob,
						 tls->client_cert_blob_len);
	if (ret < 0) {
		wpa_printf(MSG_ERROR, "mbedtls_x509_crt_parse returned -0x%x", -ret);
		return ret;
	}

	ret = mbedtls_pk_parse_key(&tls->clientkey, tls->private_key_blob, tls->private_key_blob_len,
				   (const unsigned char *)tls->private_key_passwd,
				   tls->private_key_passwd ? os_strlen(tls->private_key_passwd) : 0);
	if (ret < 0) {
		wpa_printf(MSG_ERROR, "mbedtls_pk_parse_keyfile returned -0x%x", -ret);
		return ret;
	}

	ret = mbedtls_ssl_conf_own_cert(&tls->conf, &tls->clientcert, &tls->clientkey);
	if (ret < 0) {
		wpa_printf(MSG_ERROR, "mbedtls_ssl_conf_own_cert returned -0x%x", -ret);
		return ret;
	}

	return 0;
}

static int set_ca_cert(tls_context_t *tls, const unsigned char *cacert, size_t cacert_len)
{
	tls->cacert_ptr = &tls->cacert;
	mbedtls_x509_crt_init(tls->cacert_ptr);
	int ret = esp8266_hal_x509_crt_parse_blob(tls->cacert_ptr,
						  cacert, cacert_len);
	if (ret < 0) {
		wpa_printf(MSG_ERROR, "mbedtls_x509_crt_parse returned -0x%x", -ret);
		return ret;
	}
	mbedtls_ssl_conf_authmode(&tls->conf, MBEDTLS_SSL_VERIFY_REQUIRED);
	mbedtls_ssl_conf_ca_chain(&tls->conf, tls->cacert_ptr, NULL);

	return 0;
}

static int tls_sig_hashes_for_eap[] = {
#if defined(MBEDTLS_SHA512_C)
	MBEDTLS_MD_SHA512,
	MBEDTLS_MD_SHA384,
#endif
#if defined(MBEDTLS_SHA256_C)
	MBEDTLS_MD_SHA256,
	MBEDTLS_MD_SHA224,
#endif
#if defined(MBEDTLS_SHA1_C)
	MBEDTLS_MD_SHA1,
#endif
	MBEDTLS_MD_NONE
};

const mbedtls_x509_crt_profile eap_mbedtls_x509_crt_profile =
{
#if defined(MBEDTLS_SHA1_C)
	MBEDTLS_X509_ID_FLAG( MBEDTLS_MD_SHA1 ) |
#endif
#if defined(MBEDTLS_SHA256_C)
	MBEDTLS_X509_ID_FLAG( MBEDTLS_MD_SHA224 ) |
	MBEDTLS_X509_ID_FLAG( MBEDTLS_MD_SHA256 ) |
#endif
#if defined(MBEDTLS_SHA512_C)
	MBEDTLS_X509_ID_FLAG( MBEDTLS_MD_SHA384 ) |
	MBEDTLS_X509_ID_FLAG( MBEDTLS_MD_SHA512 ) |
#endif
	0,
	0xFFFFFFF, /* Any PK alg    */
	0xFFFFFFF, /* Any curve     */
	1024,
};

static void tls_enable_sha1_config(tls_context_t *tls)
{
	const mbedtls_x509_crt_profile *crt_profile = &eap_mbedtls_x509_crt_profile;
	mbedtls_ssl_conf_cert_profile(&tls->conf, crt_profile);
	mbedtls_ssl_conf_sig_hashes(&tls->conf, tls_sig_hashes_for_eap);
}

static int set_client_config(const struct tls_connection_params *cfg, tls_context_t *tls)
{
	int ret;
	assert(cfg != NULL);
	assert(tls != NULL);

	ret = mbedtls_ssl_config_defaults(&tls->conf,
					MBEDTLS_SSL_IS_CLIENT,
					MBEDTLS_SSL_TRANSPORT_STREAM,
					MBEDTLS_SSL_PRESET_DEFAULT);
	if (ret != 0) {
		wpa_printf(MSG_ERROR, "mbedtls_ssl_config_defaults returned -0x%x", -ret);
		return ret;
	}

	/* Enable SHA1 support since it's not enabled by default in mbedtls */
	tls_enable_sha1_config(tls);
#if defined(MBEDTLS_SSL_CBC_RECORD_SPLITTING)
	mbedtls_ssl_conf_cbc_record_splitting(&tls->conf,
					MBEDTLS_SSL_CBC_RECORD_SPLITTING_DISABLED);
#endif

	if (cfg->ca_cert_blob != NULL) {
		ret = set_ca_cert(tls, cfg->ca_cert_blob, cfg->ca_cert_blob_len);
		if (ret != 0) {
			return ret;
		}
		mbedtls_ssl_conf_ca_chain(&tls->conf, tls->cacert_ptr, NULL);
	} else {
		mbedtls_ssl_conf_authmode(&tls->conf, MBEDTLS_SSL_VERIFY_NONE);
	}

	/* The SDK blobs and password borrow the HAL's static credential bytes;
	 * the caller's temporary tls_connection_params itself is not retained. */
	if (cfg->client_cert_blob != NULL && cfg->private_key_blob != NULL) {
		tls->client_cert_blob = cfg->client_cert_blob;
		tls->client_cert_blob_len = cfg->client_cert_blob_len;
		tls->private_key_blob = cfg->private_key_blob;
		tls->private_key_blob_len = cfg->private_key_blob_len;
		tls->private_key_passwd = cfg->private_key_passwd;
	}

	return 0;
}

static int tls_create_mbedtls_handle(const struct tls_connection_params *params,
				     tls_context_t *tls)
{
	int ret;

	assert(params != NULL);
	assert(tls != NULL);

	mbedtls_ssl_init(&tls->ssl);
	mbedtls_ctr_drbg_init(&tls->ctr_drbg);
	mbedtls_ssl_config_init(&tls->conf);
	mbedtls_entropy_init(&tls->entropy);

	ret = set_client_config(params, tls);
	if (ret != 0) {
		wpa_printf(MSG_ERROR, "Failed to set client configurations");
		goto exit;
	}

	ret = mbedtls_ctr_drbg_seed(&tls->ctr_drbg, mbedtls_entropy_func,
				    &tls->entropy, NULL, 0);
	if (ret != 0) {
		wpa_printf(MSG_ERROR, "mbedtls_ctr_drbg_seed returned -0x%x", -ret);
		goto exit;
	}

	mbedtls_ssl_conf_rng(&tls->conf, mbedtls_ctr_drbg_random, &tls->ctr_drbg);

	ret = mbedtls_ssl_setup(&tls->ssl, &tls->conf);
	if (ret != 0) {
		wpa_printf(MSG_ERROR, "mbedtls_ssl_setup returned -0x%x", -ret);
		goto exit;
	}

	/* Enable debug prints in case supplicant's prints are enabled */
#if defined(DEBUG_PRINT) && defined(CONFIG_MBEDTLS_DEBUG) && defined(ESPRESSIF_USE)
	mbedtls_esp_enable_debug_log(&tls->conf, 2);
#endif
	return 0;

exit:
	tls_mbedtls_cleanup(tls);
	return ret;
}

void *tls_init()
{
	tls_instance_count++;
	return &tls_instance_count;
}

void tls_deinit(void *tls_ctx)
{
	tls_instance_count--;
}

struct tls_connection * tls_connection_init(void *tls_ctx)
{
	struct tls_connection *conn = os_zalloc(sizeof(*conn));
	if (!conn) {
		wpa_printf(MSG_ERROR, "TLS: Failed to allocate connection memory");
		return NULL;
	}
	return conn;
}


void tls_connection_deinit(void *tls_ctx, struct tls_connection *conn)
{
	if (conn == NULL)
		return;

	/* A failed read may leave an owned input tail. Output belongs to EAP. */
	wpabuf_free(conn->tls_io_data.in_data);
	wpabuf_free(conn->tls_io_data.in_ext_owner);
	tls_mbedtls_cleanup(conn->tls);
	os_free(conn->tls);
	os_free(conn);
}

int tls_get_errors(void *tls_ctx)
{
	return 0;
}

int tls_connection_established(void *tls_ctx, struct tls_connection *conn)
{
	mbedtls_ssl_context *ssl = &conn->tls->ssl;

	if (ssl->state == MBEDTLS_SSL_HANDSHAKE_OVER) {
		return 1;
	}

	return 0;
}

int tls_global_set_verify(void *tls_ctx, int check_crl)
{
	wpa_printf(MSG_INFO, "TLS: global settings are not supported");
	return -1;
}

int tls_connection_set_verify(void *tls_ctx, struct tls_connection *conn,
			      int verify_peer)
{
	wpa_printf(MSG_INFO, "TLS: tls_connection_set_verify not supported");
	return -1;
}

/* Shared pre-encryption buffer ownership, implemented in ssl_tls. */
int esp8266_hal_ssl_release_input(mbedtls_ssl_context *ssl);
int esp8266_hal_ssl_restore_input(mbedtls_ssl_context *ssl, int released);

int esp8266_hal_tls_restore_deferred_input(struct tls_connection *conn)
{
	int ret;

	if (conn == NULL || conn->tls == NULL)
		return 0;
	if (!conn->tls_io_data.in_buf_restore_pending)
		return 0;

	ret = esp8266_hal_ssl_restore_input(&conn->tls->ssl, 1);
	if (ret == 0)
		conn->tls_io_data.in_buf_restore_pending = 0;

	return ret;
}

/* All step entry points share preparation: a server flight may contain
 * several records, so CLIENT_CERTIFICATE is not reached only by the final loop. */
static int tls_handshake_step(struct tls_connection *conn)
{
	tls_context_t *tls = conn->tls;
	int ret;

	if (conn->failed)
		return MBEDTLS_ERR_SSL_INTERNAL_ERROR;

	if (tls->ssl.state == MBEDTLS_SSL_CLIENT_CERTIFICATE) {
		os_memcpy(conn->randbytes, tls->ssl.handshake->randbytes,
			  sizeof(conn->randbytes));
		if (esp8266_hal_ssl_release_input(&tls->ssl))
			conn->tls_io_data.in_buf_restore_pending = 1;
		if (tls->client_cert_blob != NULL) {
			ret = set_pki_context(tls);
			if (ret != 0) {
				mbedtls_ssl_send_alert_message(&tls->ssl,
					MBEDTLS_SSL_ALERT_LEVEL_FATAL,
					MBEDTLS_SSL_ALERT_MSG_INTERNAL_ERROR);
				return ret;
			}
		}
	}

	ret = mbedtls_ssl_handshake_step(&tls->ssl);
	if (ret >= 0)
		esp8266_hal_tls_drop_verified_ca(tls);
	return ret;
}

struct wpabuf * tls_connection_handshake(void *tls_ctx,
					 struct tls_connection *conn,
					 const struct wpabuf *in_data,
					 struct wpabuf **appl_data)
{
	tls_context_t *tls = conn->tls;
	int ret = 0;

	/* data freed by sender */
	conn->tls_io_data.out_data = NULL;
	wpabuf_free(conn->tls_io_data.in_data);
	conn->tls_io_data.in_data = NULL;
	conn->tls_io_data.in_ext_data = NULL;
	conn->tls_io_data.in_ext_len = 0;
	conn->tls_io_data.in_ext_pos = 0;
	if (wpabuf_len(in_data)) {
		conn->tls_io_data.in_ext_data = wpabuf_head_u8(in_data);
		conn->tls_io_data.in_ext_len = wpabuf_len(in_data);
		conn->tls_io_data.in_ext_pos = 0;
	}
	ret = tls_handshake_step(conn);
	if (ret < 0) {
		wpa_printf(MSG_ERROR, "%s:%d", __func__, __LINE__);
		goto end;
	}

	/* Multiple reads */
	while (conn->tls_io_data.in_data || conn->tls_io_data.in_ext_data) {
		ret = tls_handshake_step(conn);
		if (ret < 0)
			break;
	}

	/* State machine just started, get client hello */
	if (tls->ssl.state == MBEDTLS_SSL_CLIENT_HELLO) {
		ret = tls_handshake_step(conn);
	}

	if (ret < 0) {
		wpa_printf(MSG_ERROR, "%s:%d", __func__, __LINE__);
		goto end;
	}

	/* Already read sever data till hello done */
	if (tls->ssl.state == MBEDTLS_SSL_SERVER_HELLO_DONE) {
		ret = tls_handshake_step(conn);
		if (ret < 0)
			goto end;
	}

	if (tls->ssl.state == MBEDTLS_SSL_CLIENT_CERTIFICATE) {
		while (tls->ssl.state <= MBEDTLS_SSL_CLIENT_FINISHED) {
			ret = tls_handshake_step(conn);
			if (ret < 0)
				break;
		}
	}

	/* Trigger state machine till handshake is complete or error occures */
	if (tls->ssl.state == MBEDTLS_SSL_FLUSH_BUFFERS) {
		while (tls->ssl.state < MBEDTLS_SSL_HANDSHAKE_OVER) {
			ret = tls_handshake_step(conn);
			if (ret < 0) {
				break;
			}
		}
	}

	if (!conn->tls_io_data.out_data) {
		conn->tls_io_data.out_data = wpabuf_alloc(0);
	}

end:
	if (ret < 0 && ret != MBEDTLS_ERR_SSL_WANT_READ &&
	    ret != MBEDTLS_ERR_SSL_WANT_WRITE &&
	    ret != MBEDTLS_ERR_SSL_CRYPTO_IN_PROGRESS)
		conn->failed = 1;
	conn->tls_io_data.in_ext_data = NULL;
	conn->tls_io_data.in_ext_len = 0;
	conn->tls_io_data.in_ext_pos = 0;
	wpabuf_free(conn->tls_io_data.in_ext_owner);
	conn->tls_io_data.in_ext_owner = NULL;
	return conn->tls_io_data.out_data;
}

struct wpabuf * tls_connection_server_handshake(void *tls_ctx,
						struct tls_connection *conn,
						const struct wpabuf *in_data,
						struct wpabuf **appl_data)
{
	wpa_printf(MSG_ERROR, "%s: not supported %d", __func__, __LINE__);
	return NULL;
}


struct wpabuf * tls_connection_encrypt(void *tls_ctx,
				       struct tls_connection *conn,
				       const struct wpabuf *in_data)
{
	/* Reset dangling pointer */
	conn->tls_io_data.out_data = NULL;

	ssize_t ret = mbedtls_ssl_write(&conn->tls->ssl,
			(unsigned char*) wpabuf_head(in_data),  wpabuf_len(in_data));

	if (ret < wpabuf_len(in_data)) {
		wpa_printf(MSG_ERROR, "%s:%d, not able to write whole data",
			   __func__, __LINE__);
	}

	return conn->tls_io_data.out_data;
}


struct wpabuf * tls_connection_decrypt(void *tls_ctx,
		struct tls_connection *conn,
		const struct wpabuf *in_data)
{
	unsigned char buf[1200];
	struct wpabuf *out;
	int ret;
	wpabuf_free(conn->tls_io_data.in_data);
	conn->tls_io_data.in_data = wpabuf_dup(in_data);
	conn->tls_io_data.in_ext_data = NULL;
	conn->tls_io_data.in_ext_len = 0;
	conn->tls_io_data.in_ext_pos = 0;
	if (!conn->tls_io_data.in_data) {
		return NULL;
	}
	ret = mbedtls_ssl_read(&conn->tls->ssl, buf, 1200);
	if (ret < 0) {
		wpa_printf(MSG_ERROR, "%s:%d, not able to write whole data",
				__func__, __LINE__);
		return NULL;
	}

	out = wpabuf_alloc_copy(buf, (size_t)ret);

	return out;
}


int tls_connection_resumed(void *tls_ctx, struct tls_connection *conn)
{
	if (conn && conn->tls && conn->tls->ssl.handshake) {
		return conn->tls->ssl.handshake->resume;
	}

	return 0;
}

int tls_connection_set_cipher_list(void *tls_ctx, struct tls_connection *conn,
				   u8 *ciphers)
{
	/* This backend uses the configured mbedTLS defaults. */
	return -1;
}

int tls_get_version(void *tls_ctx, struct tls_connection *conn,
		    char *buf, size_t buflen)
{
	const char *name;

	if (conn == NULL) {
		return -1;
	}

	name = mbedtls_ssl_get_version(&conn->tls->ssl);
	if (name == NULL) {
		return -1;
	}

	os_strlcpy(buf, name, buflen);

	return 0;
}

int tls_get_cipher(void *tls_ctx, struct tls_connection *conn,
		   char *buf, size_t buflen)
{
	const char *name;
	if (conn == NULL) {
		return -1;
	}

	name = mbedtls_ssl_get_ciphersuite(&conn->tls->ssl);
	if (name == NULL) {
		return -1;
	}

	os_strlcpy(buf, name, buflen);

	return 0;
}


int tls_connection_enable_workaround(void *tls_ctx,
				     struct tls_connection *conn)
{
	wpa_printf(MSG_ERROR, "%s: not supported %d", __func__, __LINE__);
	return -1;
}

int tls_connection_get_failed(void *tls_ctx, struct tls_connection *conn)
{
	return conn->failed;
}

int tls_connection_get_read_alerts(void *tls_ctx, struct tls_connection *conn)
{
	wpa_printf(MSG_ERROR, "%s: not supported %d", __func__, __LINE__);
	return 0;
}

int tls_connection_get_write_alerts(void *tls_ctx, struct tls_connection *conn)
{
	wpa_printf(MSG_ERROR, "%s: not supported %d", __func__, __LINE__);
	return 0;
}

int tls_connection_set_params(void *tls_ctx, struct tls_connection *conn,
			      const struct tls_connection_params *params)
{
	int ret = 0;
	tls_context_t *tls = (tls_context_t *)os_zalloc(sizeof(tls_context_t));

	if (!tls) {
		wpa_printf(MSG_ERROR, "failed to allocate tls context");
		return -1;
	}
	if (!params) {
		wpa_printf(MSG_ERROR, "configuration is null");
		ret = -1;
		goto err;
	}

	ret = tls_create_mbedtls_handle(params, tls);
	if (ret < 0) {
		wpa_printf(MSG_ERROR, "failed to create ssl handle");
		goto err;
	}
	mbedtls_ssl_set_bio(&tls->ssl, conn, tls_mbedtls_write, tls_mbedtls_read, NULL);
	conn->tls = (tls_context_t *)tls;

	return ret;
err:
	os_free(tls);
	return ret;
}

int tls_global_set_params(void *tls_ctx,
			  const struct tls_connection_params *params)
{
	wpa_printf(MSG_INFO, "TLS: Global parameters not supported");
	return -1;
}

int tls_connection_set_session_ticket_cb(void *tls_ctx,
					 struct tls_connection *conn,
					 tls_session_ticket_cb cb,
					 void *ctx)
{
	wpa_printf(MSG_ERROR, "TLS: %s not supported", __func__);
	return -1;
}

static int tls_connection_prf(void *tls_ctx, struct tls_connection *conn,
		       const char *label, int server_random_first,
		       u8 *out, size_t out_len)
{
	int ret;
	u8 seed[2 * TLS_RANDOM_LEN];
	mbedtls_ssl_context *ssl = &conn->tls->ssl;
	mbedtls_ssl_transform *transform = ssl->transform;

	if (!ssl || !transform) {
		wpa_printf(MSG_ERROR, "TLS: %s, session ingo is null", __func__);
		return -1;
	}
	if (ssl->state != MBEDTLS_SSL_HANDSHAKE_OVER) {
		wpa_printf(MSG_ERROR, "TLS: %s, incorrect tls state=%d", __func__, ssl->state);
		return -1;
	}

	if (server_random_first) {
		os_memcpy(seed, conn->randbytes + TLS_RANDOM_LEN, TLS_RANDOM_LEN);
		os_memcpy(seed + TLS_RANDOM_LEN, conn->randbytes, TLS_RANDOM_LEN);
	} else {
		os_memcpy(seed, conn->randbytes, 2 * TLS_RANDOM_LEN);
	}

	wpa_hexdump_key(MSG_MSGDUMP, "random", seed, 2 * TLS_RANDOM_LEN);
	wpa_hexdump_key(MSG_MSGDUMP, "master", ssl->session->master, TLS_MASTER_SECRET_LEN);

	if (transform->ciphersuite_info->mac == MBEDTLS_MD_SHA384) {
		ret = tls_prf_sha384(ssl->session->master, TLS_MASTER_SECRET_LEN,
				label, seed, 2 * TLS_RANDOM_LEN, out, out_len);
	} else if (transform->ciphersuite_info->mac == MBEDTLS_MD_SHA256) {
		ret = tls_prf_sha256(ssl->session->master, TLS_MASTER_SECRET_LEN,
				label, seed, 2 * TLS_RANDOM_LEN, out, out_len);
	} else {
		ret = tls_prf_sha1_md5(ssl->session->master, TLS_MASTER_SECRET_LEN,
				label, seed, 2 * TLS_RANDOM_LEN, out, out_len);
	}

	if (ret < 0) {
		wpa_printf(MSG_ERROR, "prf failed, ret=%d\n", ret);
	}
	wpa_hexdump_key(MSG_MSGDUMP, "key", out, out_len);

	return ret;
}

int tls_connection_export_key(void *tls_ctx, struct tls_connection *conn,
			      const char *label, u8 *out, size_t out_len)
{
	return tls_connection_prf(tls_ctx, conn, label, 0, out, out_len);
}

int tls_connection_shutdown(void *tls_ctx, struct tls_connection *conn)
{
	if (conn->tls_io_data.in_data) {
		wpabuf_free(conn->tls_io_data.in_data);
	}
	conn->tls_io_data.in_data = NULL;

	/* outdata may have dangling pointer */
	conn->tls_io_data.out_data = NULL;

	conn->failed = 0;
	return mbedtls_ssl_session_reset(&conn->tls->ssl);
}

int tls_connection_get_random(void *tls_ctx, struct tls_connection *conn,
			    struct tls_random *data)
{
	mbedtls_ssl_context *ssl = &conn->tls->ssl;

	os_memset(data, 0, sizeof(*data));
	if (ssl->state == MBEDTLS_SSL_CLIENT_HELLO) {
		return -1;
	}

	data->client_random = conn->randbytes;
	data->client_random_len = TLS_RANDOM_LEN;

	if (ssl->state != MBEDTLS_SSL_SERVER_HELLO) {
		data->server_random = conn->randbytes + TLS_RANDOM_LEN;
		data->server_random_len = TLS_RANDOM_LEN;
	}

	return 0;
}
