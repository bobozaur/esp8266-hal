#!/usr/bin/env python3
"""Rebuild the vendored native runtime explicitly; never run by Cargo builds.

Source order and compiler options descend from driver revision
2624d1c959f71b98ff93c4f611f987a9c3d9fc71. Canonical modified translation units
replace their SDK originals directly. No historical generator or patch is needed.
"""

import argparse
from pathlib import Path
import subprocess

SDK_COMMIT = "da7ce37be424d71874241ca9f17cf8da0e3cdb70"
GCC_VERSION = "xtensa-lx106-elf-gcc (crosstool-NG esp-2020r3-49-gd5524c1) 8.4.0"
BINDGEN_VERSION = "bindgen 0.70.1"

SDK_INCLUDES = (
    'esp8266/include',
    'esp8266/include/driver',
    'esp8266/source',
    'esp_common/include',
    'esp_event/include',
    'esp_event/private_include',
    'tcpip_adapter/include',
    'lwip/include/apps',
    'lwip/include/apps/sntp',
    'lwip/lwip/src/include',
    'lwip/port/esp8266/include',
    'lwip/port/esp8266/include/arch',
    'freertos/include',
    'freertos/include/freertos',
    'freertos/include/freertos/private',
    'freertos/port/esp8266/include',
    'freertos/port/esp8266/include/freertos',
    'heap/include',
    'heap/port/esp8266/include',
    'newlib/platform_include',
    'vfs/include',
    'wpa_supplicant/include',
    'wpa_supplicant/include/esp_supplicant',
    'wpa_supplicant/port/include',
    'wpa_supplicant/src',
    'wpa_supplicant/src/esp_supplicant',
    'log/include',
    'spi_flash/include',
    'bootloader_support/include',
    'bootloader_support/include_priv',
    'app_update/include',
    'nvs_flash/include',
    'esp_ringbuf/include',
    'esp_ringbuf/include/freertos',
    'mbedtls/port/include',
    'mbedtls/port/include/esp8266',
    'mbedtls/mbedtls/include',
)

WPA_DEFINES = (
    'ESP_PLATFORM',
    '__XTENSA__',
    '__xtensa__',
    '__ets__',
    'ESP_SUPPLICANT',
    'IEEE8021X_EAPOL',
    'EAP_PEER_METHOD',
    'EAP_MSCHAPv2',
    'EAP_TTLS',
    'EAP_TLS',
    'EAP_PEAP',
    'USE_WPA2_TASK',
    'CONFIG_WPS2',
    'CONFIG_WPS_PIN',
    'USE_WPS_TASK',
    'ESPRESSIF_USE',
    'ESP8266_WORKAROUND',
    'CONFIG_ECC',
    'CONFIG_IEEE80211W',
    'CONFIG_WPA3_SAE',
    'CONFIG_DPP',
    'CONFIG_WNM',
    'CONFIG_SSL_USING_MBEDTLS',
    '__ESP_FILE__=__FILE__',
    'MBEDTLS_CONFIG_FILE="mbedtls/esp_config.h"',
)

# The removed AP bridge has no native lwIP owner. Its 29 lwIP objects and
# the unused mbedTLS socket adapter are deliberately absent.

