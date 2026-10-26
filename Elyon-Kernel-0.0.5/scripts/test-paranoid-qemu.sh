#!/usr/bin/env bash
set -euo pipefail

# K005-03 : entrée « paranoïaque » NMI/#DF/#MC, piles IST dédiées NMI et #MC.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Avec ELYON_CHECK_LOG, vérifie ce log existant (VMware) sans construire ni booter.
LOG="${ELYON_CHECK_LOG:-$ROOT/build/qemu-paranoid.log}"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-150}"
CPUS="${ELYON_CPUS:-4}"
# Paramètres attendus (elyon_core::paranoid), recalculés ici.
IST_DF=1
IST_NMI=2
IST_MC=3
ISTS_PER_CPU=3
CR4_MCE=$((1 << 6))

if [[ "$CPUS" -lt 2 ]]; then
  echo "[FAIL] K005-03 demande au moins 2 CPU (ELYON_CPUS=$CPUS)" >&2
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
  echo "[FAIL] K005-03: $*" >&2
  exit 1
}

grep -Fq "ELYON_RING3_SWAPGS_READY" <<<"$CLEAN" || {
  echo "[FAIL] régression K005-02 avant K005-03" >&2
  exit 1
}

if grep -Eq '^\[paranoid\] (FAILED|skipped)' <<<"$CLEAN"; then
  grep -E '^\[paranoid\] (FAILED|skipped)' <<<"$CLEAN" >&2
  fail "échec signalé par le noyau"
fi

aps=$((CPUS - 1))
value() { sed -nE "s/.* $2=([0-9]+)( .*|$)/\1/p" <<<"$1"; }

# Une ligne par AP, dans l'ordre : les trois événements (NMI en mode noyau,
# NMI et #MC dans la fenêtre swapgs/iretq) contrôlés, GS remplacé pour les
# deux de la fenêtre seulement, GS d'entrée retrouvé après chacun.
mapfile -t PROBES < <(grep -E '^\[paranoid\] cpu=[0-9]+ nmi_kernel=[01] nmi_window=[01] mc_window=[01] replaced=[0-9]+ restored=[0-9]+ gates_ok=[01] cr4_mce=[01]$' <<<"$CLEAN")
[[ "${#PROBES[@]}" == "$aps" ]] || fail "${#PROBES[@]} lignes de sonde, $aps attendues"
for index in "${!PROBES[@]}"; do
  line="${PROBES[$index]}"
  [[ "$(value "$line" cpu)" == $((index + 1)) ]] || fail "sonde hors ordre: $line"
  for event in nmi_kernel nmi_window mc_window; do
    [[ "$(value "$line" "$event")" == 1 ]] || fail "$event refusé: $line"
  done
  [[ "$(value "$line" replaced)" == 2 ]] || fail "GS remplacé hors de la fenêtre, ou fenêtre manquée: $line"
  [[ "$(value "$line" restored)" == 3 ]] || fail "GS d'entrée non restauré avant iretq: $line"
  [[ "$(value "$line" gates_ok)" == 1 ]] || fail "portes NMI/#DF/#MC invalides sur l'AP: $line"
  [[ "$(value "$line" cr4_mce)" == 1 ]] || fail "CR4.MCE inactif sur l'AP: $line"
done

slots=$((CPUS * ISTS_PER_CPU))
expected_summary="$(printf '[paranoid] entry aps=%d cpus=%d ist_df=%d ist_nmi=%d ist_mc=%d slots_ok=%d/%d bsp_gates_ok=1 bsp_cr4_mce=1' \
  "$aps" "$CPUS" "$IST_DF" "$IST_NMI" "$IST_MC" "$slots" "$slots")"
summary="$(grep -E '^\[paranoid\] entry ' <<<"$CLEAN" || true)"
[[ "$summary" == "$expected_summary" ]] || fail "synthèse inattendue: $summary (attendu: $expected_summary)"

# Contrôle indépendant de CR4.MCE sur le BSP : CR4 relu en fin de boot.
cr4="$(sed -nE 's/^\[cpu\] CR0=0x[0-9a-f]+ CR3=0x[0-9a-f]+ CR4=(0x[0-9a-f]+)$/\1/p' <<<"$CLEAN" | tail -1)"
[[ -n "$cr4" ]] || fail "ligne [cpu] CR4 absente"
(( (cr4 & CR4_MCE) != 0 )) || fail "CR4.MCE inactif en fin de boot: $cr4"

order="$(grep -nE '^(ELYON_RING3_SWAPGS_READY|ELYON_PARANOID_ENTRY_READY|ELYON_KERNEL_READY_0_0_5)$' <<<"$CLEAN" | cut -d: -f2 | tr '\n' ' ')"
[[ "$order" == "ELYON_RING3_SWAPGS_READY ELYON_PARANOID_ENTRY_READY ELYON_KERNEL_READY_0_0_5 " ]] \
  || fail "marqueurs absents ou dans le désordre: $order (qemu rc=$RC)"

if grep -Eq '\[sched\] FAILED|\[KERNEL PANIC\]|\[EXCEPTION\]|\[halt\]' <<<"$CLEAN"; then
  fail "erreur pendant la qualification"
fi

echo "[PASS] K005-03 paranoid: sur $aps AP, NMI en mode noyau puis NMI et #MC dans la fenêtre swapgs/iretq servis avec le GS noyau sur leur IST dédiée, GS d'entrée restauré ; $slots piles IST disjointes (#DF=$IST_DF, NMI=$IST_NMI, #MC=$IST_MC) ; CR4.MCE actif ($cr4)"
