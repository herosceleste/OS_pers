#!/usr/bin/env bash
set -euo pipefail

# K005-02 : swapgs à chaque entrée depuis le ring 3, sonde ring 3 sur chaque AP.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Avec ELYON_CHECK_LOG, vérifie ce log existant (VMware) sans construire ni booter.
LOG="${ELYON_CHECK_LOG:-$ROOT/build/qemu-ring3.log}"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-120}"
CPUS="${ELYON_CPUS:-4}"
# Paramètres attendus (user_probe.rs, elyon_core::ring3), recalculés ici.
TARGET_ENTRIES=20
USER_BASE=$((1 << 39))
CR4_FSGSBASE=$((1 << 16))

if [[ "$CPUS" -lt 2 ]]; then
  echo "[FAIL] K005-02 demande au moins 2 CPU (ELYON_CPUS=$CPUS)" >&2
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
  echo "[FAIL] K005-02: $*" >&2
  exit 1
}

grep -Fq "ELYON_SEGMENTS_READY" <<<"$CLEAN" || {
  echo "[FAIL] régression K005-01 avant K005-02" >&2
  exit 1
}

if grep -Eq '^\[ring3\] (FAILED|skipped)' <<<"$CLEAN"; then
  grep -E '^\[ring3\] (FAILED|skipped)' <<<"$CLEAN" >&2
  fail "échec signalé par le noyau"
fi

aps=$((CPUS - 1))
value() { sed -nE "s/.* $2=([0-9]+)( .*|$)/\1/p" <<<"$1"; }

# Une ligne par AP, dans l'ordre, chacune avec au moins TARGET_ENTRIES entrées
# depuis le ring 3, aucune erreur de GS, une sortie dont les contrôles passent.
mapfile -t PROBES < <(grep -E '^\[ring3\] cpu=[0-9]+ user_entries=[0-9]+ gs_failures=[0-9]+ exits=[0-9]+ exit_gs_ok=[0-9]+ fsgsbase_on=[0-9]+$' <<<"$CLEAN")
[[ "${#PROBES[@]}" == "$aps" ]] || fail "${#PROBES[@]} lignes de sonde, $aps attendues"
total_entries=0
for index in "${!PROBES[@]}"; do
  line="${PROBES[$index]}"
  [[ "$(value "$line" cpu)" == $((index + 1)) ]] || fail "sonde hors ordre: $line"
  entries="$(value "$line" user_entries)"
  [[ "$entries" -ge "$TARGET_ENTRIES" ]] || fail "moins de $TARGET_ENTRIES entrées depuis le ring 3: $line"
  [[ "$(value "$line" gs_failures)" == 0 ]] || fail "noyau entré avec le GS utilisateur: $line"
  [[ "$(value "$line" exits)" == 1 && "$(value "$line" exit_gs_ok)" == 1 ]] || fail "sortie de sonde invalide: $line"
  [[ "$(value "$line" fsgsbase_on)" == 0 ]] || fail "CR4.FSGSBASE actif pendant la sonde: $line"
  total_entries=$((total_entries + entries))
done

expected_summary="$(printf '[ring3] swapgs aps=%d target_entries=%d cr4_fsgsbase=0 user_base=%#x' "$aps" "$TARGET_ENTRIES" "$USER_BASE")"
summary="$(grep -E '^\[ring3\] swapgs ' <<<"$CLEAN" || true)"
[[ "$summary" == "$expected_summary" ]] || fail "synthèse inattendue: $summary (attendu: $expected_summary)"

# Contrôle indépendant de la politique FSGSBASE : CR4 relu en fin de boot.
cr4="$(sed -nE 's/^\[cpu\] CR0=0x[0-9a-f]+ CR3=0x[0-9a-f]+ CR4=(0x[0-9a-f]+)$/\1/p' <<<"$CLEAN" | tail -1)"
[[ -n "$cr4" ]] || fail "ligne [cpu] CR4 absente"
(( (cr4 & CR4_FSGSBASE) == 0 )) || fail "CR4.FSGSBASE actif en fin de boot: $cr4"

order="$(grep -nE '^(ELYON_SEGMENTS_READY|ELYON_RING3_SWAPGS_READY|ELYON_KERNEL_READY_0_0_5)$' <<<"$CLEAN" | cut -d: -f2 | tr '\n' ' ')"
[[ "$order" == "ELYON_SEGMENTS_READY ELYON_RING3_SWAPGS_READY ELYON_KERNEL_READY_0_0_5 " ]] \
  || fail "marqueurs absents ou dans le désordre: $order (qemu rc=$RC)"

if grep -Eq '\[sched\] FAILED|\[KERNEL PANIC\]|\[EXCEPTION\]|\[halt\]' <<<"$CLEAN"; then
  fail "erreur pendant la qualification"
fi

echo "[PASS] K005-02 swapgs: code exécuté en ring 3 sur $aps AP, $total_entries entrées depuis le ring 3 avec GS_BASE noyau et GS utilisateur préservé, $aps sorties par int 0x81 contrôlées, CR4.FSGSBASE inactif ($cr4)"