SDK_SOURCES = (
    'components/esp_event/default_event_loop.c',
    'components/esp_event/esp_event.c',
    'components/esp_event/esp_event_private.c',
    'components/esp_event/event_loop_legacy.c',
    'components/esp_event/event_send.c',
    'components/newlib/src/esp_malloc.c',
    'components/newlib/src/locks.c',
    'components/newlib/src/random.c',
    'components/newlib/src/reent_init.c',
    'components/newlib/src/select.c',
    'components/newlib/src/syscall.c',
    'components/newlib/src/termios.c',
    'components/newlib/src/time.c',
    'components/esp_common/src/esp_err_to_name.c',
    'components/esp_common/src/stack_check.c',
    'components/vfs/vfs.c',
    'components/vfs/vfs_uart.c',
    'components/vfs/vfs_semihost.c',
    'components/esp_ringbuf/ringbuf.c',
    'components/spi_flash/src/partition.c',
    'components/spi_flash/src/spi_flash.c',
    'components/bootloader_support/src/bootloader_common.c',
    'components/bootloader_support/src/bootloader_flash.c',
    'components/bootloader_support/src/bootloader_random.c',
    'components/bootloader_support/src/bootloader_sha.c',
    'components/bootloader_support/src/bootloader_utility.c',
    'components/bootloader_support/src/efuse.c',
    'components/bootloader_support/src/esp_image_format.c',
    'components/bootloader_support/src/flash_encrypt.c',
    'components/bootloader_support/src/flash_partitions.c',
    'components/bootloader_support/src/flash_qio_mode.c',
    'components/bootloader_support/src/secure_boot.c',
    'components/bootloader_support/src/secure_boot_signatures.c',
    'components/app_update/esp_ota_ops.c',
    'components/app_update/esp_app_desc.c',
    'components/esp8266/source/chip_boot.c',
    'components/esp8266/source/backtrace.c',
    'components/esp8266/source/esp_sleep.c',
    'components/esp8266/source/esp_timer.c',
    'components/esp8266/source/ets_printf.c',
    'components/esp8266/source/crc.c',
    'components/esp8266/source/reset_reason.c',
    'components/esp8266/source/startup.c',
    'components/esp8266/source/task_wdt.c',
    'components/esp8266/source/rom.c',
    'components/esp8266/source/hw_random.c',
    'components/esp8266/source/esp_fast_boot.c',
    'components/esp8266/driver/gpio.c',
    'components/esp8266/driver/uart.c',
    'components/esp8266/source/smartconfig.c',
)

