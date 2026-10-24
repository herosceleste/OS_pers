#!/usr/bin/env bash
set -euo pipefail

# K004-11 : stress SMP, nombreuses tâches et volume élevé de context switches.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Avec ELYON_CHECK_LOG, vérifie ce log existant (VMware) sans construire ni booter.
LOG="${ELYON_CHECK_LOG:-$ROOT/build/qemu-sched-stress.log}"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-120}"
CPUS="${ELYON_CPUS:-4}"
# Paramètres de la qualification (sched.rs), recalculés ici sans faire confiance au log.
WAVES=9
TASKS_PER_AP=40
ROUNDS=100
ROLES=4
QUEUE_CAPACITY=64
# Borne d.attente en Round-Robin : la longueur maximale d.une file.

if [[ "$CPUS" -lt 2 ]]; then
  echo "[FAIL] K004-11 demande au moins 2 CPU (ELYON_CPUS=$CPUS)" >&2
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
  echo "[FAIL] K004-11: $*" >&2
  exit 1
}

grep -Fq "ELYON_SCHED_SYNC_READY" <<<"$CLEAN" || {
  echo "[FAIL] régression K004-10 avant K004-11" >&2
  exit 1
}

if grep -Eq '^\[lock\] FAILED' <<<"$CLEAN"; then
  grep -E '^\[lock\] FAILED' <<<"$CLEAN" >&2
  fail "violation d'ordre des verrous ou verrou tenu pendant un switch"
fi
if grep -Eq '^\[stress\] (FAILED|skipped|wave=)' <<<"$CLEAN"; then
  grep -E '^\[stress\] (FAILED|skipped|wave=)' <<<"$CLEAN" >&2
  fail "échec signalé par le noyau"
fi

value() { sed -nE "s/.* $2=([0-9]+)( .*|$)/\1/p" <<<"$1"; }
one() {
  local found
  found="$(grep -E "^\[stress\] $1" <<<"$CLEAN" || true)"
  [[ -n "$found" && "$(wc -l <<<"$found")" -eq 1 ]] || fail "ligne [stress] $2 absente ou répétée"
  echo "$found"
}

totals="$(one 'cpus=[0-9]+ waves=[0-9]+ tasks_per_wave=[0-9]+ rounds=[0-9]+ created=[0-9]+ exited=[0-9]+ reaped=[0-9]+ switches=[0-9]+$' totaux)"
roles="$(one 'roles simd_rounds=[0-9]+ sleeps=[0-9]+ blocks=[0-9]+ wakes=[0-9]+ moves=[0-9]+ corruptions=[0-9]+ canary_failures=[0-9]+ running_max=[0-9]+$' rôles)"
fairness="$(one 'fairness min_progress=[0-9]+ max_wait_switches=[0-9]+ initial_wait=[0-9]+ yield_wait=[0-9]+ limit=[0-9]+ idle_running_aps=[0-9]+$' équité)"
reclaim="$(one 'reclaim frames_before=[0-9]+ frames_after=[0-9]+ slots_before=[0-9]+ slots_after=[0-9]+$' récupération)"

aps=$((CPUS - 1))
per_wave=$((aps * TASKS_PER_AP))
tasks=$((WAVES * per_wave))
# Tâches d'un rôle dans une vague : indices i < per_wave avec i % ROLES == rôle.
role_tasks() { echo $(((per_wave - $1 + ROLES - 1) / ROLES)); }

[[ "$(value "$totals" cpus)" == "$CPUS" ]] || fail "CPU: $totals"
[[ "$(value "$totals" waves)" == "$WAVES" && "$(value "$totals" tasks_per_wave)" == "$per_wave" \
  && "$(value "$totals" rounds)" == "$ROUNDS" ]] || fail "paramètres inattendus: $totals"
[[ "$(value "$totals" created)" == "$tasks" ]] || fail "$(value "$totals" created) tâches créées, $tasks attendues"
[[ "$(value "$totals" exited)" == "$tasks" ]] || fail "toutes les tâches ne se sont pas terminées: $totals"
[[ "$(value "$totals" reaped)" == "$tasks" ]] || fail "toutes les tâches n'ont pas été récupérées: $totals"
min_switches=$((tasks * ROUNDS / 2))
[[ "$(value "$totals" switches)" -ge "$min_switches" ]] || fail "moins de $min_switches switchs: $totals"

