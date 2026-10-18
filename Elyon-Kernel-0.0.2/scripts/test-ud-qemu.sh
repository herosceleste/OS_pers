#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UEFI_TARGET="x86_64-unknown-uefi"
KERNEL_TARGET="x86_64-unknown-none"
TARGET_DIR="$ROOT/target-test-ud"
ESP="$ROOT/build/tests/ud/esp"
LOG="$ROOT/build/tests/ud/qemu.log"
QEMU_TIMEOUT="${ELYON_QEMU_TIMEOUT:-15}"

rm -rf "$TARGET_DIR" "$ROOT/build/tests/ud"
mkdir -p "$ESP/EFI/BOOT" "$ESP/ELYON"

export CARGO_TARGET_DIR="$TARGET_DIR"
export ELYON_TEST_UD=1
export ELYON_BUILD_ID="$(git -C "$ROOT/.." rev-parse --short=12 HEAD 2>/dev/null || echo unknown)-ud"

cargo build   --locked   --manifest-path "$ROOT/Cargo.toml"   -p elyon-kernel   --target "$KERNEL_TARGET"   --release

cargo build   --locked   --manifest-path "$ROOT/Cargo.toml"   -p elyon-loader   --target "$UEFI_TARGET"   --release

cp "$TARGET_DIR/$UEFI_TARGET/release/elyon-loader.efi" "$ESP/EFI/BOOT/BOOTX64.EFI"
cp "$TARGET_DIR/$KERNEL_TARGET/release/elyon-kernel" "$ESP/ELYON/KERNEL.ELF"

set +e
timeout "${QEMU_TIMEOUT}s" env ELYON_HEADLESS=1 ELYON_ESP="$ESP" bash "$ROOT/scripts/run-qemu.sh" >"$LOG" 2>&1
RC=$?
set -e

cat "$LOG"

grep -Fq "[test] injecting #UD via ud2" "$LOG" || {
  echo "[FAIL] injection #UD non atteinte" >&2
  exit 1
}

grep -Fq "[EXCEPTION] vector=6 #UD invalid opcode" "$LOG" || {
  echo "[FAIL] diagnostic #UD absent" >&2
  exit 1
}

grep -Fq "[EXCEPTION] fail-closed halt" "$LOG" || {
  echo "[FAIL] arrêt fail-closed absent" >&2
  exit 1
}

if grep -Fq "ELYON_KERNEL_READY_0_0_2" "$LOG"; then
  echo "[FAIL] le noyau a continué après #UD" >&2
  exit 1
fi

echo "[PASS] #UD diagnostiqué puis arrêté en fail-closed (qemu rc=$RC)"
