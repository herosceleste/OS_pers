#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$ROOT/build/qemu-sched-rr.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-35}"
ITERATIONS=4

bash "$ROOT/scripts/build.sh"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS=4 bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"

grep -Fq "ELYON_TASK_STACKS_READY" "$LOG" || {
  echo "[FAIL] régression K004-02 avant K004-03" >&2
  exit 1
}

line="$(tr -d '\r' <"$LOG" | grep -E '^\[sched\] tasks=[0-9]+,[0-9]+,[0-9]+ trace=[0-9]+(,[0-9]+)*$' || true)"
if [[ -z "$line" ]]; then
  echo "[FAIL] K004-03: ligne [sched] tasks/trace absente" >&2
  exit 1
fi

# Vérification indépendante du noyau : la trace est exactement ITERATIONS tours
# des trois tâches, dans l'ordre de la run queue, sans doublon de tâche.
IFS=, read -r -a tasks <<<"$(sed -nE 's/.* tasks=([0-9,]+) .*/\1/p' <<<"$line")"
IFS=, read -r -a trace <<<"$(sed -nE 's/.* trace=([0-9,]+)$/\1/p' <<<"$line")"

if [[ "${#tasks[@]}" -ne 3 ]] || [[ "$(printf '%s\n' "${tasks[@]}" | sort -u | wc -l)" -ne 3 ]]; then
  echo "[FAIL] K004-03: 3 tâches distinctes attendues: ${tasks[*]}" >&2
  exit 1
fi
if [[ "${#trace[@]}" -ne $((3 * ITERATIONS)) ]]; then
  echo "[FAIL] K004-03: trace de ${#trace[@]} passages, $((3 * ITERATIONS)) attendus" >&2
  exit 1
fi
for i in "${!trace[@]}"; do
  if [[ "${trace[i]}" != "${tasks[i % 3]}" ]]; then
    echo "[FAIL] K004-03: rotation rompue au passage $i (${trace[i]} au lieu de ${tasks[i % 3]})" >&2
    exit 1
  fi
done

grep -Fq "[sched] policy=round-robin cooperative iterations=4 yields=9 exits=3 task_switches=11 bsp_switches=2 running_max=1 args_ok=3 rsp_ok=12 canaries_ok=3 queue_empty=1" "$LOG" || {
  echo "[FAIL] K004-03: synthèse scheduler invalide" >&2
  exit 1
}

grep -Fq "ELYON_SCHED_RR_READY" "$LOG" || {
  echo "[FAIL] marqueur ELYON_SCHED_RR_READY absent" >&2
  exit 1
}

grep -Fq "ELYON_KERNEL_READY_0_0_4" "$LOG" || {
  echo "[FAIL] boot nominal 0.0.4 non atteint après K004-03 (qemu rc=$RC)" >&2
  exit 1
}

if grep -Eq '\[sched\] FAILED|\[KERNEL PANIC\]|\[EXCEPTION\]' "$LOG"; then
  echo "[FAIL] erreur pendant la qualification du scheduler" >&2
  exit 1
fi

echo "[PASS] K004-03 Round-Robin coopératif: 3 tâches, 4 tours stricts, 11 switches directs tâche→tâche, une seule tâche RUNNING"
