#!/usr/bin/env bash
set -euo pipefail

# K005-01 : segments utilisateur dans la GDT de chaque CPU et RSP0 par tâche.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Avec ELYON_CHECK_LOG, vérifie ce log existant (VMware) sans construire ni booter.
LOG="${ELYON_CHECK_LOG:-$ROOT/build/qemu-segments.log}"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-120}"
CPUS="${ELYON_CPUS:-4}"
# Disposition et paramètres attendus (elyon_core::segments, sched.rs), recalculés ici.
KERNEL_CODE=0x08
KERNEL_DATA=0x10
USER_DATA=0x18
USER_CODE=0x20
TSS=0x28
TASKS_PER_AP=4
ROUNDS=50

if [[ "$CPUS" -lt 2 ]]; then
  echo "[FAIL] K005-01 demande au moins 2 CPU (ELYON_CPUS=$CPUS)" >&2
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
  echo "[FAIL] K005-01: $*" >&2
  exit 1
}

grep -Fq "ELYON_SCHED_STRESS_READY" <<<"$CLEAN" || {
  echo "[FAIL] régression K004-11 avant K005-01" >&2
  exit 1
}

if grep -Eq '^\[segments\] (FAILED|skipped)' <<<"$CLEAN"; then
  grep -E '^\[segments\] (FAILED|skipped)' <<<"$CLEAN" >&2
  fail "échec signalé par le noyau"
fi

one() {
  local found
  found="$(grep -E "^\[segments\] $1" <<<"$CLEAN" || true)"
  [[ -n "$found" && "$(wc -l <<<"$found")" -eq 1 ]] || fail "ligne [segments] $2 absente ou répétée"
  echo "$found"
}

aps=$((CPUS - 1))
tasks=$((aps * TASKS_PER_AP))
# STAR : bits 47:32 pour syscall (code noyau), 63:48 pour sysret (données
# utilisateur - 8, RPL 3) ; sysret prend SS = base + 8 et CS = base + 16.
(( KERNEL_DATA == KERNEL_CODE + 8 && USER_CODE == USER_DATA + 8 )) || fail "disposition attendue incohérente"
star=$(( (((USER_DATA - 8) | 3) << 48) | (KERNEL_CODE << 32) ))
all_cpus=$(( (1 << CPUS) - 1 ))
expected_layout="$(printf 'layout kernel_code=%#x kernel_data=%#x user_data=%#x user_code=%#x tss=%#x star=0x%016x gdt_cpus=%#x' \
  "$KERNEL_CODE" "$KERNEL_DATA" $((USER_DATA | 3)) $((USER_CODE | 3)) "$TSS" "$star" "$all_cpus")"
layout="$(one 'layout ' disposition)"
[[ "$layout" == "[segments] $expected_layout" ]] || fail "disposition ou STAR inattendue: $layout (attendu: $expected_layout)"

expected_rsp0="rsp0 cpus=$CPUS tasks=$tasks rounds=$ROUNDS checks=$((tasks * ROUNDS)) mismatches=0 gdt_failures=0 distinct=$tasks bsp_rsp0=0x0 running_max=1"
rsp0="$(one 'rsp0 ' RSP0)"
[[ "$rsp0" == "[segments] $expected_rsp0" ]] || fail "RSP0 invalide: $rsp0 (attendu: $expected_rsp0)"

# Chaque AP a chargé le TSS au sélecteur de la nouvelle disposition.
tr_hex="$(printf '%#x' "$TSS")"
online="$(grep -cE "^\[smp\] AP online cpu=[1-9][0-9]* apic_id=[0-9]+ selector=0x8 tr=$tr_hex$" <<<"$CLEAN" || true)"
[[ "$online" == "$aps" ]] || fail "$online AP avec tr=$tr_hex, $aps attendus"

order="$(grep -nE '^(ELYON_SCHED_STRESS_READY|ELYON_SEGMENTS_READY|ELYON_KERNEL_READY_0_0_5)$' <<<"$CLEAN" | cut -d: -f2 | tr '\n' ' ')"
[[ "$order" == "ELYON_SCHED_STRESS_READY ELYON_SEGMENTS_READY ELYON_KERNEL_READY_0_0_5 " ]] \
  || fail "marqueurs absents ou dans le désordre: $order (qemu rc=$RC)"

if grep -Eq '\[sched\] FAILED|\[KERNEL PANIC\]|\[EXCEPTION\]|\[halt\]' <<<"$CLEAN"; then
  fail "erreur pendant la qualification"
fi

echo "[PASS] K005-01 segments: GDT vérifiée sur $CPUS CPU (code/données noyau, données/code utilisateur DPL 3, TSS $tr_hex), STAR=0x$(printf '%016x' "$star"), RSP0 = sommet de la pile de la tâche à chacun des $((tasks * ROUNDS)) contrôles ($tasks tâches distinctes), 0 hors tâche"