# Chaque tâche simd fait ROUNDS tours SIMD, chaque sleep dort un tour sur dix,
# chaque block se bloque un tour sur cinq et n'est réveillée qu'une fois par blocage.
[[ "$(value "$roles" simd_rounds)" == $((WAVES * $(role_tasks 1) * ROUNDS)) ]] || fail "tours SIMD: $roles"
[[ "$(value "$roles" sleeps)" == $((WAVES * $(role_tasks 2) * ROUNDS / 10)) ]] || fail "sommeils: $roles"
blocks=$((WAVES * $(role_tasks 3) * ROUNDS / 5))
[[ "$(value "$roles" blocks)" == "$blocks" && "$(value "$roles" wakes)" == "$blocks" ]] \
  || fail "blocages ou réveils: $roles (attendus $blocks)"
if [[ "$aps" -ge 2 ]]; then
  [[ "$(value "$roles" moves)" -gt 0 ]] || fail "aucun déplacement d'affinité: $roles"
fi
[[ "$(value "$roles" corruptions)" == 0 ]] || fail "registres SIMD corrompus: $roles"
[[ "$(value "$roles" canary_failures)" == 0 ]] || fail "canari de pile écrasé: $roles"
[[ "$(value "$roles" running_max)" == 1 ]] || fail "une tâche a tourné sur deux CPU: $roles"

[[ "$(value "$fairness" min_progress)" == "$ROUNDS" ]] || fail "une tâche n'a pas fini ses tours: $fairness"
[[ "$(value "$fairness" limit)" == $QUEUE_CAPACITY ]] || fail "borne d'attente inattendue: $fairness"
initial_wait="$(value "$fairness" initial_wait)"
yield_wait="$(value "$fairness" yield_wait)"
[[ "$(value "$fairness" max_wait_switches)" == $((initial_wait > yield_wait ? initial_wait : yield_wait)) ]] \
  || fail "attente maximale incohérente: $fairness"
[[ "$initial_wait" -le $QUEUE_CAPACITY && "$yield_wait" -le $QUEUE_CAPACITY ]] || fail "famine: $fairness"
[[ "$(value "$fairness" idle_running_aps)" == "$aps" ]] || fail "AP hors de leur idle task: $fairness"

frames="$(value "$reclaim" frames_before)"
slots="$(value "$reclaim" slots_before)"
[[ "$frames" -gt 0 && "$(value "$reclaim" frames_after)" == "$frames" ]] || fail "pages non rendues ou rendues deux fois: $reclaim"
[[ "$slots" -gt 0 && "$(value "$reclaim" slots_after)" == "$slots" ]] || fail "slots non rendus: $reclaim"
# Une vague entière doit tenir dans les slots libres, sinon rien n'a été réutilisé.
[[ "$slots" -lt "$tasks" ]] || fail "aucune réutilisation de slot prouvée: $slots slots libres pour $tasks tâches"

order="$(grep -nE '^(ELYON_SCHED_SYNC_READY|ELYON_SCHED_STRESS_READY|ELYON_KERNEL_READY_0_0_4)$' <<<"$CLEAN" | cut -d: -f2 | tr '\n' ' ')"
[[ "$order" == "ELYON_SCHED_SYNC_READY ELYON_SCHED_STRESS_READY ELYON_KERNEL_READY_0_0_4 " ]] \
  || fail "marqueurs absents ou dans le désordre: $order (qemu rc=$RC)"

if grep -Eq '\[sched\] FAILED|\[KERNEL PANIC\]|\[EXCEPTION\]|\[halt\]' <<<"$CLEAN"; then
  fail "erreur pendant la qualification"
fi

echo "[PASS] K004-11 stress SMP: $CPUS CPU, $tasks tâches en $WAVES vagues créées, terminées et récupérées, $(value "$totals" switches) switchs, $(value "$roles" simd_rounds) tours SIMD sans corruption, $blocks blocages réveillés, attente max $(value "$fairness" max_wait_switches) switchs (borne $QUEUE_CAPACITY), pages et slots restitués"
