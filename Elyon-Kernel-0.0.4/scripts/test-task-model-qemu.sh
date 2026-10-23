#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$ROOT/build/qemu-task-model.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-35}"

bash "$ROOT/scripts/build.sh"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS=4 bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"

grep -Eq "\[task\] ids=[1-9][0-9]*,[1-9][0-9]* unique=1 capacity=256 created=2 transitions=12 invalid_rejected=2 states_seen=0x1f" "$LOG" || {
  echo "[FAIL] modèle de tâche, IDs ou transitions K004-01 invalides" >&2
  exit 1
}

grep -Fq "[task] states=READY,RUNNING,BLOCKED,SLEEPING,DEAD affinity_enforced=1" "$LOG" || {
  echo "[FAIL] tous les états de tâche ou l'affinité ne sont pas qualifiés" >&2
  exit 1
}

grep -Fq "ELYON_TASK_MODEL_READY" "$LOG" || {
  echo "[FAIL] marqueur ELYON_TASK_MODEL_READY absent" >&2
  exit 1
}

grep -Fq "ELYON_SMP_STABLE" "$LOG" || {
  echo "[FAIL] régression SMP pendant K004-01" >&2
  exit 1
}

grep -Fq "ELYON_CONTEXT_SWITCH_READY" "$LOG" || {
  echo "[FAIL] régression context-switch pendant K004-01" >&2
  exit 1
}

grep -Fq "ELYON_KERNEL_READY_0_0_4" "$LOG" || {
  echo "[FAIL] boot nominal 0.0.4 non atteint après K004-01 (qemu rc=$RC)" >&2
  exit 1
}

if grep -Fq "[task] FAILED" "$LOG"; then
  echo "[FAIL] le noyau a signalé une erreur de modèle de tâche" >&2
  exit 1
fi

echo "[PASS] K004-01 modèle de tâche noyau, 5 états, IDs uniques, affinité et transitions invalides rejetées"
