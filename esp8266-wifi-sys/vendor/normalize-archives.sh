#!/bin/sh
set -eu

vendor_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
archive_tool=${AR:-ar}

for name in libesp8266_sdk.a libwpa_supplicant.a libmbedtls_all.a; do
    archive="$vendor_dir/libs/$name"
    work_dir=$(mktemp -d)
    trap 'rm -rf -- "$work_dir"' EXIT HUP INT TERM

    members=$($archive_tool t "$archive")
    (
        cd "$work_dir"
        $archive_tool x "$archive"
        # D mode fixes timestamps, owners, groups, and modes in member headers.
        # The member list preserves the link-significant input order.
        # shellcheck disable=SC2086
        $archive_tool crsD normalized.a $members
    )
    mv -- "$work_dir/normalized.a" "$archive"
    rm -rf -- "$work_dir"
    trap - EXIT HUP INT TERM
done