WPA_SOURCES = (
    'components/wpa_supplicant/port/os_xtensa.c',
    'components/wpa_supplicant/src/ap/ap_config.c',
    'components/wpa_supplicant/src/ap/ieee802_1x.c',
    'components/wpa_supplicant/src/ap/wpa_auth.c',
    'components/wpa_supplicant/src/ap/wpa_auth_ie.c',
    'components/wpa_supplicant/src/common/sae.c',
    'components/wpa_supplicant/src/common/wpa_common.c',
    'components/wpa_supplicant/src/common/dpp.c',
    'components/wpa_supplicant/src/utils/bitfield.c',
    'components/wpa_supplicant/src/crypto/aes-ctr.c',
    'components/wpa_supplicant/src/crypto/aes-siv.c',
    'components/wpa_supplicant/src/crypto/sha256-kdf.c',
    'components/wpa_supplicant/src/crypto/aes-cbc.c',
    'components/wpa_supplicant/src/crypto/aes-ccm.c',
    'components/wpa_supplicant/src/crypto/aes-internal-dec.c',
    'components/wpa_supplicant/src/crypto/aes-internal-enc.c',
    'components/wpa_supplicant/src/crypto/aes-internal.c',
    'components/wpa_supplicant/src/crypto/aes-omac1.c',
    'components/wpa_supplicant/src/crypto/aes-unwrap.c',
    'components/wpa_supplicant/src/crypto/aes-wrap.c',
    'components/wpa_supplicant/src/crypto/bignum.c',
    'components/wpa_supplicant/src/crypto/ccmp.c',
    'components/wpa_supplicant/src/crypto/crypto_mbedtls.c',
    'components/wpa_supplicant/src/crypto/crypto_mbedtls-bignum.c',
    'components/wpa_supplicant/src/crypto/crypto_mbedtls-ec.c',
    'components/wpa_supplicant/src/crypto/crypto_ops.c',
    'components/wpa_supplicant/src/crypto/crypto_internal-cipher.c',
    'components/wpa_supplicant/src/crypto/crypto_internal-modexp.c',
    'components/wpa_supplicant/src/crypto/crypto_internal-rsa.c',
    'components/wpa_supplicant/src/crypto/crypto_mbedtls-rsa.c',
    'components/wpa_supplicant/src/crypto/crypto_internal.c',
    'components/wpa_supplicant/src/crypto/des-internal.c',
    'components/wpa_supplicant/src/crypto/dh_group5.c',
    'components/wpa_supplicant/src/crypto/dh_groups.c',
    'components/wpa_supplicant/src/crypto/md4-internal.c',
    'components/wpa_supplicant/src/crypto/md5-internal.c',
    'components/wpa_supplicant/src/crypto/md5.c',
    'components/wpa_supplicant/src/crypto/ms_funcs.c',
    'components/wpa_supplicant/src/crypto/rc4.c',
    'components/wpa_supplicant/src/crypto/sha1-internal.c',
    'components/wpa_supplicant/src/crypto/sha1-pbkdf2.c',
    'components/wpa_supplicant/src/crypto/sha1.c',
    'components/wpa_supplicant/src/crypto/sha256-internal.c',
    'components/wpa_supplicant/src/crypto/sha256.c',
    'components/wpa_supplicant/src/crypto/sha1-tlsprf.c',
    'components/wpa_supplicant/src/crypto/sha256-tlsprf.c',
    'components/wpa_supplicant/src/crypto/sha384-tlsprf.c',
    'components/wpa_supplicant/src/crypto/sha256-prf.c',
    'components/wpa_supplicant/src/crypto/tls_mbedtls.c',
    'components/wpa_supplicant/src/eap_peer/chap.c',
    'components/wpa_supplicant/src/eap_peer/eap.c',
    'components/wpa_supplicant/src/eap_peer/eap_common.c',
    'components/wpa_supplicant/src/eap_peer/eap_mschapv2.c',
    'components/wpa_supplicant/src/eap_peer/eap_peap.c',
    'components/wpa_supplicant/src/eap_peer/eap_peap_common.c',
    'components/wpa_supplicant/src/eap_peer/eap_tls.c',
    'components/wpa_supplicant/src/eap_peer/eap_tls_common.c',
    'components/wpa_supplicant/src/eap_peer/eap_ttls.c',
    'components/wpa_supplicant/src/eap_peer/mschapv2.c',
    'components/wpa_supplicant/src/esp_supplicant/esp_hostap.c',
    'components/wpa_supplicant/src/esp_supplicant/esp_wpa2.c',
    'components/wpa_supplicant/src/esp_supplicant/esp_wpa_main.c',
    'components/wpa_supplicant/src/esp_supplicant/esp_wpas_glue.c',
    'components/wpa_supplicant/src/esp_supplicant/esp_wps.c',
    'components/wpa_supplicant/src/esp_supplicant/esp_wpa3.c',
    'components/wpa_supplicant/src/rsn_supp/pmksa_cache.c',
    'components/wpa_supplicant/src/rsn_supp/wpa.c',
    'components/wpa_supplicant/src/rsn_supp/wpa_ie.c',
    'components/wpa_supplicant/src/utils/base64.c',
    'components/wpa_supplicant/src/utils/common.c',
    'components/wpa_supplicant/src/utils/ext_password.c',
    'components/wpa_supplicant/src/utils/uuid.c',
    'components/wpa_supplicant/src/utils/wpabuf.c',
    'components/wpa_supplicant/src/utils/wpa_debug.c',
    'components/wpa_supplicant/src/utils/json.c',
    'components/wpa_supplicant/src/wps/wps.c',
    'components/wpa_supplicant/src/wps/wps_attr_build.c',
    'components/wpa_supplicant/src/wps/wps_attr_parse.c',
    'components/wpa_supplicant/src/wps/wps_attr_process.c',
    'components/wpa_supplicant/src/wps/wps_common.c',
    'components/wpa_supplicant/src/wps/wps_dev_attr.c',
    'components/wpa_supplicant/src/wps/wps_enrollee.c',
    'components/wpa_supplicant/src/wps/wps_registrar.c',
    'components/wpa_supplicant/src/wps/wps_validate.c',
)

