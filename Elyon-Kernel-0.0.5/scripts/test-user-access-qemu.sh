#!/usr/bin/env bash
set -euo pipefail

# K005-04 : SMEP et SMAP actifs ; copie validée entre espace utilisateur et noyau.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Avec ELYON_CHECK_LOG, vérifie ce log existant (VMware) sans construire ni booter.
LOG="${ELYON_CHECK_LOG:-$ROOT/build/qemu-user-access.log}"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-150}"
CPUS="${ELYON_CPUS:-4}"
# Paramètres attendus (elyon_core::user_access), recalculés ici.
CR0_WP=$((1 << 16))
CR4_UMIP=$((1 << 11))
CR4_SMEP=$((1 << 20))
CR4_SMAP=$((1 << 21))
COPY_FAULTS=3
REJECTS=5
MAX_COPY=$((1 << 20))
RING3_ENTRIES=20

if [[ "$CPUS" -lt 2 ]]; then
  echo "[FAIL] K005-04 demande au moins 2 CPU (ELYON_CPUS=$CPUS)" >&2
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
  echo "[FAIL] K005-04: $*" >&2
  exit 1
}

grep -Fq "ELYON_PARANOID_ENTRY_READY" <<<"$CLEAN" || {
  echo "[FAIL] régression K005-03 avant K005-04" >&2
  exit 1
}

if grep -Eq '^\[(uaccess|cpu)\] (FAILED|skipped)' <<<"$CLEAN"; then
  grep -E '^\[(uaccess|cpu)\] (FAILED|skipped)' <<<"$CLEAN" >&2
  fail "échec signalé par le noyau"
fi

features="$(grep -E '^\[cpu\] smep=[01] smap=[01] umip=[01]$' <<<"$CLEAN" || true)"
[[ -n "$features" ]] || fail "ligne CPUID SMEP/SMAP absente"
value() { sed -nE "s/.* $2=([0-9]+)( .*|$)/\1/p" <<<"$1"; }
umip="$(sed -nE 's/.* umip=([01])$/\1/p' <<<"$features")"
[[ "$features" == "[cpu] smep=1 smap=1 umip=$umip" ]] || fail "SMEP ou SMAP non pris en charge: $features"

# Une ligne par CPU, BSP compris, dans l'ordre : CR4 et CR0.WP actifs, fautes
# SMAP et SMEP de la bonne signature, aller-retour de copie, trois fautes de
# copie reprises (page en lecture seule, page absente, débordement), cinq
# plages refusées sans accès, AC nul dans le gestionnaire et après la sonde.
mapfile -t PROBES < <(grep -E '^\[uaccess\] cpu=[0-9]+ cr4_ok=[01] cr0_wp=[01] smap=[01] smep=[01] copy=[01] ro_refused=[01] unmapped_refused=[01] partial=[01] faults=[0-9]+ rejects=[0-9]+ handler_ac=[0-9]+ ac_clear=[01]$' <<<"$CLEAN")
[[ "${#PROBES[@]}" == "$CPUS" ]] || fail "${#PROBES[@]} lignes de sonde, $CPUS attendues"
for index in "${!PROBES[@]}"; do
  line="${PROBES[$index]}"
  [[ "$(value "$line" cpu)" == "$index" ]] || fail "sonde hors ordre: $line"
  for flag in cr4_ok cr0_wp smap smep copy ro_refused unmapped_refused partial ac_clear; do
    [[ "$(value "$line" "$flag")" == 1 ]] || fail "$flag refusé: $line"
  done
  [[ "$(value "$line" faults)" == "$COPY_FAULTS" ]] || fail "fautes de copie inattendues: $line"
  [[ "$(value "$line" rejects)" == "$REJECTS" ]] || fail "plage invalide acceptée: $line"
  [[ "$(value "$line" handler_ac)" == 0 ]] || fail "gestionnaire de faute avec AC levé: $line"
done

# Ring 3 : la sonde K005-02 lève AC ; chaque entrée dans le noyau l'efface.
aps=$((CPUS - 1))
mapfile -t RING3 < <(grep -E '^\[uaccess\] ring3 cpu=[0-9]+ user_ac=[0-9]+ entries=[0-9]+ ac_leaks=[0-9]+$' <<<"$CLEAN")
[[ "${#RING3[@]}" == "$aps" ]] || fail "${#RING3[@]} lignes ring 3, $aps attendues"
entries=0
for index in "${!RING3[@]}"; do
  line="${RING3[$index]}"
  [[ "$(value "$line" cpu)" == $((index + 1)) ]] || fail "ligne ring 3 hors ordre: $line"
  [[ "$(value "$line" user_ac)" == 1 ]] || fail "le ring 3 n'a pas levé AC: $line"
  [[ "$(value "$line" ac_leaks)" == 0 ]] || fail "noyau entré depuis le ring 3 avec AC levé: $line"
  (( $(value "$line" entries) >= RING3_ENTRIES )) || fail "trop peu d'entrées depuis le ring 3: $line"
  entries=$((entries + $(value "$line" entries)))
done

expected_summary="$(printf '[uaccess] protection cpus=%d smep=1 smap=1 umip=%d pages_ok=1 ring3_entries=%d max_copy=%d' \
  "$CPUS" "$umip" "$entries" "$MAX_COPY")"
summary="$(grep -E '^\[uaccess\] protection ' <<<"$CLEAN" || true)"
[[ "$summary" == "$expected_summary" ]] || fail "synthèse inattendue: $summary (attendu: $expected_summary)"

# Contrôle indépendant sur le BSP : CR0 et CR4 relus en fin de boot.
regs="$(sed -nE 's/^\[cpu\] CR0=(0x[0-9a-f]+) CR3=0x[0-9a-f]+ CR4=(0x[0-9a-f]+)$/\1 \2/p' <<<"$CLEAN" | tail -1)"
[[ -n "$regs" ]] || fail "ligne [cpu] CR0/CR4 absente"
read -r cr0 cr4 <<<"$regs"
(( (cr0 & CR0_WP) != 0 )) || fail "CR0.WP inactif en fin de boot: $cr0"
(( (cr4 & CR4_SMEP) != 0 && (cr4 & CR4_SMAP) != 0 )) || fail "SMEP ou SMAP inactif en fin de boot: $cr4"
if [[ "$umip" == 1 ]]; then
  (( (cr4 & CR4_UMIP) != 0 )) || fail "UMIP pris en charge mais inactif: $cr4"
fi

order="$(grep -nE '^(ELYON_PARANOID_ENTRY_READY|ELYON_USER_ACCESS_READY|ELYON_KERNEL_READY_0_0_5)$' <<<"$CLEAN" | cut -d: -f2 | tr '\n' ' ')"
[[ "$order" == "ELYON_PARANOID_ENTRY_READY ELYON_USER_ACCESS_READY ELYON_KERNEL_READY_0_0_5 " ]] \
  || fail "marqueurs absents ou dans le désordre: $order (qemu rc=$RC)"

if grep -Eq '\[sched\] FAILED|\[KERNEL PANIC\]|\[EXCEPTION\]|\[halt\]' <<<"$CLEAN"; then
  fail "erreur pendant la qualification"
fi

echo "[PASS] K005-04 user access: sur $CPUS CPU, SMEP et SMAP actifs (UMIP=$umip, CR0.WP), lecture et exécution directes d'une page utilisateur refusées par #PF, copies validées aller-retour, $COPY_FAULTS fautes de copie reprises et $REJECTS plages refusées par CPU ; AC effacé à $entries entrées depuis le ring 3 (CR0=$cr0 CR4=$cr4)"
