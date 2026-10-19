#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$ROOT/build/qemu-smp-one-ap.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-25}"

bash "$ROOT/scripts/build.sh"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS=2 bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"

grep -Eq "\[smp\] AP online cpu=1 apic_id=1 selector=0x[0-9a-f]+ tr=0x[0-9a-f]+" "$LOG" || {
  echo "[FAIL] AP n'a pas chargé son contexte per-CPU/GDT/TSS" >&2
  exit 1
}

grep -Fq "ELYON_AP_STARTED cpu=1 apic_id=1" "$LOG" || {
  echo "[FAIL] marqueur de démarrage réel de l'AP absent" >&2
  exit 1
}

grep -Fq "[smp] trampoline=0x8000 sipi_vector=0x8 cpu=1 apic_id=1 online=2 prepared=0" "$LOG" || {
  echo "[FAIL] séquence INIT/SIPI ou transition online non prouvée" >&2
  exit 1
}

grep -Fq "ELYON_AP_BOOT_READY" "$LOG" || {
  echo "[FAIL] marqueur ELYON_AP_BOOT_READY absent" >&2
  exit 1
}

grep -Fq "ELYON_KERNEL_READY_0_0_3" "$LOG" || {
  echo "[FAIL] BSP instable après démarrage AP (qemu rc=$RC)" >&2
  exit 1
}

echo "[PASS] K003-08 un AP a réellement démarré par INIT/SIPI et atteint Elyon en mode long"
