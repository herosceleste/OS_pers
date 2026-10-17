#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$ROOT/build/qemu-smoke.log"

"$ROOT/scripts/build.sh"

set +e
timeout 15s env ELYON_HEADLESS=1 "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

if grep -q "ELYON_KERNEL_READY_0_0_2" "$LOG"; then
  echo "[PASS] Elyon Kernel 0.0.2 a atteint ELYON_KERNEL_READY_0_0_2"
  exit 0
fi

echo "[FAIL] marqueur ELYON_KERNEL_READY_0_0_2 absent (qemu rc=$RC)" >&2
cat "$LOG" >&2
exit 1
