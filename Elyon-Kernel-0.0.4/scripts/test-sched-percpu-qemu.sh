#!/usr/bin/env bash
set -euo pipefail

# K004-07 : scheduler per-CPU et idle task officielle sur chaque CPU.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Avec ELYON_CHECK_LOG, vérifie ce log existant (VMware) sans construire ni booter.
LOG="${ELYON_CHECK_LOG:-$ROOT/build/qemu-sched-percpu.log}"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-70}"
CPUS="${ELYON_CPUS:-4}"
ITERATIONS=3

RC=0
if [[ -z "${ELYON_CHECK_LOG:-}" ]]; then
  bash "$ROOT/scripts/build.sh"

  set +e
  timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS="$CPUS" bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
  RC=$?
  set -e

  cat "$LOG"
fi

grep -Fq "ELYON_SCHED_FPU_READY" "$LOG" || {
  echo "[FAIL] régression K004-06 avant K004-07" >&2
  exit 1
}

if tr -d '\r' <"$LOG" | grep -Eq '^\[percpu-sched\] (FAILED|trace_error)'; then
  echo "[FAIL] K004-07: échec signalé par le noyau" >&2
  tr -d '\r' <"$LOG" | grep -E '^\[percpu-sched\] (FAILED|trace_error)' >&2
  exit 1
fi

mapfile -t LINES < <(tr -d '\r' <"$LOG" | grep -E '^\[percpu-sched\] cpu=[0-9]+ idle=[0-9]+ tasks=[0-9]+,[0-9]+ runs=[0-9,]+$')
if [[ "${#LINES[@]}" -ne "$CPUS" ]]; then
  echo "[FAIL] K004-07: ${#LINES[@]} CPU rapportés, $CPUS attendus" >&2
  exit 1
fi

# Vérification indépendante du noyau : sur chaque CPU, exactement ITERATIONS
# tours de ses deux tâches dans l'ordre de sa file, puis son idle task ; aucune
# tâche ni idle partagée entre deux CPU.
declare -A seen=()
for i in "${!LINES[@]}"; do
  line="${LINES[i]}"
  cpu="$(sed -nE 's/.* cpu=([0-9]+) .*/\1/p' <<<"$line")"
  idle="$(sed -nE 's/.* idle=([0-9]+) .*/\1/p' <<<"$line")"
  IFS=, read -r -a tasks <<<"$(sed -nE 's/.* tasks=([0-9,]+) .*/\1/p' <<<"$line")"
  IFS=, read -r -a runs <<<"$(sed -nE 's/.* runs=([0-9,]+)$/\1/p' <<<"$line")"
  if [[ "$cpu" != "$i" ]]; then
    echo "[FAIL] K004-07: ligne CPU inattendue: $line" >&2
    exit 1
  fi
  for id in "$idle" "${tasks[@]}"; do
    if [[ -n "${seen[$id]:-}" ]]; then
      echo "[FAIL] K004-07: tâche $id présente sur deux CPU" >&2
      exit 1
    fi
    seen[$id]=1
  done
  expected=()
  for ((round = 0; round < ITERATIONS; round++)); do
    expected+=("${tasks[0]}" "${tasks[1]}")
  done
  expected+=("$idle")
  if [[ "${runs[*]}" != "${expected[*]}" ]]; then
    echo "[FAIL] K004-07: CPU $cpu a exécuté [${runs[*]}], attendu [${expected[*]}]" >&2
    exit 1
  fi
done

summary="$(tr -d '\r' <"$LOG" | grep -E '^\[percpu-sched\] cpus=' || true)"
field() { sed -nE "s/.* $1=([0-9]+)( .*|$)/\1/p" <<<"$summary"; }
if [[ -z "$summary" ]]; then
  echo "[FAIL] K004-07: synthèse [percpu-sched] absente" >&2
  exit 1
fi
aps=$((CPUS - 1))
if [[ "$(field cpus)" != "$CPUS" || "$(field tasks_per_cpu)" != 2 || "$(field iterations)" != "$ITERATIONS" \
  || "$(field trace_entries)" != $((CPUS * (2 * ITERATIONS + 1))) || "$(field affinity_ok)" != 1 \
  || "$(field rotation_ok)" != 1 || "$(field idle_ran)" != "$CPUS" || "$(field idle_running_aps)" != "$aps" \
  || "$(field ap_fpu_ready)" != "$aps" || "$(field running_max)" != 1 || "$(field canaries_ok)" != $((CPUS * 3)) ]]; then
  echo "[FAIL] K004-07: synthèse invalide: $summary" >&2
  exit 1
fi

grep -Fq "ELYON_SCHED_PERCPU_READY" "$LOG" || {
  echo "[FAIL] marqueur ELYON_SCHED_PERCPU_READY absent" >&2
  exit 1
}

grep -Fq "ELYON_KERNEL_READY_0_0_4" "$LOG" || {
  echo "[FAIL] boot nominal 0.0.4 non atteint après K004-07 (qemu rc=$RC)" >&2
  exit 1
}

if grep -Eq '\[sched\] FAILED|\[KERNEL PANIC\]|\[EXCEPTION\]' "$LOG"; then
  echo "[FAIL] erreur pendant la qualification per-CPU" >&2
  exit 1
fi

echo "[PASS] K004-07 scheduler per-CPU: $CPUS CPU, 2 tâches par CPU en Round-Robin sur leur seul CPU, idle officielle sur chaque CPU ($aps AP en idle), FPU des AP prête"
