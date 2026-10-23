#!/usr/bin/env bash
set -euo pipefail

# K004-09 : sleep()/wakeup et file temporelle.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Avec ELYON_CHECK_LOG, vérifie ce log existant (VMware) sans construire ni booter.
LOG="${ELYON_CHECK_LOG:-$ROOT/build/qemu-sched-sleep.log}"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-70}"
CPUS="${ELYON_CPUS:-4}"
MAX_LATE=2

if [[ "$CPUS" -lt 2 ]]; then
  echo "[FAIL] K004-09 demande au moins 2 CPU (ELYON_CPUS=$CPUS)" >&2
  exit 1
fi

RC=0
if [[ -z "${ELYON_CHECK_LOG:-}" ]]; then
  bash "$ROOT/scripts/build.sh"

  set +e
  timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS="$CPUS" bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
  RC=$?
  set -e

  cat "$LOG"
fi

CLEAN="$(tr -d '\r' <"$LOG")"
fail() {
  echo "[FAIL] K004-09: $*" >&2
  exit 1
}

grep -Fq "ELYON_SCHED_IPI_READY" <<<"$CLEAN" || {
  echo "[FAIL] régression K004-08 avant K004-09" >&2
  exit 1
}

if grep -Eq '^\[sched-sleep\] (FAILED|trace_error|counts|skipped)' <<<"$CLEAN"; then
  grep -E '^\[sched-sleep\] (FAILED|trace_error|counts|skipped)' <<<"$CLEAN" >&2
  fail "échec signalé par le noyau"
fi

value() { sed -nE "s/.* $2=([0-9]+)( .*|$)/\1/p" <<<"$1"; }
hz_line="$(grep -E '^\[sched-sleep\] cpus=' <<<"$CLEAN" || true)"
HZ="$(value "$hz_line" hz)"
[[ -n "$HZ" && "$HZ" -gt 0 ]] || fail "fréquence du timer absente"
# Échéance recalculée indépendamment : arrondi au tick supérieur, plus le tick entamé.
expected_deadline() { echo $(($1 + ($2 * HZ + 999) / 1000 + 1)); }

mapfile -t TIMED < <(grep -E '^\[sched-sleep\] timed task=[0-9]+ cpu=[0-9]+ ms=[0-9]+ start=[0-9]+ deadline=[0-9]+ woke_at=[0-9]+ resumed_at=[0-9]+$' <<<"$CLEAN")
[[ "${#TIMED[@]}" -eq 3 ]] || fail "${#TIMED[@]} sommeils minutés rapportés, 3 attendus"

declare -A seen=()
while read -r id; do
  [[ -n "$id" ]] && seen[$id]=1
done < <(grep -E '^\[percpu-sched\] cpu=' <<<"$CLEAN" | sed -nE 's/.* idle=([0-9]+) tasks=([0-9]+),([0-9]+) .*/\1\n\2\n\3/p'
  grep -E '^\[sched-ipi\] (remote_spawn|block_wake|migrate) ' <<<"$CLEAN" | sed -nE 's/.* task=([0-9]+) .*/\1/p')
claim() {
  [[ -z "${seen[$1]:-}" ]] || fail "tâche $1 déjà utilisée"
  seen[$1]=1
}

ms_list=()
pairs=()
for i in "${!TIMED[@]}"; do
  line="${TIMED[i]}"
  task="$(value "$line" task)"
  cpu="$(value "$line" cpu)"
  ms="$(value "$line" ms)"
  start="$(value "$line" start)"
  deadline="$(value "$line" deadline)"
  woke="$(value "$line" woke_at)"
  resumed="$(value "$line" resumed_at)"
  claim "$task"
  ms_list+=("$ms")
  [[ "$cpu" == $((1 + i % (CPUS - 1))) ]] || fail "sommeil $i sur le CPU $cpu"
  [[ "$deadline" == "$(expected_deadline "$start" "$ms")" ]] || fail "échéance $deadline incohérente pour ${ms} ms depuis $start"
  ((woke >= deadline)) || fail "tâche $task réveillée au tick $woke avant son échéance $deadline"
  ((woke - deadline <= MAX_LATE)) || fail "tâche $task réveillée $((woke - deadline)) ticks en retard"
  ((resumed >= woke)) || fail "tâche $task reprise avant son réveil"
  pairs+=("$deadline $woke")
