#!/usr/bin/env bash
set -euo pipefail

# K004-10 : spinlocks irq-save et invariants de synchronisation scheduler/IRQ/SMP.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Avec ELYON_CHECK_LOG, vérifie ce log existant (VMware) sans construire ni booter.
LOG="${ELYON_CHECK_LOG:-$ROOT/build/qemu-sched-sync.log}"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-90}"
CPUS="${ELYON_CPUS:-4}"
COUNTER_PER_AP=2

if [[ "$CPUS" -lt 2 ]]; then
  echo "[FAIL] K004-10 demande au moins 2 CPU (ELYON_CPUS=$CPUS)" >&2
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
  echo "[FAIL] K004-10: $*" >&2
  exit 1
}

grep -Fq "ELYON_SCHED_SLEEP_READY" <<<"$CLEAN" || {
  echo "[FAIL] régression K004-09 avant K004-10" >&2
  exit 1
}

if grep -Eq '^\[lock\] FAILED' <<<"$CLEAN"; then
  grep -E '^\[lock\] FAILED' <<<"$CLEAN" >&2
  fail "violation d'ordre des verrous ou verrou tenu pendant un switch"
fi
if grep -Eq '^\[sched-sync\] (FAILED|counts|skipped)' <<<"$CLEAN"; then
  grep -E '^\[sched-sync\] (FAILED|counts|skipped)' <<<"$CLEAN" >&2
  fail "échec signalé par le noyau"
fi

value() { sed -nE "s/.* $2=([0-9]+)( .*|$)/\1/p" <<<"$1"; }
list() { sed -nE "s/.* $2=([0-9,]+)( .*|$)/\1/p" <<<"$1"; }
one() {
  local found
  found="$(grep -E "^\[sched-sync\] $1" <<<"$CLEAN" || true)"
  [[ -n "$found" && "$(wc -l <<<"$found")" -eq 1 ]] || fail "ligne [sched-sync] $2 absente ou répétée"
  echo "$found"
}

aps=$((CPUS - 1))
counter="$(one 'counter tasks=[0-9]+ rounds=[0-9]+ total=[0-9]+ expected=[0-9]+$' counter)"
pmm="$(one 'pmm tasks=[0-9]+ rounds=[0-9]+ allocations=[0-9]+ duplicates=[0-9]+ failures=[0-9]+ dirty=[0-9]+ free_before=[0-9]+ free_after=[0-9]+$' pmm)"
storm="$(one 'storm tasks=[0-9]+ rounds=[0-9]+ migrations=[0-9]+ moves=[0-9]+ deferred=[0-9]+ misplaced=[0-9]+ forced=[0-9]+ wakes=[0-9]+ on_cpu_waits=[0-9]+$' storm)"
pingpong="$(one 'pingpong ping_cpu=[0-9]+ pong_cpu=[0-9]+ rounds=[0-9]+ wakes=[0-9]+ retries=[0-9]+$' pingpong)"
race="$(one 'wake_race target=[0-9]+ wakers=3 wins=[0-9]+ losses=[0-9]+ target_runs=[0-9]+$' wake_race)"
locks="$(one 'locks acquisitions=[0-9]+,[0-9]+,[0-9]+,[0-9]+ contention=[0-9]+,[0-9]+,[0-9]+,[0-9]+ classes=pmm,run_queue,timer_queue,data switch_checks=[0-9]+$' locks)"
summary="$(one 'cpus=[0-9]+ ipi_sent=' synthèse)"

# a. Compteur : total recalculé à partir du nombre d'AP et de tours.
rounds="$(value "$counter" rounds)"
tasks="$(value "$counter" tasks)"
expected=$((aps * COUNTER_PER_AP * rounds))
[[ "$tasks" == $((aps * COUNTER_PER_AP)) ]] || fail "$tasks tâches de comptage, $((aps * COUNTER_PER_AP)) attendues"
[[ "$(value "$counter" total)" == "$expected" && "$(value "$counter" expected)" == "$expected" ]] \
  || fail "compteur $(value "$counter" total) sur $expected : mises à jour perdues"

# b. PMM : chaque allocation rendue, aucune page distribuée deux fois.
p_rounds="$(value "$pmm" rounds)"
[[ "$(value "$pmm" tasks)" == "$aps" && "$(value "$pmm" allocations)" == $((aps * p_rounds)) ]] \
  || fail "allocations PMM invalides: $pmm"
[[ "$(value "$pmm" duplicates)" == 0 && "$(value "$pmm" failures)" == 0 ]] || fail "page distribuée deux fois ou échec PMM: $pmm"
[[ "$(value "$pmm" dirty)" == 0 ]] || fail "page allouée non effacée: $pmm"
before="$(value "$pmm" free_before)"
[[ "$before" -gt 0 && "$(value "$pmm" free_after)" == "$before" ]] || fail "pages libres non restaurées: $pmm"

