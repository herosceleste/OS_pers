#!/usr/bin/env bash
set -euo pipefail

# K005-04 : un accès direct du noyau à une page utilisateur, hors de la copie
# validée, lève une #PF de signature SMAP qui n'est pas reprise : arrêt
# fail-closed, sans lire la page.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UEFI_TARGET="x86_64-unknown-uefi"
KERNEL_TARGET="x86_64-unknown-none"
TARGET_DIR="$ROOT/target-test-smap"
ESP="$ROOT/build/tests/smap/esp"
LOG="$ROOT/build/tests/smap/qemu.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-150}"
CPUS="${ELYON_CPUS:-4}"
# Code d'erreur attendu : page présente, lecture, mode superviseur.
PF_SMAP_READ=0x1

rm -rf "$TARGET_DIR" "$ROOT/build/tests/smap"
mkdir -p "$ESP/EFI/BOOT" "$ESP/ELYON"

export CARGO_TARGET_DIR="$TARGET_DIR"
export ELYON_TEST_SMAP=1
export ELYON_BUILD_ID="$(git -C "$ROOT/.." rev-parse --short=12 HEAD 2>/dev/null || echo unknown)-smap"

cargo build \
  --locked \
  --manifest-path "$ROOT/Cargo.toml" \
  -p elyon-kernel \
  --target "$KERNEL_TARGET" \
  --release

cargo build \
  --locked \
  --manifest-path "$ROOT/Cargo.toml" \
  -p elyon-loader \
  --target "$UEFI_TARGET" \
  --release

cp "$TARGET_DIR/$UEFI_TARGET/release/elyon-loader.efi" "$ESP/EFI/BOOT/BOOTX64.EFI"
cp "$TARGET_DIR/$KERNEL_TARGET/release/elyon-kernel" "$ESP/ELYON/KERNEL.ELF"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_CPUS="$CPUS" ELYON_ESP="$ESP" bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"
CLEAN="$(tr -d '\r' <"$LOG")"
fail() {
  echo "[FAIL] K005-04 SMAP: $*" >&2
  exit 1
}

grep -Fxq "ELYON_USER_ACCESS_READY" <<<"$CLEAN" || fail "qualification K005-04 non atteinte"
injection="$(grep -E '^\[test\] injecting a direct kernel read of user page ' <<<"$CLEAN" || true)"
[[ -n "$injection" ]] || fail "injection non atteinte"
page="$(sed -nE 's/.* user page (0x[0-9a-f]+)$/\1/p' <<<"$injection")"
[[ -n "$page" ]] || fail "injection mal formée: $injection"

grep -Fxq "[EXCEPTION] vector=14 #PF page fault" <<<"$CLEAN" || fail "diagnostic #PF absent"
grep -Eq "^\[EXCEPTION\] error=$PF_SMAP_READ rip=0x[0-9a-f]+ cs=0x8 " <<<"$CLEAN" \
  || fail "#PF sans la signature SMAP (présente, lecture, superviseur)"
grep -Fxq "[EXCEPTION] CR2=$page" <<<"$CLEAN" || fail "#PF sur une autre adresse que $page"
if grep -Fq "[test] SMAP bypassed" <<<"$CLEAN"; then
  fail "le noyau a lu la page utilisateur"
fi
if [[ "$(grep -c '^\[EXCEPTION\] vector=' <<<"$CLEAN")" != 1 ]]; then
  fail "exceptions en cascade"
fi
grep -Fxq "[EXCEPTION] fail-closed halt" <<<"$CLEAN" || fail "arrêt fail-closed absent"
if grep -Fq "ELYON_KERNEL_READY_0_0_5" <<<"$CLEAN"; then
  fail "le noyau a continué après la #PF"
fi

echo "[PASS] K005-04 SMAP: lecture directe de la page utilisateur $page refusée par #PF (error=$PF_SMAP_READ), non reprise, arrêt fail-closed (qemu rc=$RC)"
