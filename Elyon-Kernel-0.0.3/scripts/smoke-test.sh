#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$ROOT/build/qemu-smoke.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-15}"

bash "$ROOT/scripts/build.sh"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

if grep -q "ELYON_KERNEL_READY_0_0_3" "$LOG"; then
  echo "[PASS] Elyon Kernel 0.0.3 a atteint ELYON_KERNEL_READY_0_0_3"
  exit 0
fi

echo "[FAIL] marqueur ELYON_KERNEL_READY_0_0_3 absent (qemu rc=$RC)" >&2
cat "$LOG" >&2
exit 1
