#!/usr/bin/env bash
set -euo pipefail

# K005-05 : pages utilisateur prises par allocate_owned, bit U/S strict,
# jamais d'accès noyau par l'identity map.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Avec ELYON_CHECK_LOG, vérifie ce log existant (VMware) sans construire ni booter.
LOG="${ELYON_CHECK_LOG:-$ROOT/build/qemu-user-pages.log}"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-150}"
CPUS="${ELYON_CPUS:-4}"
# Paramètres attendus, recalculés ici.
EFER_NXE=$((1 << 11))
PERMANENT_PAGES=4
SHOOTDOWN_IPIS=2

if [[ "$CPUS" -lt 2 ]]; then
  echo "[FAIL] K005-05 demande au moins 2 CPU (ELYON_CPUS=$CPUS)" >&2
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
  echo "[FAIL] K005-05: $*" >&2
  exit 1
}

grep -Fq "ELYON_USER_ACCESS_READY" <<<"$CLEAN" || {
  echo "[FAIL] régression K005-04 avant K005-05" >&2
  exit 1
}

if grep -Eq '^\[(usermem|user-memory|cpu)\] (FAILED|skipped)' <<<"$CLEAN"; then
  grep -E '^\[(usermem|user-memory|cpu)\] (FAILED|skipped)' <<<"$CLEAN" >&2
  fail "échec signalé par le noyau"
fi

# Contrôle indépendant sur le BSP : EFER relu après l'activation de NX.
efer="$(sed -nE 's/^\[cpu\] nx=1 efer=(0x[0-9a-f]+)$/\1/p' <<<"$CLEAN")"
[[ -n "$efer" ]] || fail "ligne NX absente"
(( (efer & EFER_NXE) != 0 )) || fail "EFER.NXE inactif sur le BSP: $efer"

value() { sed -nE "s/.* $2=([0-9]+)( .*|$)/\1/p" <<<"$1"; }

# Une ligne par CPU, BSP compris, dans l'ordre : NXE, alias des quatre pages
# permanentes en faute, cycle de vie complet ; sur un AP, au moins deux IPI de
# shootdown reçues (création puis libération de la page).
mapfile -t PROBES < <(grep -E '^\[usermem\] cpu=[0-9]+ nxe=[01] alias_faults=[0-9]+ warm=[01] alias_shot=[01] user=[01] user_shot=[01] alias_back=[01] ipis=[0-9]+$' <<<"$CLEAN")
[[ "${#PROBES[@]}" == "$CPUS" ]] || fail "${#PROBES[@]} lignes de sonde, $CPUS attendues"
for index in "${!PROBES[@]}"; do
  line="${PROBES[$index]}"
  [[ "$(value "$line" cpu)" == "$index" ]] || fail "sonde hors ordre: $line"
  for flag in nxe warm alias_shot user user_shot alias_back; do
    [[ "$(value "$line" "$flag")" == 1 ]] || fail "$flag refusé: $line"
  done
  [[ "$(value "$line" alias_faults)" == "$PERMANENT_PAGES" ]] || fail "alias identité d'une page utilisateur atteignable: $line"
  if [[ "$index" != 0 ]]; then
    (( $(value "$line" ipis) >= SHOOTDOWN_IPIS )) || fail "IPI de shootdown manquante: $line"
  fi
done

summary="$(grep -E '^\[usermem\] pages ' <<<"$CLEAN" || true)"
[[ "$summary" =~ ^\[usermem\]\ pages\ cpus=$CPUS\ user_pages=$PERMANENT_PAGES/$PERMANENT_PAGES\ owned=$PERMANENT_PAGES/$PERMANENT_PAGES\ kernel_identity_user=0\ split_tables=([0-9]+)\ pmm_balanced=1$ ]] \
  || fail "synthèse inattendue: $summary"
split="${BASH_REMATCH[1]}"
(( split >= 1 )) || fail "aucune grande page du mappage identité découpée: $summary"

order="$(grep -nE '^(ELYON_USER_ACCESS_READY|ELYON_USER_PAGES_READY|ELYON_KERNEL_READY_0_0_5)$' <<<"$CLEAN" | cut -d: -f2 | tr '\n' ' ')"
[[ "$order" == "ELYON_USER_ACCESS_READY ELYON_USER_PAGES_READY ELYON_KERNEL_READY_0_0_5 " ]] \
  || fail "marqueurs absents ou dans le désordre: $order (qemu rc=$RC)"

if grep -Eq '\[sched\] FAILED|\[KERNEL PANIC\]|\[EXCEPTION\]|\[halt\]' <<<"$CLEAN"; then
  fail "erreur pendant la qualification"
fi

echo "[PASS] K005-05 user pages: $PERMANENT_PAGES pages utilisateur prises par allocate_owned, droits exacts (code lecture seule, données NX), sans alias identité ; sur $CPUS CPU, alias en faute, page lue par la copie puis retirée par shootdown, alias rendu ; PMM équilibré, $split table(s) de découpe sans U/S (EFER=$efer)"
