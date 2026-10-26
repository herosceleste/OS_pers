#!/usr/bin/env bash
set -euo pipefail

# K005-05 : une lecture du noyau par l'alias identité d'une page utilisateur
# lève une #PF (page absente) qui n'est pas reprise : arrêt fail-closed.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UEFI_TARGET="x86_64-unknown-uefi"
KERNEL_TARGET="x86_64-unknown-none"
TARGET_DIR="$ROOT/target-test-alias"
ESP="$ROOT/build/tests/alias/esp"
LOG="$ROOT/build/tests/alias/qemu.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-150}"
CPUS="${ELYON_CPUS:-4}"
# Code d'erreur attendu : page absente, lecture, mode superviseur.
PF_ALIAS_READ=0x0

rm -rf "$TARGET_DIR" "$ROOT/build/tests/alias"
mkdir -p "$ESP/EFI/BOOT" "$ESP/ELYON"

export CARGO_TARGET_DIR="$TARGET_DIR"
export ELYON_TEST_ALIAS=1
export ELYON_BUILD_ID="$(git -C "$ROOT/.." rev-parse --short=12 HEAD 2>/dev/null || echo unknown)-alias"

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
  echo "[FAIL] K005-05 alias: $*" >&2
  exit 1
}

grep -Fxq "ELYON_USER_PAGES_READY" <<<"$CLEAN" || fail "qualification K005-05 non atteinte"
injection="$(grep -E '^\[test\] injecting a kernel read of user frame 0x[0-9a-f]+ through the identity map$' <<<"$CLEAN" || true)"
[[ -n "$injection" ]] || fail "injection non atteinte"
frame="$(sed -nE 's/.* user frame (0x[0-9a-f]+) .*/\1/p' <<<"$injection")"
[[ -n "$frame" && "$frame" != 0x0000000000000000 ]] || fail "injection mal formée: $injection"

grep -Fxq "[EXCEPTION] vector=14 #PF page fault" <<<"$CLEAN" || fail "diagnostic #PF absent"
grep -Eq "^\[EXCEPTION\] error=$PF_ALIAS_READ rip=0x[0-9a-f]+ cs=0x8 " <<<"$CLEAN" \
  || fail "#PF sans la signature d'un alias absent (absente, lecture, superviseur)"
grep -Fxq "[EXCEPTION] CR2=$frame" <<<"$CLEAN" || fail "#PF sur une autre adresse que $frame"
if grep -Fq "[test] identity alias reachable" <<<"$CLEAN"; then
  fail "le noyau a lu la page utilisateur par l'alias identité"
fi
if [[ "$(grep -c '^\[EXCEPTION\] vector=' <<<"$CLEAN")" != 1 ]]; then
  fail "exceptions en cascade"
fi
grep -Fxq "[EXCEPTION] fail-closed halt" <<<"$CLEAN" || fail "arrêt fail-closed absent"
if grep -Fq "ELYON_KERNEL_READY_0_0_5" <<<"$CLEAN"; then
  fail "le noyau a continué après la #PF"
fi

echo "[PASS] K005-05 alias: lecture de la frame utilisateur $frame par l'alias identité refusée par #PF (error=$PF_ALIAS_READ), non reprise, arrêt fail-closed (qemu rc=$RC)"
