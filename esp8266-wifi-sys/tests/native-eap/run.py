#!/usr/bin/env python3
"""Run optional actual-C EAP ownership regressions with the pinned SDK headers."""

import argparse
import os
import platform
import runpy
import subprocess
import tempfile
from pathlib import Path


CASES = ("short", "cached", "distinct", "cache-identifier", "cache-failure",
         "fragments", "empty-output",
         "lifecycle-init", "lifecycle-queue", "lifecycle-rx", "lifecycle-suspended",
         "lifecycle-active", "lifecycle-credentials", "tls-backend", "tls-exporter")
TLS_CASES = ("tls-backend", "tls-exporter")


def run_tests(args, output, shared):
    here = Path(__file__).resolve().parent
    canonical = here.parents[1] / "vendor/patched-sources"
    includes = [canonical]
    includes.extend(args.sdk / "components" / name for name in shared["SDK_INCLUDES"])
    includes.extend((args.sysroot / "include", args.sdk / "components/wpa_supplicant/src/crypto"))
    # eap.h defines tentative globals; match pinned GCC 8's common-symbol default.
    flags = ["-Os", "-ffunction-sections", "-fdata-sections", "-fno-pie", "-fcommon", "-Wall"]
    for path in includes:
        flags.extend(("-I", str(path)))
    flags.extend("-D" + define for define in shared["DEFINES"])
    flags.append("-DSSIZE_MAX=2147483647")
    selected = args.case or CASES
    host = output / "host.o"
    subprocess.run([args.cc, "-O1", "-fno-pie", "-Wall", "-c", str(here / "host.c"),
                    "-o", str(host)], check=True)
    groups = {}
    if any(case not in TLS_CASES for case in selected):
        groups["native-eap"] = [
            args.request_source or canonical / "esp_wpa2_patched.c",
            args.tls_source or canonical / "eap_tls_common_patched.c",
            args.sdk / "components/wpa_supplicant/src/utils/wpabuf.c",
            args.sdk / "components/wpa_supplicant/src/eap_peer/eap.c",
            args.sdk / "components/wpa_supplicant/src/eap_peer/eap_common.c",
            here / "fixture.c",
        ]
    if any(case in TLS_CASES for case in selected):
        groups["native-tls"] = [
            args.backend_source or canonical / "tls_mbedtls_patched.c",
            args.sdk / "components/wpa_supplicant/src/utils/wpabuf.c",
            here / "tls_backend.c",
        ]
    for name, sources in groups.items():
        objects = []
        for index, source in enumerate(sources):
            obj = output / f"{name}-{index}-{source.stem}.o"
            subprocess.run([args.cc, *flags, "-c", str(source), "-o", str(obj)], check=True)
            objects.append(obj)
        subprocess.run([args.cc, "-no-pie", "-Wl,--gc-sections",
                        *(str(obj) for obj in objects), str(host), "-o", str(output / name)], check=True)
    failures = []
    for case in selected:
        executable = output / ("native-tls" if case in TLS_CASES else "native-eap")
        print(f"native EAP case: {case}", flush=True)
        try:
            subprocess.run([str(executable), case], check=True, timeout=10)
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
            print(f"native EAP case {case} failed: {error}", flush=True)
            failures.append(case)
    if failures:
        raise SystemExit("native EAP cases failed: " + ", ".join(failures))


def main():
    here = Path(__file__).resolve().parent
    # Reuse the existing pinned header/feature manifest without modifying WPS tests.
    shared = runpy.run_path(str(here.parent / "native-wps/run.py"))
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("--sysroot", type=Path, required=True)
    parser.add_argument("--cc", default="gcc")
    parser.add_argument("--build-dir", type=Path)
    parser.add_argument("--request-source", type=Path,
                        help="Alternate request source for explicit negative controls")
    parser.add_argument("--tls-source", type=Path,
                        help="Alternate TLS common source for explicit negative controls")
    parser.add_argument("--backend-source", type=Path,
                        help="Alternate TLS backend source for explicit negative controls")
    parser.add_argument("--case", action="append", choices=CASES)
    args = parser.parse_args()
    if platform.system() != "Linux" or platform.machine() != "x86_64" or os.sysconf("SC_PAGESIZE") != 4096:
        parser.error("guarded inputs require Linux x86_64 with 4 KiB pages")
    args.sdk = args.sdk.resolve()
    args.sysroot = args.sysroot.resolve()
    if args.request_source:
        args.request_source = args.request_source.resolve()
    if args.tls_source:
        args.tls_source = args.tls_source.resolve()
    if args.backend_source:
        args.backend_source = args.backend_source.resolve()
    for header in ("sys/reent.h", "stdio.h"):
        if not (args.sysroot / "include" / header).is_file():
            parser.error("--sysroot must contain the pinned Xtensa newlib include/ headers")
    commit = subprocess.check_output(["git", "-C", str(args.sdk), "rev-parse", "HEAD"], text=True).strip()
    if commit != shared["SDK_COMMIT"]:
        parser.error(f"SDK must be at {shared['SDK_COMMIT']}; found {commit}")
    if subprocess.run(["git", "-C", str(args.sdk), "diff", "--quiet", "HEAD", "--", "components"]).returncode:
        parser.error("SDK components have local changes")
    if args.build_dir:
        output = args.build_dir.resolve()
        output.mkdir(parents=True, exist_ok=True)
        run_tests(args, output, shared)
    else:
        with tempfile.TemporaryDirectory(prefix="native-eap-") as directory:
            run_tests(args, Path(directory), shared)


if __name__ == "__main__":
    main()