# c. Ping-pong : chaque tâche réveille l'autre à chaque tour.
[[ "$(value "$pingpong" ping_cpu)" == 1 && "$(value "$pingpong" pong_cpu)" == $((CPUS >= 3 ? 2 : 1)) ]] \
  || fail "CPU du ping-pong invalides: $pingpong"
[[ "$(value "$pingpong" wakes)" == $((2 * $(value "$pingpong" rounds))) ]] || fail "réveils du ping-pong invalides: $pingpong"

# d. Course au réveil : un seul gagnant, une seule reprise.
[[ "$(value "$race" wins)" == 1 && "$(value "$race" losses)" == 2 && "$(value "$race" target_runs)" == 1 ]] \
  || fail "course au réveil invalide: $race"

# e. Tempête de migrations : chaque tâche se bloque un tour sur trois et est
# réveillée ; dès 2 AP, elle change réellement de CPU, par déplacement d'une file
# ou par migration différée après son yield.
STORM_PER_AP=2
s_tasks="$(value "$storm" tasks)"
s_rounds="$(value "$storm" rounds)"
[[ "$s_tasks" == $((aps * STORM_PER_AP)) ]] || fail "$s_tasks tâches de tempête, $((aps * STORM_PER_AP)) attendues"
[[ "$(value "$storm" wakes)" == $((s_tasks * (s_rounds / 3))) ]] || fail "réveils de la tempête invalides: $storm"
if [[ "$aps" -ge 2 ]]; then
  [[ "$(value "$storm" migrations)" -gt 0 ]] || fail "aucune migration effective: $storm"
  [[ $(($(value "$storm" moves) + $(value "$storm" deferred))) -gt 0 ]] || fail "aucun déplacement ni migration différée: $storm"
fi

# Verrous : une prise du verrou Data par incrément ; contention réelle dès 2 AP.
IFS=, read -r -a acq <<<"$(list "$locks" acquisitions)"
IFS=, read -r -a cont <<<"$(list "$locks" contention)"
[[ "${acq[3]}" == "$expected" ]] || fail "${acq[3]} prises du verrou Data pour $expected incréments"
for index in 0 1 2; do
  [[ "${acq[index]}" -gt 0 ]] || fail "classe de verrou $index jamais prise: $locks"
done
[[ "$(value "$locks" switch_checks)" -gt 0 ]] || fail "aucun switch contrôlé"
if [[ "$CPUS" -ge 3 && "${cont[3]}" -eq 0 ]]; then
  fail "aucune contention sur le verrou partagé par $aps AP"
fi

sent="$(value "$summary" ipi_sent)"
received="$(value "$summary" ipi_received)"
coalesced="$(value "$summary" ipi_coalesced)"
if [[ -z "$sent" || -z "$received" || "$received" -lt 1 || "$received" -gt "$sent" \
  || "$coalesced" != $((sent - received)) ]]; then
  fail "comptes d'IPI incohérents: $summary"
fi
tasks_total=$((aps * (COUNTER_PER_AP + 1 + STORM_PER_AP) + 2 + 3))
if [[ "$(value "$summary" cpus)" != "$CPUS" || "$(value "$summary" idle_running_aps)" != "$aps" \
  || "$(value "$summary" running_max)" != 1 || "$(value "$summary" canaries_ok)" != "$tasks_total" ]]; then
  fail "synthèse invalide: $summary"
fi

order="$(grep -nE '^(ELYON_SCHED_SLEEP_READY|ELYON_SCHED_SYNC_READY|ELYON_KERNEL_READY_0_0_4)$' <<<"$CLEAN" | cut -d: -f2 | tr '\n' ' ')"
[[ "$order" == "ELYON_SCHED_SLEEP_READY ELYON_SCHED_SYNC_READY ELYON_KERNEL_READY_0_0_4 " ]] \
  || fail "marqueurs absents ou dans le désordre: $order (qemu rc=$RC)"

if grep -Eq '\[sched\] FAILED|\[KERNEL PANIC\]|\[EXCEPTION\]' <<<"$CLEAN"; then
  fail "erreur pendant la qualification"
fi

echo "[PASS] K004-10 synchronisation: $CPUS CPU, compteur $expected/$expected sous verrou (contention ${cont[3]}), PMM $((aps * p_rounds)) allocations sans doublon, ping-pong $(value "$pingpong" wakes) réveils, course au réveil 1 gagnant, tempête $(value "$storm" migrations) migrations ($(value "$storm" on_cpu_waits) attentes on_cpu), $(value "$locks" switch_checks) switchs contrôlés, aucune violation d'ordre"
