#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="$ROOT/build/qemu-sched-context.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-45}"
TARGET=30
MIN_BUSY_PREEMPTIONS=10

bash "$ROOT/scripts/build.sh"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS=4 bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"

grep -Fq "ELYON_SCHED_PREEMPT_READY" "$LOG" || {
  echo "[FAIL] régression K004-04 avant K004-05" >&2
  exit 1
}

if grep -Fq "[context-integrity] FAILED" "$LOG"; then
  echo "[FAIL] K004-05: corruption de contexte détectée par le noyau" >&2
  grep -F "[context-integrity] FAILED" "$LOG" >&2
  exit 1
fi

summary="$(tr -d '\r' <"$LOG" | grep -E '^\[context-integrity\] tasks=[0-9]+,[0-9]+,[0-9]+ ' || true)"
if [[ -z "$summary" ]]; then
  echo "[FAIL] K004-05: synthèse [context-integrity] absente" >&2
  exit 1
fi
field() { sed -nE "s/.* $1=([0-9]+)( .*|$)/\1/p" <<<"$summary"; }
triple() { sed -nE "s/.* $1=([0-9]+,[0-9]+,[0-9]+)( .*|$)/\1/p" <<<"$summary"; }

IFS=, read -r -a tasks <<<"$(triple tasks)"
if [[ "${#tasks[@]}" -ne 3 ]] || [[ "$(printf '%s\n' "${tasks[@]}" | sort -u | wc -l)" -ne 3 ]]; then
  echo "[FAIL] K004-05: 3 tâches distinctes attendues: $summary" >&2
  exit 1
fi
if [[ "$(field registers)" != 15 || "$(field df)" != 1 || "$(field corruptions)" != 0 ]]; then
  echo "[FAIL] K004-05: couverture registres/DF ou corruption: $summary" >&2
  exit 1
fi
if (( $(field preemptions) < TARGET || $(field min_slices) < 10 )); then
  echo "[FAIL] K004-05: préemptions ou part minimale insuffisantes: $summary" >&2
  exit 1
fi

# Chaque tâche doit avoir été préemptée en pleine boucle de vérification, et
# avoir effectivement vérifié ses registres.
IFS=, read -r -a busy <<<"$(triple busy_preemptions)"
IFS=, read -r -a iterations <<<"$(triple iterations)"
for i in 0 1 2; do
  if (( ${busy[i]:-0} < MIN_BUSY_PREEMPTIONS )); then
    echo "[FAIL] K004-05: tâche ${tasks[i]} préemptée ${busy[i]:-0} fois dans sa boucle, au moins $MIN_BUSY_PREEMPTIONS attendues" >&2
    exit 1
  fi
  if (( ${iterations[i]:-0} == 0 )); then
    echo "[FAIL] K004-05: tâche ${tasks[i]} sans itération de vérification" >&2
    exit 1
  fi
done

if [[ "$(field exits)" != 3 || "$(field running_max)" != 1 || "$(field eoi_failures)" != 0 || "$(field canaries_ok)" != 3 ]]; then
  echo "[FAIL] K004-05: sorties, invariant RUNNING, EOI ou canaris invalides: $summary" >&2
  exit 1
fi

grep -Fq "ELYON_SCHED_CONTEXT_READY" "$LOG" || {
  echo "[FAIL] marqueur ELYON_SCHED_CONTEXT_READY absent" >&2
  exit 1
}

grep -Fq "ELYON_KERNEL_READY_0_0_4" "$LOG" || {
  echo "[FAIL] boot nominal 0.0.4 non atteint après K004-05 (qemu rc=$RC)" >&2
  exit 1
}

if grep -Eq '\[sched\] FAILED|\[KERNEL PANIC\]|\[EXCEPTION\]' "$LOG"; then
  echo "[FAIL] erreur pendant la qualification du contexte" >&2
  exit 1
fi

echo "[PASS] K004-05 contexte CPU: 15 registres + DF intacts sous préemption, ${busy[0]},${busy[1]},${busy[2]} préemptions en boucle, aucune corruption"
