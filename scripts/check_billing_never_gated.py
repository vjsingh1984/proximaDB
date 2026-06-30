#!/usr/bin/env python3
"""Guard: per-tenant BILLING meters are NEVER behind the perf ``io-trace`` gate.

ADR-027 / ADR-030 keep the two I/O-trace classes structurally separate:

* the **perf emission** (the structured ``tracing`` event in
  ``io_trace::IoTrace::emit``) is gateable behind the ``io-trace`` cargo feature
  (default off, zero cost in normal operation), but
* the per-tenant **billing** meters (KSU/KRU/KIU/KOU/KEU) and the always-on
  billing observer must **never** be gated — turning the perf gate off must not
  silence billing (the chargeback source of truth).

If any ``consumption_metrics`` symbol — or the billing observer — were wrapped in
``cfg(feature = "io-trace")``, the default (feature-off) build would stop
metering revenue. This lightweight fence (in the spirit of
``check_oss_boundary.py`` / ``check_tenant_path_guard.py``) fails the build if
that ever happens. The perf ``emit`` gate in ``io_trace.rs`` is *expected* and is
not flagged, because it sits next to perf fields, not billing symbols.

Exit status:
* 0 - no billing symbol is behind the io-trace gate.
* 1 - a billing meter / observer is gated by ``io-trace`` (ERROR).
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# consumption_metrics.rs is billing in its entirety: ANY io-trace gate there is a
# violation. io_trace.rs legitimately gates the perf emission, so it is only a
# violation when the gate sits next to a billing symbol (the billing observer).
CONSUMPTION = ROOT / "src" / "metrics" / "consumption_metrics.rs"
IO_TRACE = ROOT / "src" / "observability" / "io_trace.rs"

CFG_IO_TRACE = re.compile(r'cfg\s*\(\s*[^)]*feature\s*=\s*"io-trace"')
BILLING_HINTS = (
    "billing_observer",
    "record_task_execution_time",
    "record_storage_bytes",
    "record_storage_snapshot",
    "record_kou_bytes",
    "record_keu_units",
    "TASK_EXECUTION_TIME_MS",
    "STORAGE_BYTES_SECONDS",
    "KOU_BYTES_TOTAL",
)


def scan() -> list[str]:
    violations: list[str] = []

    if CONSUMPTION.exists():
        for i, line in enumerate(CONSUMPTION.read_text().splitlines(), start=1):
            if CFG_IO_TRACE.search(line):
                violations.append(
                    f"{CONSUMPTION.relative_to(ROOT)}:{i}: "
                    "billing meters must never be behind the io-trace perf gate"
                )

    if IO_TRACE.exists():
        lines = IO_TRACE.read_text().splitlines()
        for i, line in enumerate(lines, start=1):
            if not CFG_IO_TRACE.search(line):
                continue
            window = "\n".join(lines[max(0, i - 4) : i + 3])
            if any(h in window for h in BILLING_HINTS):
                violations.append(
                    f"{IO_TRACE.relative_to(ROOT)}:{i}: "
                    "the billing observer must stay always-on, not behind the io-trace gate"
                )
    return violations


def main() -> int:
    violations = scan()
    if violations:
        print(
            "ERROR: billing/consumption meters must never be gated by the perf "
            "`io-trace` feature (ADR-027 / ADR-030 non-entanglement):",
            file=sys.stderr,
        )
        for v in violations:
            print("  " + v, file=sys.stderr)
        return 1
    print("OK: no billing meter or observer is behind the io-trace perf gate")
    return 0


if __name__ == "__main__":
    sys.exit(main())
