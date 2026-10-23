#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$ROOT/build/qemu-percpu.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-25}"

bash "$ROOT/scripts/build.sh"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS=4 bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"

grep -Eq "\[percpu\] cpus=4 capacity=64 bsp_logical=0 bsp_apic_id=[0-9]+ unique=1 owned=1 tr_loaded=1" "$LOG" || {
  echo "[FAIL] résumé per-CPU 4 vCPU invalide" >&2
  exit 1
}

mapfile -t CPU_LINES < <(grep -E '^\[percpu\] cpu=' "$LOG")
if [[ "${#CPU_LINES[@]}" -ne 4 ]]; then
  echo "[FAIL] 4 slots per-CPU attendus, obtenus: ${#CPU_LINES[@]}" >&2
  exit 1
fi

ONLINE_COUNT="$(printf '%s\n' "${CPU_LINES[@]}" | grep -c 'state=online' || true)"
PREPARED_COUNT="$(printf '%s\n' "${CPU_LINES[@]}" | grep -c 'state=prepared' || true)"
if [[ "$ONLINE_COUNT" -ne 1 || "$PREPARED_COUNT" -ne 3 ]]; then
  echo "[FAIL] états attendus: 1 online + 3 prepared, obtenus: $ONLINE_COUNT online + $PREPARED_COUNT prepared" >&2
  exit 1
fi

for field in stack gdt tss ist; do
  UNIQUE_COUNT="$(
    printf '%s\n' "${CPU_LINES[@]}" |
      sed -nE "s/.* ${field}=([^ ]+).*/\1/p" |
      sort -u |
      wc -l
  )"
  if [[ "$UNIQUE_COUNT" -ne 4 ]]; then
    echo "[FAIL] ressources ${field} non uniques: $UNIQUE_COUNT/4" >&2
    exit 1
  fi
done

grep -Fq "ELYON_PERCPU_READY" "$LOG" || {
  echo "[FAIL] marqueur ELYON_PERCPU_READY absent" >&2
  exit 1
}

grep -Eq "\[percpu\] bsp_ticks=(1[0-9][0-9]|[2-9][0-9]{2,}) timer_ticks=\1 match=1" "$LOG" || {
  echo "[FAIL] compteur de ticks BSP per-CPU non corrélé au timer" >&2
  exit 1
}

grep -Fq "ELYON_KERNEL_READY_0_0_4" "$LOG" || {
  echo "[FAIL] régression du boot nominal après per-CPU (qemu rc=$RC)" >&2
  exit 1
}

echo "[PASS] K003-07 4 slots per-CPU uniques, BSP online/TSS chargé, 3 AP préparés et ticks BSP validés"
