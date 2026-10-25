#!/usr/bin/env bash
set -euo pipefail

# K004-08 : IPI de rescheduling, réveil à distance et migration par affinité.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Avec ELYON_CHECK_LOG, vérifie ce log existant (VMware) sans construire ni booter.
LOG="${ELYON_CHECK_LOG:-$ROOT/build/qemu-sched-ipi.log}"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-70}"
CPUS="${ELYON_CPUS:-4}"

if [[ "$CPUS" -lt 2 ]]; then
  echo "[FAIL] K004-08 demande au moins 2 CPU (ELYON_CPUS=$CPUS)" >&2
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

grep -Fq "ELYON_SCHED_PERCPU_READY" <<<"$CLEAN" || {
  echo "[FAIL] régression K004-07 avant K004-08" >&2
  exit 1
}

if grep -Eq '^\[sched-ipi\] (FAILED|trace_error|skipped)' <<<"$CLEAN"; then
  echo "[FAIL] K004-08: échec signalé par le noyau" >&2
  grep -E '^\[sched-ipi\] (FAILED|trace_error|skipped)' <<<"$CLEAN" >&2
  exit 1
fi

line() { grep -E "^\[sched-ipi\] $1" <<<"$CLEAN" || true; }
value() { sed -nE "s/.* $2=([0-9]+)( .*|$)/\1/p" <<<"$1"; }

spawn="$(line 'remote_spawn task=[0-9]+ cpu=[0-9]+ runs=1 ipi=1$')"
block="$(line 'block_wake task=[0-9]+ cpu=[0-9]+ runs=2 blocked_then_woken_by_cpu=0$')"
migrate="$(line 'migrate task=[0-9]+ from=[0-9]+ to=[0-9]+ runs=1$')"
summary="$(line 'cpus=')"
for name in spawn block migrate summary; do
  if [[ -z "${!name}" || "$(wc -l <<<"${!name}")" -ne 1 ]]; then
    echo "[FAIL] K004-08: ligne [sched-ipi] $name absente ou répétée" >&2
    exit 1
  fi
done

# Vérification indépendante du noyau : placement attendu de chaque scénario.
expected_block_cpu=$((CPUS >= 3 ? 2 : 1))
if [[ "$(value "$spawn" cpu)" != 1 ]]; then
  echo "[FAIL] K004-08: création distante sur le CPU $(value "$spawn" cpu), attendu 1" >&2
  exit 1
fi
if [[ "$(value "$block" cpu)" != "$expected_block_cpu" ]]; then
  echo "[FAIL] K004-08: tâche bloquée sur le CPU $(value "$block" cpu), attendu $expected_block_cpu" >&2
  exit 1
fi
if [[ "$(value "$migrate" from)" != 1 || "$(value "$migrate" to)" != 0 ]]; then
  echo "[FAIL] K004-08: migration invalide: $migrate" >&2
  exit 1
fi

# Trois tâches distinctes, jamais réutilisées depuis K004-07.
declare -A seen=()
while read -r id; do
  seen[$id]=1
done < <(grep -E '^\[percpu-sched\] cpu=' <<<"$CLEAN" | sed -nE 's/.* idle=([0-9]+) tasks=([0-9]+),([0-9]+) .*/\1\n\2\n\3/p')
for id in "$(value "$spawn" task)" "$(value "$block" task)" "$(value "$migrate" task)"; do
  if [[ -n "${seen[$id]:-}" ]]; then
    echo "[FAIL] K004-08: tâche $id déjà utilisée" >&2
    exit 1
  fi
  seen[$id]=1
done

aps=$((CPUS - 1))
sent="$(value "$summary" ipi_sent)"
received="$(value "$summary" ipi_received)"
coalesced="$(value "$summary" ipi_coalesced)"
# Un IPI envoyé pendant qu'un autre attend dans le LAPIC est absorbé par lui :
# envois exacts, chaque réception correspond à un envoi, écart déclaré cohérent.
if [[ "$sent" != 3 || -z "$received" || "$received" -lt 1 || "$received" -gt "$sent" \
  || "$coalesced" != $((sent - received)) ]]; then
  echo "[FAIL] K004-08: comptes d'IPI incohérents: $summary" >&2
  exit 1
fi
if [[ "$(value "$summary" cpus)" != "$CPUS" || "$(value "$summary" placement_ok)" != 1 \
  || "$(value "$summary" blocks)" != 1 || "$(value "$summary" wakes)" != 1 \
  || "$(value "$summary" migrations)" != 1 || -z "$(value "$summary" contention)" \
  || "$(value "$summary" idle_running_aps)" != "$aps" || "$(value "$summary" running_max)" != 1 \
  || "$(value "$summary" canaries_ok)" != 3 ]]; then
  echo "[FAIL] K004-08: synthèse invalide: $summary" >&2
  exit 1
fi

# Ordre des marqueurs : K004-07, puis K004-08, puis boot nominal.
order="$(grep -nE '^(ELYON_SCHED_PERCPU_READY|ELYON_SCHED_IPI_READY|ELYON_KERNEL_READY_0_0_5)$' <<<"$CLEAN" | cut -d: -f2 | tr '\n' ' ')"
if [[ "$order" != "ELYON_SCHED_PERCPU_READY ELYON_SCHED_IPI_READY ELYON_KERNEL_READY_0_0_5 " ]]; then
  echo "[FAIL] K004-08: marqueurs absents ou dans le désordre: $order (qemu rc=$RC)" >&2
  exit 1
fi

if grep -Eq '\[sched\] FAILED|\[KERNEL PANIC\]|\[EXCEPTION\]' <<<"$CLEAN"; then
  echo "[FAIL] erreur pendant la qualification K004-08" >&2
  exit 1
fi

echo "[PASS] K004-08 rescheduling à distance: $CPUS CPU, création sur un AP en idle par IPI, blocage puis réveil depuis le BSP, migration AP 1 -> CPU 0, IPI envoyés=$sent reçus=$received (fusionnés=$coalesced), $aps AP revenus en idle"
