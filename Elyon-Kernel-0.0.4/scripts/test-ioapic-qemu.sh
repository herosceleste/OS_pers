#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$ROOT/build/qemu-ioapic.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-20}"

bash "$ROOT/scripts/build.sh"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS=2 bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"

grep -Eq "\[ioapic\] controllers=[1-9][0-9]* bsp_apic_id=[0-9]+" "$LOG" || {
  echo "[FAIL] IOAPIC non initialisé" >&2
  exit 1
}

grep -Eq "\[ioapic\] madt_id=[0-9]+ hw_id=[0-9]+ version=0x[0-9a-f]+ .* redirs=[1-9][0-9]* id_match=1" "$LOG" || {
  echo "[FAIL] identité/version IOAPIC ou nombre de redirections non validé" >&2
  exit 1
}

grep -Eq "\[ioapic\] route irq=[0-9]+ gsi=[0-9]+ vector=0x[0-9a-f]+ polarity=(high|low) trigger=(edge|level) masked=1 override=1" "$LOG" || {
  echo "[FAIL] aucun Interrupt Source Override MADT appliqué" >&2
  exit 1
}

grep -Eq "\[ioapic\] legacy_routes=[1-9][0-9]* overrides_applied=[1-9][0-9]* shadowed=[0-9]+ id_mismatches=[0-9]+ masked=[1-9][0-9]*/[1-9][0-9]* all_masked=1" "$LOG" || {
  echo "[FAIL] masque global des redirections IOAPIC non prouvé" >&2
  exit 1
}

grep -Fq "ELYON_IOAPIC_READY" "$LOG" || {
  echo "[FAIL] marqueur ELYON_IOAPIC_READY absent" >&2
  exit 1
}

grep -Fq "ELYON_KERNEL_READY_0_0_4" "$LOG" || {
  echo "[FAIL] régression du boot nominal après IOAPIC (qemu rc=$RC)" >&2
  exit 1
}

echo "[PASS] K003-05 IOAPIC lu, redirections masquées et overrides MADT appliqués"
