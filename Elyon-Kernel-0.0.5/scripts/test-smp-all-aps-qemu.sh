#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$ROOT/build/qemu-smp-all-aps.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-30}"

bash "$ROOT/scripts/build.sh"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS=4 bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"

mapfile -t PREPARED < <(
  grep -E '^\[percpu\] cpu=[1-9][0-9]* apic_id=[0-9]+ state=prepared ' "$LOG"
)

if [[ "${#PREPARED[@]}" -ne 3 ]]; then
  echo "[FAIL] 3 AP préparés attendus avant INIT/SIPI, obtenus: ${#PREPARED[@]}" >&2
  exit 1
fi

for line in "${PREPARED[@]}"; do
  cpu="$(sed -nE 's/^\[percpu\] cpu=([0-9]+).*/\1/p' <<<"$line")"
  apic="$(sed -nE 's/^\[percpu\].* apic_id=([0-9]+).*/\1/p' <<<"$line")"

  if [[ -z "$cpu" || -z "$apic" ]]; then
    echo "[FAIL] impossible d'extraire cpu/apic_id depuis: $line" >&2
    exit 1
  fi

  grep -Fq "ELYON_AP_READY cpu=$cpu apic_id=$apic" "$LOG" || {
    echo "[FAIL] AP cpu=$cpu apic_id=$apic n'a pas atteint ELYON_AP_READY" >&2
    exit 1
  }
done

READY_COUNT="$(grep -c '^ELYON_AP_READY cpu=' "$LOG" || true)"
if [[ "$READY_COUNT" -ne 3 ]]; then
  echo "[FAIL] 3 marqueurs ELYON_AP_READY attendus, obtenus: $READY_COUNT" >&2
  exit 1
fi

grep -Fq "[smp] all_ready started=3 online=4 prepared=0 expected=4 trampoline=0x8000 sipi_vector=0x8" "$LOG" || {
  echo "[FAIL] état SMP final 4/4 non prouvé" >&2
  exit 1
}

grep -Fq "ELYON_SMP_READY" "$LOG" || {
  echo "[FAIL] marqueur ELYON_SMP_READY absent" >&2
  exit 1
}

if grep -Fq "[smp] AP FAILED" "$LOG"; then
  echo "[FAIL] un AP a signalé une erreur de bootstrap" >&2
  exit 1
fi

grep -Fq "ELYON_KERNEL_READY_0_0_5" "$LOG" || {
  echo "[FAIL] BSP instable après mise online de tous les AP (qemu rc=$RC)" >&2
  exit 1
}

echo "[PASS] K003-09 tous les AP MADT ont atteint ELYON_AP_READY et online=4 prepared=0"
