#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UEFI_TARGET="x86_64-unknown-uefi"
KERNEL_TARGET="x86_64-unknown-none"
TARGET_DIR="$ROOT/target-test-user-iret"
ESP="$ROOT/build/tests/user-iret/esp"
LOG="$ROOT/build/tests/user-iret/qemu.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-20}"

rm -rf "$TARGET_DIR" "$ROOT/build/tests/user-iret"
mkdir -p "$ESP/EFI/BOOT" "$ESP/ELYON"

export CARGO_TARGET_DIR="$TARGET_DIR"
export ELYON_TEST_USER_IRET=1
export ELYON_BUILD_ID="$(git -C "$ROOT/.." rev-parse --short=12 HEAD 2>/dev/null || echo unknown)-user-iret"

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
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_ESP="$ESP" bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"
CLEAN="$(tr -d '\r' <"$LOG")"
fail() {
  echo "[FAIL] K005-03 iretq: $*" >&2
  exit 1
}

# K005-03 : un `iretq` vers le ring 3 refusé lève #GP après `swapgs`, CS noyau.
# Le stub d'exception doit reconnaître l'unique `iretq` de sortie et refaire
# `swapgs` : sans cela, le diagnostic lirait la zone per-CPU via le GS
# utilisateur (adresse jamais mappée), d'où #PF en cascade puis #DF.
injection="$(grep -E '^\[test\] injecting #GP on the user iretq ' <<<"$CLEAN" || true)"
[[ -n "$injection" ]] || fail "injection non atteinte"
user_gs="$(sed -nE 's/.* user_gs=(0x[0-9a-f]+) .*/\1/p' <<<"$injection")"
iret="$(sed -nE 's/.* iret=(0x[0-9a-f]+)$/\1/p' <<<"$injection")"
[[ -n "$user_gs" && -n "$iret" ]] || fail "injection mal formée: $injection"

grep -Fxq "[EXCEPTION] vector=13 #GP general protection" <<<"$CLEAN" || fail "diagnostic #GP absent"
grep -Eq "^\[EXCEPTION\] error=0x[0-9a-f]+ rip=$iret cs=0x8 " <<<"$CLEAN" \
  || fail "#GP hors de l'iretq de sortie $iret"
grep -Eq "^\[EXCEPTION\] user iret fault gs_base=0x[0-9a-f]+ kernel_gs=1$" <<<"$CLEAN" \
  || fail "diagnostic sans le GS noyau"
if grep -Eq "gs_base=$user_gs" <<<"$CLEAN"; then
  fail "diagnostic exécuté avec le GS utilisateur"
fi
if [[ "$(grep -c '^\[EXCEPTION\] vector=' <<<"$CLEAN")" != 1 ]]; then
  fail "exceptions en cascade"
fi
grep -Fxq "[EXCEPTION] fail-closed halt" <<<"$CLEAN" || fail "arrêt fail-closed absent"
if grep -Fq "ELYON_KERNEL_READY_0_0_5" <<<"$CLEAN"; then
  fail "le noyau a continué après le #GP"
fi

echo "[PASS] K005-03 iretq: #GP de l'iretq de sortie ($iret) diagnostiqué avec le GS noyau, GS utilisateur $user_gs écarté, arrêt fail-closed (qemu rc=$RC)"