MBEDTLS_SOURCES = (
    'components/mbedtls/mbedtls/library/aes.c',
    'components/mbedtls/mbedtls/library/aesni.c',
    'components/mbedtls/mbedtls/library/arc4.c',
    'components/mbedtls/mbedtls/library/asn1parse.c',
    'components/mbedtls/mbedtls/library/asn1write.c',
    'components/mbedtls/mbedtls/library/base64.c',
    'components/mbedtls/mbedtls/library/bignum.c',
    'components/mbedtls/mbedtls/library/blowfish.c',
    'components/mbedtls/mbedtls/library/camellia.c',
    'components/mbedtls/mbedtls/library/ccm.c',
    'components/mbedtls/mbedtls/library/certs.c',
    'components/mbedtls/mbedtls/library/cipher.c',
    'components/mbedtls/mbedtls/library/cipher_wrap.c',
    'components/mbedtls/mbedtls/library/cmac.c',
    'components/mbedtls/mbedtls/library/ctr_drbg.c',
    'components/mbedtls/mbedtls/library/debug.c',
    'components/mbedtls/mbedtls/library/des.c',
    'components/mbedtls/mbedtls/library/dhm.c',
    'components/mbedtls/mbedtls/library/ecdh.c',
    'components/mbedtls/mbedtls/library/ecdsa.c',
    'components/mbedtls/mbedtls/library/ecjpake.c',
    'components/mbedtls/mbedtls/library/ecp.c',
    'components/mbedtls/mbedtls/library/ecp_curves.c',
    'components/mbedtls/mbedtls/library/entropy.c',
    'components/mbedtls/mbedtls/library/entropy_poll.c',
    'components/mbedtls/mbedtls/library/error.c',
    'components/mbedtls/mbedtls/library/gcm.c',
    'components/mbedtls/mbedtls/library/havege.c',
    'components/mbedtls/mbedtls/library/hmac_drbg.c',
    'components/mbedtls/mbedtls/library/md.c',
    'components/mbedtls/mbedtls/library/md2.c',
    'components/mbedtls/mbedtls/library/md4.c',
    'components/mbedtls/mbedtls/library/md5.c',
    'components/mbedtls/mbedtls/library/md_wrap.c',
    'components/mbedtls/mbedtls/library/memory_buffer_alloc.c',
    'components/mbedtls/mbedtls/library/oid.c',
    'components/mbedtls/mbedtls/library/padlock.c',
    'components/mbedtls/mbedtls/library/pem.c',
    'components/mbedtls/mbedtls/library/pk.c',
    'components/mbedtls/mbedtls/library/pk_wrap.c',
    'components/mbedtls/mbedtls/library/pkcs5.c',
    'components/mbedtls/mbedtls/library/pkcs11.c',
    'components/mbedtls/mbedtls/library/pkcs12.c',
    'components/mbedtls/mbedtls/library/pkparse.c',
    'components/mbedtls/mbedtls/library/pkwrite.c',
    'components/mbedtls/mbedtls/library/platform.c',
    'components/mbedtls/mbedtls/library/platform_util.c',
    'components/mbedtls/mbedtls/library/ripemd160.c',
    'components/mbedtls/mbedtls/library/rsa.c',
    'components/mbedtls/mbedtls/library/rsa_internal.c',
    'components/mbedtls/mbedtls/library/sha1.c',
    'components/mbedtls/mbedtls/library/sha256.c',
    'components/mbedtls/mbedtls/library/sha512.c',
    'components/mbedtls/mbedtls/library/ssl_cache.c',
    'components/mbedtls/mbedtls/library/ssl_ciphersuites.c',
    'components/mbedtls/mbedtls/library/ssl_cli.c',
    'components/mbedtls/mbedtls/library/ssl_cookie.c',
    'components/mbedtls/mbedtls/library/ssl_srv.c',
    'components/mbedtls/mbedtls/library/ssl_ticket.c',
    'components/mbedtls/mbedtls/library/ssl_tls.c',
    'components/mbedtls/mbedtls/library/threading.c',
    'components/mbedtls/mbedtls/library/timing.c',
    'components/mbedtls/mbedtls/library/version.c',
    'components/mbedtls/mbedtls/library/version_features.c',
    'components/mbedtls/mbedtls/library/x509.c',
    'components/mbedtls/mbedtls/library/x509_create.c',
    'components/mbedtls/mbedtls/library/x509_crl.c',
    'components/mbedtls/mbedtls/library/x509_crt.c',
    'components/mbedtls/mbedtls/library/x509_csr.c',
    'components/mbedtls/mbedtls/library/x509write_crt.c',
    'components/mbedtls/mbedtls/library/x509write_csr.c',
    'components/mbedtls/mbedtls/library/xtea.c',
    'components/mbedtls/port/mbedtls_debug.c',
    'components/mbedtls/port/esp_hardware.c',
    'components/mbedtls/port/esp_mem.c',
    'components/mbedtls/port/esp_timing.c',
    'components/mbedtls/port/esp_aes.c',
    'components/mbedtls/port/esp_sha1.c',
    'components/mbedtls/port/esp_sha256.c',
    'components/mbedtls/port/esp_sha512.c',
    'components/mbedtls/port/esp8266/aes.c',
    'components/mbedtls/port/esp8266/arc4.c',
    'components/mbedtls/port/esp8266/base64.c',
    'components/mbedtls/port/esp8266/md5.c',
    'components/mbedtls/port/esp8266/sha1.c',
    'components/mbedtls/port/esp8266/sha256.c',
    'components/mbedtls/port/esp8266/sha512.c',
)

