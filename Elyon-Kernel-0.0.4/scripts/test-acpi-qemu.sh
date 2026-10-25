#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$ROOT/build/qemu-acpi.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-20}"

bash "$ROOT/scripts/build.sh"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS=2 bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"

grep -Fq "[acpi] RSDP=OK" "$LOG" || {
  echo "[FAIL] RSDP non valide ou non atteint" >&2
  exit 1
}

grep -Eq "\[acpi\] root=(XSDT|RSDT) entries=[1-9][0-9]* validated=[1-9][0-9]*" "$LOG" || {
  echo "[FAIL] parcours XSDT/RSDT non prouvé" >&2
  exit 1
}

grep -Fq "[acpi] MADT=OK" "$LOG" || {
  echo "[FAIL] MADT non parsée" >&2
  exit 1
}

grep -Eq "\[acpi\] cpu_count=2 usable=2 ioapics=[1-9][0-9]*" "$LOG" || {
  echo "[FAIL] topologie MADT attendue pour 2 vCPU absente" >&2
  exit 1
}

grep -Fq "ELYON_ACPI_READY" "$LOG" || {
  echo "[FAIL] marqueur ELYON_ACPI_READY absent" >&2
  exit 1
}

grep -Fq "ELYON_KERNEL_READY_0_0_4" "$LOG" || {
  echo "[FAIL] régression du boot nominal après parsing ACPI (qemu rc=$RC)" >&2
  exit 1
}

echo "[PASS] K003-01/K003-02/K003-03 RSDP, XSDT/RSDT et MADT validés"
