#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$ROOT/build/qemu-sched-preempt.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-40}"
TARGET=30
MIN_SLICES=10

bash "$ROOT/scripts/build.sh"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS=4 bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"

for marker in ELYON_SCHED_RR_READY ELYON_TIMER_READY; do
  grep -Fq "$marker" "$LOG" || {
    echo "[FAIL] régression avant K004-04: $marker absent" >&2
    exit 1
  }
done

line="$(tr -d '\r' <"$LOG" | grep -E '^\[preempt\] tasks=[0-9]+,[0-9]+,[0-9]+ trace=[0-9]+(,[0-9]+)*$' || true)"
if [[ -z "$line" ]]; then
  echo "[FAIL] K004-04: ligne [preempt] tasks/trace absente" >&2
  exit 1
fi

# Vérification indépendante du noyau : la trace des tranches compte exactement
# TARGET+1 entrées en rotation stricte, et chaque tâche en reçoit au moins
# MIN_SLICES.
IFS=, read -r -a tasks <<<"$(sed -nE 's/.* tasks=([0-9,]+) .*/\1/p' <<<"$line")"
IFS=, read -r -a trace <<<"$(sed -nE 's/.* trace=([0-9,]+)$/\1/p' <<<"$line")"

if [[ "${#tasks[@]}" -ne 3 ]] || [[ "$(printf '%s\n' "${tasks[@]}" | sort -u | wc -l)" -ne 3 ]]; then
  echo "[FAIL] K004-04: 3 tâches distinctes attendues: ${tasks[*]}" >&2
  exit 1
fi
if [[ "${#trace[@]}" -ne $((TARGET + 1)) ]]; then
  echo "[FAIL] K004-04: trace de ${#trace[@]} tranches, $((TARGET + 1)) attendues" >&2
  exit 1
fi
for i in "${!trace[@]}"; do
  if [[ "${trace[i]}" != "${tasks[i % 3]}" ]]; then
    echo "[FAIL] K004-04: rotation rompue à la tranche $i (${trace[i]} au lieu de ${tasks[i % 3]})" >&2
    exit 1
  fi
done
for task in "${tasks[@]}"; do
  slices="$(printf '%s\n' "${trace[@]}" | grep -cx "$task")"
  if (( slices < MIN_SLICES )); then
    echo "[FAIL] K004-04: tâche $task servie $slices fois, au moins $MIN_SLICES attendues" >&2
    exit 1
  fi
done

summary="$(tr -d '\r' <"$LOG" | grep -E '^\[preempt\] policy=timer-preemptive ' || true)"
if [[ -z "$summary" ]]; then
  echo "[FAIL] K004-04: synthèse [preempt] absente" >&2
  exit 1
fi
field() { sed -nE "s/.* $1=([0-9]+).*/\1/p" <<<"$summary"; }

if [[ "$(field quantum_ticks)" != 1 || "$(field target)" != "$TARGET" ]]; then
  echo "[FAIL] K004-04: quantum ou cible inattendus: $summary" >&2
  exit 1
fi
if (( $(field preemptions) < TARGET || $(field task_ticks) < TARGET )); then
  echo "[FAIL] K004-04: moins de $TARGET préemptions ou ticks de tâche: $summary" >&2
  exit 1
fi
if (( $(field min_slices) < MIN_SLICES )); then
  echo "[FAIL] K004-04: part minimale insuffisante: $summary" >&2
  exit 1
fi
IFS=, read -r -a counters <<<"$(sed -nE 's/.* counters=([0-9,]+) .*/\1/p' <<<"$summary")"
if [[ "${#counters[@]}" -ne 3 ]]; then
  echo "[FAIL] K004-04: 3 compteurs de progression attendus: $summary" >&2
  exit 1
fi
for counter in "${counters[@]}"; do
  if (( counter == 0 )); then
    echo "[FAIL] K004-04: une tâche préemptible n'a pas progressé: $summary" >&2
    exit 1
  fi
done
if [[ "$(field exits)" != 3 || "$(field running_max)" != 1 || "$(field eoi_failures)" != 0 || "$(field canaries_ok)" != 3 ]]; then
  echo "[FAIL] K004-04: sorties, invariant RUNNING, EOI ou canaris invalides: $summary" >&2
  exit 1
fi

grep -Fq "ELYON_SCHED_PREEMPT_READY" "$LOG" || {
  echo "[FAIL] marqueur ELYON_SCHED_PREEMPT_READY absent" >&2
  exit 1
}

grep -Fq "ELYON_KERNEL_READY_0_0_5" "$LOG" || {
  echo "[FAIL] boot nominal 0.0.5 non atteint après K004-04 (qemu rc=$RC)" >&2
  exit 1
}

if grep -Eq '\[preempt\] FAILED|\[sched\] FAILED|\[KERNEL PANIC\]|\[EXCEPTION\]' "$LOG"; then
  echo "[FAIL] erreur pendant la qualification de la préemption" >&2
  exit 1
fi

echo "[PASS] K004-04 préemption LAPIC: 3 tâches sans yield, $(field preemptions) préemptions timer, $((TARGET + 1)) tranches en rotation stricte, une seule tâche RUNNING"
