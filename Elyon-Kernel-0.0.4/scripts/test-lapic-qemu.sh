#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$ROOT/build/qemu-lapic.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-20}"

bash "$ROOT/scripts/build.sh"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS=2 bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"

grep -Eq "\[apic\] cpuid apic=1 x2apic=[01]" "$LOG" || {
  echo "[FAIL] capacités APIC CPUID non confirmées" >&2
  exit 1
}

grep -Eq "\[apic\] IA32_APIC_BASE=0x[0-9a-f]+ madt_base=0x[0-9a-f]+ mode=(xapic|x2apic) bsp=1" "$LOG" || {
  echo "[FAIL] état IA32_APIC_BASE/BSP non confirmé" >&2
  exit 1
}

grep -Eq "\[apic\] BSP apic_id=[0-9]+ version=0x[0-9a-f]+ madt_match=1" "$LOG" || {
  echo "[FAIL] APIC ID BSP non corrélé à MADT" >&2
  exit 1
}

grep -Fq "[apic] SVR vector=0xff software_enable=1" "$LOG" || {
  echo "[FAIL] Spurious Interrupt Vector non activé" >&2
  exit 1
}

grep -Fq "[apic] EOI primitive=OK" "$LOG" || {
  echo "[FAIL] primitive EOI non opérationnelle" >&2
  exit 1
}

grep -Fq "ELYON_LAPIC_READY" "$LOG" || {
  echo "[FAIL] marqueur ELYON_LAPIC_READY absent" >&2
  exit 1
}

grep -Fq "ELYON_KERNEL_READY_0_0_4" "$LOG" || {
  echo "[FAIL] régression du boot nominal après Local APIC (qemu rc=$RC)" >&2
  exit 1
}

echo "[PASS] K003-04 Local APIC BSP activé, APIC ID corrélé à MADT, SVR et EOI validés"
