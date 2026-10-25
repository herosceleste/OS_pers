#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$ROOT/build/qemu-context-switch.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-35}"

bash "$ROOT/scripts/build.sh"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS=4 bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"

grep -Eq "\[context\] task phase=1 switches=1 rsp=0x[0-9a-f]+" "$LOG" || {
  echo "[FAIL] première entrée sur la pile de tâche non prouvée" >&2
  exit 1
}

grep -Eq "\[context\] task phase=2 switches=3 rsp=0x[0-9a-f]+" "$LOG" || {
  echo "[FAIL] reprise du contexte de tâche au second passage non prouvée" >&2
  exit 1
}

grep -Eq "\[context\] task_stack=0x[0-9a-f]+\.\.0x[0-9a-f]+ size=64 KiB source=PMM" "$LOG" || {
  echo "[FAIL] pile dédiée de tâche PMM non prouvée" >&2
  exit 1
}

grep -Eq "\[context\] saved bsp_rsp=0x[0-9a-f]+ task_rsp_first=0x[0-9a-f]+ task_rsp_final=0x[0-9a-f]+" "$LOG" || {
  echo "[FAIL] RSP sauvegardés du BSP et de la tâche absents" >&2
  exit 1
}

grep -Fq "[context] result phases=2 switches=4 online_before=4 online_after=4 prepared=0" "$LOG" || {
  echo "[FAIL] deux allers-retours ou stabilité SMP non prouvés" >&2
  exit 1
}

grep -Fq "ELYON_CONTEXT_SWITCH_READY" "$LOG" || {
  echo "[FAIL] marqueur ELYON_CONTEXT_SWITCH_READY absent" >&2
  exit 1
}

if grep -Fq "[context] FAILED" "$LOG"; then
  echo "[FAIL] qualification de changement de contexte en erreur" >&2
  exit 1
fi

grep -Fq "ELYON_SMP_STABLE" "$LOG" || {
  echo "[FAIL] stabilité SMP perdue pendant K003-11" >&2
  exit 1
}

grep -Fq "ELYON_KERNEL_READY_0_0_5" "$LOG" || {
  echo "[FAIL] noyau non revenu au chemin nominal après context switch (qemu rc=$RC)" >&2
  exit 1
}

echo "[PASS] K003-11 contexte BSP sauvegardé, tâche sur pile dédiée reprise deux fois, 4 switches et retour nominal"
