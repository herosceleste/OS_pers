#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$ROOT/build/qemu-task-stacks.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-35}"

bash "$ROOT/scripts/build.sh"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS=4 bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"

grep -Fq "ELYON_TASK_MODEL_READY" "$LOG" || {
  echo "[FAIL] régression K004-01 avant K004-02" >&2
  exit 1
}

mapfile -t TASKS < <(
  tr -d '\r' <"$LOG" | grep -E '^\[task-stack\] task=[0-9]+ stack=0x[0-9a-f]+\.\.0x[0-9a-f]+ size=64 KiB initial_rsp=0x[0-9a-f]+ observed_rsp=0x[0-9a-f]+ saved_rsp=0x[0-9a-f]+ arg=0x[0-9a-f]+ canary=OK$'
)
if [[ "${#TASKS[@]}" -ne 3 ]]; then
  echo "[FAIL] K004-02: 3 tâches avec pile dédiée attendues, obtenues: ${#TASKS[@]}" >&2
  exit 1
fi

# Vérification indépendante du noyau : chaque RSP relevé est dans la pile de sa
# tâche, et les trois piles sont deux à deux disjointes.
starts=()
ends=()
for line in "${TASKS[@]}"; do
  start=$((16#$(sed -nE 's/.* stack=0x([0-9a-f]+)\.\..*/\1/p' <<<"$line")))
  end=$((16#$(sed -nE 's/.* stack=0x[0-9a-f]+\.\.0x([0-9a-f]+) .*/\1/p' <<<"$line")))
  for field in initial_rsp observed_rsp saved_rsp; do
    rsp=$((16#$(sed -nE "s/.* ${field}=0x([0-9a-f]+) .*/\1/p" <<<"$line")))
    if (( rsp < start || rsp >= end )); then
      echo "[FAIL] K004-02: ${field} hors de la pile de sa tâche: $line" >&2
      exit 1
    fi
  done
  for i in "${!starts[@]}"; do
    if (( start < ends[i] && starts[i] < end )); then
      echo "[FAIL] K004-02: piles de tâches qui se chevauchent" >&2
      exit 1
    fi
  done
  starts+=("$start")
  ends+=("$end")
done

grep -Fq "[task-stack] tasks=3 distinct=1 overlaps=0 owned=3 rounds=2 switches=12 args_ok=3 canaries_ok=3" "$LOG" || {
  echo "[FAIL] K004-02: synthèse piles/contextes invalide" >&2
  exit 1
}

grep -Fq "ELYON_TASK_STACKS_READY" "$LOG" || {
  echo "[FAIL] marqueur ELYON_TASK_STACKS_READY absent" >&2
  exit 1
}

grep -Fq "ELYON_KERNEL_READY_0_0_4" "$LOG" || {
  echo "[FAIL] boot nominal 0.0.4 non atteint après K004-02 (qemu rc=$RC)" >&2
  exit 1
}

if grep -Eq '\[task-stack\] FAILED|\[KERNEL PANIC\]|\[EXCEPTION\]' "$LOG"; then
  echo "[FAIL] erreur pendant la qualification des piles de tâches" >&2
  exit 1
fi

echo "[PASS] K004-02 3 tâches sur piles PMM dédiées disjointes, frames initiaux, 2 tours, arguments et canaris validés"
