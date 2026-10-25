#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$ROOT/build/qemu-timer.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-20}"

bash "$ROOT/scripts/build.sh"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS=2 bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"

grep -Eq "\[timer\] source=PIT2 pit_count=[1-9][0-9]* lapic_elapsed=[1-9][0-9]* divide=16 effective_hz=[1-9][0-9]* initial_count=[1-9][0-9]* target_hz=100" "$LOG" || {
  echo "[FAIL] calibration PIT2/Local APIC non prouvée" >&2
  exit 1
}

grep -Fq "[timer] vector=0xe0 legacy_pic_masked=1 qualification_target=100" "$LOG" || {
  echo "[FAIL] vecteur timer ou masquage PIC non confirmé" >&2
  exit 1
}

grep -Eq "\[timer\] ticks=(1[0-9][0-9]|[2-9][0-9]{2,}) eoi_failures=0 periodic=1" "$LOG" || {
  echo "[FAIL] 100 interruptions timer consécutives avec EOI ne sont pas prouvées" >&2
  exit 1
}

grep -Fq "ELYON_TIMER_READY" "$LOG" || {
  echo "[FAIL] marqueur ELYON_TIMER_READY absent" >&2
  exit 1
}

grep -Fq "ELYON_KERNEL_READY_0_0_5" "$LOG" || {
  echo "[FAIL] régression du boot nominal après timer (qemu rc=$RC)" >&2
  exit 1
}

echo "[PASS] K003-06 timer LAPIC périodique 100 Hz, 100 ticks et EOI validés"
