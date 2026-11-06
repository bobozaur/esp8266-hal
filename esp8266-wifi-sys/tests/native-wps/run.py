#!/usr/bin/env python3
"""Run optional WPS C regressions against the explicit pinned SDK checkout."""

import argparse
import os
import platform
import runpy
import subprocess
import tempfile
from pathlib import Path

# Reproduction and host regression compilation use the same pinned headers/features.
_manifest = runpy.run_path(str(Path(__file__).resolve().parents[2] / "vendor/rebuild.py"))
SDK_COMMIT = _manifest["SDK_COMMIT"]
SDK_INCLUDES = _manifest["SDK_INCLUDES"]
DEFINES = _manifest["WPA_DEFINES"]
SDK_SOURCES = (
    "wps/wps.c", "wps/wps_dev_attr.c", "eap_peer/eap_common.c",
    "wps/wps_attr_parse.c", "utils/wpabuf.c",
)


def run_tests(args, output):
    here = Path(__file__).resolve().parent
    canonical = here.parents[1] / "vendor" / "patched-sources"
    includes = [canonical]
    includes.extend(args.sdk / "components" / name for name in SDK_INCLUDES)
    includes.extend((
        args.sysroot / "include",
        args.sdk / "components/wpa_supplicant/src/crypto",
    ))
    flags = ["-Os", "-ffunction-sections", "-fdata-sections", "-fno-pie", "-Wall"]
    for path in includes:
        flags.extend(("-I", str(path)))
    flags.extend("-D" + define for define in DEFINES)
    # SDK lwIP otherwise redeclares ssize_t when parsed with newlib host headers.
    flags.append("-DSSIZE_MAX=2147483647")
    sources = [canonical / "esp_wps_patched.c", here / "fixture.c"]
    sources.extend(
        args.sdk / "components/wpa_supplicant/src" / name for name in SDK_SOURCES
    )
    sources.append(canonical / "wps_attr_build_patched.c")
    objects = []
    for index, source in enumerate(sources):
        obj = output / f"{index}-{source.stem}.o"
        subprocess.run([args.cc, *flags, "-c", str(source), "-o", str(obj)], check=True)
        objects.append(obj)
    host = output / "host.o"
    # ucontext/mmap use the host's libc headers, not the SDK/newlib headers.
    subprocess.run(
        [args.cc, "-O1", "-fno-pie", "-Wall", "-c", str(here / "host.c"), "-o", str(host)],
        check=True,
    )
    executable = output / "native-wps"
    subprocess.run(
        [
            args.cc, "-no-pie", "-Wl,--gc-sections", "-Wl,--wrap=wps_build_public_key",
            *(str(obj) for obj in objects), str(host), "-o", str(executable),
        ],
        check=True,
    )
    subprocess.run([str(executable)], check=True, timeout=30)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--sdk", type=Path, required=True,
        help="Unmodified pinned ESP8266_RTOS_SDK checkout",
    )
    parser.add_argument(
        "--sysroot", type=Path, required=True,
        help="Pinned Xtensa GCC 8.4 newlib sysroot containing include/",
    )
    parser.add_argument("--cc", default="gcc", help="Host GCC executable (default: gcc)")
    parser.add_argument(
        "--build-dir", type=Path,
        help="Keep objects/executable here; otherwise use a temporary directory",
    )
    args = parser.parse_args()
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        parser.error("the host fixture requires Linux x86_64 (ucontext and MAP_32BIT)")
    if os.sysconf("SC_PAGESIZE") != 4096:
        parser.error("the guarded-input fixture requires 4 KiB host pages")
    args.sdk = args.sdk.resolve()
    args.sysroot = args.sysroot.resolve()
    for header in ("sys/reent.h", "stdio.h"):
        if not (args.sysroot / "include" / header).is_file():
            parser.error(f"--sysroot must contain the pinned newlib include/{header}")
    commit = subprocess.check_output(
        ["git", "-C", str(args.sdk), "rev-parse", "HEAD"], text=True,
    ).strip()
    if commit != SDK_COMMIT:
        parser.error(f"SDK must be at {SDK_COMMIT}; found {commit}")
    dirty = subprocess.run(
        ["git", "-C", str(args.sdk), "diff", "--quiet", "HEAD", "--", "components"],
    ).returncode
    if dirty:
        parser.error("SDK components have local changes; use an unmodified pinned checkout")
    if args.build_dir:
        output = args.build_dir.resolve()
        output.mkdir(parents=True, exist_ok=True)
        run_tests(args, output)
    else:
        with tempfile.TemporaryDirectory(prefix="native-wps-") as directory:
            run_tests(args, Path(directory))


if __name__ == "__main__":
    main()