# Source identities for both reproduction and comparison with the pinned SDK.
PATCHED_SOURCES = {
    'components/wpa_supplicant/src/crypto/aes-internal.c': 'aes_internal_aligned_irom_tables.c',
    'components/wpa_supplicant/src/crypto/aes-internal-dec.c': 'aes_internal_dec.c_aligned_irom.c',
    'components/wpa_supplicant/src/crypto/aes-internal-enc.c': 'aes_internal_enc.c_aligned_irom.c',
    'components/wpa_supplicant/src/crypto/tls_mbedtls.c': 'tls_mbedtls_patched.c',
    'components/wpa_supplicant/src/eap_peer/eap_tls_common.c': 'eap_tls_common_patched.c',
    'components/wpa_supplicant/src/esp_supplicant/esp_wpa2.c': 'esp_wpa2_patched.c',
    'components/wpa_supplicant/src/esp_supplicant/esp_wps.c': 'esp_wps_patched.c',
    'components/wpa_supplicant/src/wps/wps_attr_build.c': 'wps_attr_build_patched.c',
    'components/mbedtls/mbedtls/library/ssl_cli.c': 'ssl_cli_patched.c',
    'components/mbedtls/mbedtls/library/ssl_tls.c': 'ssl_tls_patched.c',
}

BINDGEN_FUNCTIONS = (
    'esp_wifi_init',
    'esp_wifi_deinit',
    'esp_wifi_start',
    'esp_wifi_stop',
    'esp_wifi_restore',
    'esp_wifi_get_state',
    'esp_wifi_set_mode',
    'esp_wifi_get_mode',
    'esp_wifi_set_config',
    'esp_wifi_get_config',
    'esp_wifi_connect',
    'esp_wifi_disconnect',
    'esp_wifi_clear_fast_connect',
    'esp_wifi_sta_get_ap_info',
    'esp_wifi_deauth_sta',
    'esp_wifi_ap_get_sta_list',
    'esp_wifi_scan_start',
    'esp_wifi_scan_stop',
    'esp_wifi_scan_get_ap_num',
    'esp_wifi_scan_get_ap_records',
    'esp_wifi_set_channel',
    'esp_wifi_get_channel',
    'esp_wifi_set_bandwidth',
    'esp_wifi_get_bandwidth',
    'esp_wifi_set_protocol',
    'esp_wifi_get_protocol',
    'esp_wifi_set_country',
    'esp_wifi_get_country',
    'esp_wifi_set_mac',
    'esp_wifi_get_mac',
    'esp_wifi_set_ps',
    'esp_wifi_get_ps',
    'esp_wifi_set_max_tx_power',
    'esp_wifi_get_max_tx_power',
    'esp_wifi_set_max_tx_power_via_vdd33',
    'esp_wifi_get_vdd33',
    'esp_wifi_set_storage',
    'esp_wifi_set_event_mask',
    'esp_wifi_get_event_mask',
    'esp_wifi_set_promiscuous_rx_cb',
    'esp_wifi_set_promiscuous',
    'esp_wifi_get_promiscuous',
    'esp_wifi_set_promiscuous_filter',
    'esp_wifi_get_promiscuous_filter',
    'esp_wifi_set_promiscuous_ctrl_filter',
    'esp_wifi_get_promiscuous_ctrl_filter',
    'esp_wifi_set_vendor_ie',
    'esp_wifi_set_vendor_ie_cb',
    'esp_wifi_80211_tx',
    'esp_wifi_set_rssi_threshold',
    'esp_wifi_get_tsf_time',
    'esp_wifi_set_inactive_time',
    'esp_wifi_get_inactive_time',
    'esp_wifi_internal_free_rx_buffer',
    'esp_wifi_internal_tx',
    'esp_wifi_internal_reg_rxcb',
    'esp_wifi_internal_set_log_level',
    'esp_wifi_internal_set_log_mod',
    'esp_wifi_internal_get_log',
    'esp_event_loop_create_default',
    'esp_event_handler_register',
    'esp_event_handler_unregister',
    'esp_event_post',
    'esp_timer_get_time',
    'esp_smartconfig_.*',
    'esp_esptouch_set_timeout',
    'esp_wifi_wps_.*',
    'esp_wifi_sta_wpa2_ent_.*',
    'esp_now_.*',
    'esp_restart',
    'esp_get_free_heap_size',
    'esp_random',
    'esp_fill_random',
    'esp_read_mac',
    'esp_base_mac_addr_set',
    'esp_base_mac_addr_get',
)

