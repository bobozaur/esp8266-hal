# Licensing

The Rust source and build glue in `esp8266-wifi-sys` are available under the
MIT or Apache-2.0 terms. The complete MIT text is in `LICENSE-MIT`; the complete
Apache-2.0 text is also shipped as `vendor/licenses/ESP8266_RTOS_SDK-LICENSE`.

The vendored native runtime remains under its upstream licenses; bundling it
does not relicense those components. Complete notices are shipped in
`vendor/licenses/`:

- ESP8266 RTOS SDK and mbedTLS: Apache-2.0
- lwIP: BSD-3-Clause
- wpa_supplicant: BSD-3-Clause
- newlib: the per-file terms collected in `newlib-COPYING`

Redistributors must preserve the applicable upstream notices alongside the
vendored archives.
