#!/usr/bin/env python3
"""Real AppKit process test. Requires a macOS graphical login session."""
import platform
import re
import resource
import subprocess
import time
from pathlib import Path

if platform.system() != "Darwin":
    raise SystemExit("This test requires macOS; it must not pass as a headless simulation.")

binary = Path("target/debug/rivet").resolve()
start = time.monotonic()
result = subprocess.run([str(binary), "ui", "--smoke"], text=True,
                        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=90)
elapsed = time.monotonic() - start
usage = resource.getrusage(resource.RUSAGE_CHILDREN)
metadata = subprocess.check_output(["sysctl", "hw.model", "hw.ncpu", "hw.memsize"], text=True)
report = (f"Host: {platform.platform()} {platform.machine()}\n{metadata}"
          f"process_elapsed_seconds={elapsed:.3f}\npeak_rss_bytes={usage.ru_maxrss}\n"
          f"process_cpu_seconds={usage.ru_utime + usage.ru_stime:.3f}\n"
          f"process_average_cpu_cores={(usage.ru_utime + usage.ru_stime) / elapsed:.3f}\n"
          f"{result.stdout}")
Path("rivet-ui-smoke.log").write_text(report)
print(report)
if result.returncode != 0 or "RIVET_UI_SMOKE_PASS" not in result.stdout:
    raise SystemExit(f"Native smoke failed: exit {result.returncode}")
if not Path("rivet-ui-smoke.png").is_file():
    raise SystemExit("Native smoke did not produce a screenshot")
# CPU callback regression budgets, NOT proof of displayed frame latency/refresh rate.
for metric, budget_ns in [("session_switch_cpu", 16_000_000), ("settings_cpu", 16_000_000), ("draw_cpu", 8_000_000)]:
    match = re.search(rf"^{metric}: n=(\d+) p50=\d+ p95=\d+ p99=(\d+)$", result.stdout, re.M)
    if not match or int(match[1]) < 20:
        raise SystemExit(f"Insufficient native samples for {metric}")
    if int(match[2]) > budget_ns:
        raise SystemExit(f"Native {metric} p99 exceeded {budget_ns}ns callback budget")