BINDGEN_TYPES = (
    'wifi_.*',
    'esp_interface_t',
    'esp_err_t',
    'esp_event_handler_t',
    'esp_event_base_t',
    'esp_event_loop_handle_t',
    'smartconfig_.*',
    'wps_.*',
    'esp_wps_config_t',
    'wps_factory_information_t',
    'esp_now_.*',
    'vendor_ie_data_t',
    'esp_pm_config.*',
    'esp_mac_type_t',
    'esp_cpu_freq_t',
    'esp_reset_reason_t',
)

BINDGEN_VARIABLES = (
    'WIFI_EVENT',
    'SC_EVENT',
    'WIFI_PROTOCOL_.*',
    'WIFI_PROMIS_FILTER_MASK_.*',
    'WIFI_PROMIS_CTRL_FILTER_MASK_.*',
    'ESP_NOW_MAX_DATA_LEN',
    'ESP_NOW_ETH_ALEN',
    'ESP_NOW_KEY_LEN',
)

BINDGEN_BLOCKED_TYPES = (
    'QueueHandle_t',
    'SemaphoreHandle_t',
    'TaskHandle_t',
    'xQueueHandle',
    'FILE',
    '_reent',
    '__sFILE.*',
    '__FILE',
    'wifi_pkt_rx_ctrl_t',
    'wifi_promiscuous_pkt_t',
)


def include_flags(sdk, canonical, sysroot):
    paths = [canonical, *(sdk / "components" / name for name in SDK_INCLUDES),
             sysroot / "include"]
    return [flag for path in paths for flag in ("-I", str(path))]


def rebuild(sdk, toolchain, output):
    canonical = Path(__file__).resolve().parent / "patched-sources"
    gcc = toolchain / "bin/xtensa-lx106-elf-gcc"
    ar = toolchain / "bin/xtensa-lx106-elf-ar"
    flags = ["-Os", "-ffunction-sections", "-fdata-sections", "-mlongcalls", "-w"]
    flags += include_flags(sdk, canonical, toolchain / "xtensa-lx106-elf")
    for path, replacement in ((sdk, "esp8266-rtos-sdk"),
                              (canonical, "esp8266-wifi-build")):
        flags += [f"-ffile-prefix-map={path}={replacement}",
                  f"-fmacro-prefix-map={path}={replacement}"]
    common = ("ESP_PLATFORM", "__XTENSA__", "__xtensa__", "__ESP_FILE__=__FILE__",
              'MBEDTLS_CONFIG_FILE="mbedtls/esp_config.h"', "CONFIG_SSL_USING_MBEDTLS",
              "USING_IBUS_FASTER_GET")
    sdk_defines = (*common, 'IDF_VER="v3.4"', 'PROJECT_VER="0.1.0"',
                   'PROJECT_NAME="esp8266-wifi"')
    for name, sources, defines in (("esp8266_sdk", SDK_SOURCES, sdk_defines),
                                   ("wpa_supplicant", WPA_SOURCES, WPA_DEFINES),
                                   ("mbedtls_all", MBEDTLS_SOURCES, common)):
        directory = output / name
        directory.mkdir(parents=True, exist_ok=True)
        objects = []
        group_flags = [*flags, *("-D" + define for define in defines)]
        if name == "wpa_supplicant":
            group_flags += ["-I", str(sdk / "components/wpa_supplicant/src/crypto")]
        for index, original in enumerate(sources):
            source = canonical / PATCHED_SOURCES[original] if original in PATCHED_SOURCES else sdk / original
            # Stable names depend on this ordered manifest, never checkout paths.
            obj = directory / f"{index:03}-{Path(original).stem}.o"
            subprocess.run([str(gcc), *group_flags, "-c", str(source), "-o", str(obj)], check=True)
            objects.append(str(obj))
        archive = output / f"lib{name}.a"
        # ar replaces existing members but does not remove obsolete ones.
        archive.unlink(missing_ok=True)
        subprocess.run([str(ar), "crsD", str(archive), *objects], check=True)
        print(f"{archive.name}: {len(objects)} objects", flush=True)
    probe = canonical.parents[1] / "tests/abi_layout.c"
    subprocess.run([str(gcc), *flags, *("-D" + define for define in common),
                    "-c", str(probe), "-o", str(output / "abi_layout.o")], check=True)


