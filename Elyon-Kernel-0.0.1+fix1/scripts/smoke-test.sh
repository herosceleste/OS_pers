#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$ROOT/build/qemu-smoke.log"

"$ROOT/scripts/build.sh"

set +e
timeout 12s "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

if grep -q "ELYON_KERNEL_READY" "$LOG"; then
  echo "[PASS] Elyon Kernel 0.0.1 a atteint ELYON_KERNEL_READY"
  exit 0
fi

echo "[FAIL] marqueur ELYON_KERNEL_READY absent (qemu rc=$RC)" >&2
cat "$LOG" >&2
exit 1