done
[[ "${ms_list[*]}" == "30 10 20" ]] || fail "durées [${ms_list[*]}], attendu [30 10 20]"
# Ordre : à échéance plus tardive, réveil jamais plus tôt.
sorted_wakes="$(printf '%s\n' "${pairs[@]}" | sort -n -k1,1 | awk '{print $2}' | tr '\n' ' ')"
[[ "$sorted_wakes" == "$(printf '%s\n' "${pairs[@]}" | awk '{print $2}' | sort -n | tr '\n' ' ')" ]] \
  || fail "réveils hors de l'ordre des échéances"

shared="$(grep -E '^\[sched-sleep\] shared cpu=[0-9]+ sleeper=[0-9]+ runner=[0-9]+ ran_while_sleeping=1 idle_entries=[0-9]+$' <<<"$CLEAN" || true)"
[[ -n "$shared" ]] || fail "scénario de CPU partagé absent"
[[ "$(value "$shared" cpu)" == 1 && "$(value "$shared" idle_entries)" == 1 ]] || fail "CPU partagé invalide: $shared"
claim "$(value "$shared" sleeper)"
claim "$(value "$shared" runner)"

explicit="$(grep -E '^\[sched-sleep\] explicit task=[0-9]+ cpu=[0-9]+ ms=[0-9]+ start=[0-9]+ deadline=[0-9]+ woke_at=[0-9]+ stale_expiries=[0-9]+$' <<<"$CLEAN" || true)"
[[ -n "$explicit" ]] || fail "scénario de réveil explicite absent"
claim "$(value "$explicit" task)"
e_start="$(value "$explicit" start)"
e_deadline="$(value "$explicit" deadline)"
e_woke="$(value "$explicit" woke_at)"
[[ "$e_deadline" == "$(expected_deadline "$e_start" "$(value "$explicit" ms)")" ]] || fail "échéance du réveil explicite incohérente"
((e_woke > e_start && e_woke < e_deadline)) || fail "réveil explicite au tick $e_woke hors de ]$e_start, $e_deadline["
[[ "$(value "$explicit" stale_expiries)" == 0 ]] || fail "échéance périmée déclenchée après un réveil explicite"

zero="$(grep -E '^\[sched-sleep\] zero task=[0-9]+ stayed_awake=1$' <<<"$CLEAN" || true)"
[[ -n "$zero" ]] || fail "scénario sleep(0) absent"
claim "$(value "$zero" task)"

max_late="$(value "$hz_line" max_late)"
if [[ "$(value "$hz_line" cpus)" != "$CPUS" || "$(value "$hz_line" sleeps)" != 6 \
  || "$(value "$hz_line" timer_wakes)" != 5 || "$(value "$hz_line" explicit_wakes)" != 1 \
  || "$(value "$hz_line" order_ok)" != 1 || "$(value "$hz_line" never_early)" != 1 \
  || -z "$max_late" || "$max_late" -gt "$MAX_LATE" \
  || "$(value "$hz_line" ipi_sent)" != 12 || "$(value "$hz_line" ipi_received)" != 12 \
  || "$(value "$hz_line" timer_queue_empty)" != 1 || "$(value "$hz_line" idle_running_aps)" != $((CPUS - 1)) \
  || "$(value "$hz_line" running_max)" != 1 || "$(value "$hz_line" canaries_ok)" != 7 ]]; then
  fail "synthèse invalide: $hz_line"
fi

order="$(grep -nE '^(ELYON_SCHED_IPI_READY|ELYON_SCHED_SLEEP_READY|ELYON_KERNEL_READY_0_0_4)$' <<<"$CLEAN" | cut -d: -f2 | tr '\n' ' ')"
[[ "$order" == "ELYON_SCHED_IPI_READY ELYON_SCHED_SLEEP_READY ELYON_KERNEL_READY_0_0_4 " ]] \
  || fail "marqueurs absents ou dans le désordre: $order (qemu rc=$RC)"

if grep -Eq '\[sched\] FAILED|\[KERNEL PANIC\]|\[EXCEPTION\]' <<<"$CLEAN"; then
  fail "erreur pendant la qualification"
fi

echo "[PASS] K004-09 sleep/wakeup: $CPUS CPU, sommeils de 30/10/20 ms réveillés dans l'ordre jamais en avance (retard max $max_late tick), CPU partagé, réveil explicite sans échéance périmée, sleep(0), IPI envoyés=reçus=12"