def generate_bindings(sdk, toolchain, output, bindgen):
    version = subprocess.check_output([bindgen, "--version"], text=True).strip()
    if version != BINDGEN_VERSION:
        raise SystemExit(f"bindings require {BINDGEN_VERSION}; found {version}")
    canonical = Path(__file__).resolve().parent / "patched-sources"
    command = [bindgen, str(canonical / "wrapper.h"), "--output", str(output / "bindings.rs"),
               "--use-core", "--ctypes-prefix", "core::ffi", "--no-layout-tests",
               "--default-enum-style", "consts", "--merge-extern-blocks",
               "--no-doc-comments", "--no-prepend-enum-name"]
    for option, values in (("--allowlist-function", BINDGEN_FUNCTIONS),
                           ("--allowlist-type", BINDGEN_TYPES),
                           ("--allowlist-var", BINDGEN_VARIABLES),
                           ("--blocklist-type", BINDGEN_BLOCKED_TYPES)):
        for value in values:
            command += [option, value]
    command += ["--", "--target=xtensa-esp-elf", "-ffreestanding", "-DESP_PLATFORM",
                "-D__XTENSA__", "-D__xtensa__"]
    command += include_flags(sdk, canonical, toolchain / "xtensa-lx106-elf")
    subprocess.run(command, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", type=Path, required=True, help="Unmodified pinned SDK checkout")
    parser.add_argument("--toolchain", type=Path, required=True,
                        help="Pinned toolchain directory containing bin/ and xtensa-lx106-elf/include/")
    parser.add_argument("--output", type=Path, required=True, help="Disposable output directory")
    parser.add_argument("--bindings", metavar="BINDGEN",
                        help="Also regenerate bindings with this bindgen 0.70.1 executable; requires Xtensa libclang")
    args = parser.parse_args()
    sdk, toolchain, output = args.sdk.resolve(), args.toolchain.resolve(), args.output.resolve()
    vendor = Path(__file__).resolve().parent
    if output == vendor or vendor in output.parents or output == sdk or sdk in output.parents:
        parser.error("--output must be outside the vendor and SDK source trees")
    commit = subprocess.check_output(["git", "-C", str(sdk), "rev-parse", "HEAD"], text=True).strip()
    if commit != SDK_COMMIT:
        parser.error(f"SDK must be at {SDK_COMMIT}; found {commit}")
    if subprocess.check_output(["git", "-C", str(sdk), "status", "--porcelain", "--untracked-files=all",
                                "--", "components"], text=True):
        parser.error("SDK components have local changes")
    gcc = toolchain / "bin/xtensa-lx106-elf-gcc"
    version = subprocess.check_output([str(gcc), "--version"], text=True).splitlines()[0]
    if version != GCC_VERSION:
        parser.error(f"compiler must be {GCC_VERSION}; found {version}")
    output.mkdir(parents=True, exist_ok=True)
    rebuild(sdk, toolchain, output)
    if args.bindings:
        generate_bindings(sdk, toolchain, output, args.bindings)


if __name__ == "__main__":
    main()
