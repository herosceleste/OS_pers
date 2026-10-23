#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UEFI_TARGET="x86_64-unknown-uefi"
KERNEL_TARGET="x86_64-unknown-none"
PROFILE="release"
ESP="$ROOT/build/esp"

command -v cargo >/dev/null || { echo "cargo introuvable" >&2; exit 1; }
command -v rustup >/dev/null || { echo "rustup introuvable" >&2; exit 1; }

export ELYON_BUILD_ID
ELYON_BUILD_ID="$(git -C "$ROOT/.." rev-parse --short=12 HEAD 2>/dev/null || echo unknown)"

rustup target add "$UEFI_TARGET" "$KERNEL_TARGET" >/dev/null

cargo build   --locked   --manifest-path "$ROOT/Cargo.toml"   -p elyon-kernel   --target "$KERNEL_TARGET"   --release

cargo build   --locked   --manifest-path "$ROOT/Cargo.toml"   -p elyon-loader   --target "$UEFI_TARGET"   --release

KERNEL="$ROOT/target/$KERNEL_TARGET/$PROFILE/elyon-kernel"
LOADER="$ROOT/target/$UEFI_TARGET/$PROFILE/elyon-loader.efi"

[[ -f "$KERNEL" ]] || { echo "Kernel ELF introuvable: $KERNEL" >&2; exit 1; }
[[ -f "$LOADER" ]] || { echo "Loader UEFI introuvable: $LOADER" >&2; exit 1; }

rm -rf "$ESP"
mkdir -p "$ESP/EFI/BOOT" "$ESP/ELYON"
cp "$LOADER" "$ESP/EFI/BOOT/BOOTX64.EFI"
cp "$KERNEL" "$ESP/ELYON/KERNEL.ELF"

mkdir -p "$ROOT/build"
(
  cd "$ROOT"
  sha256sum     build/esp/EFI/BOOT/BOOTX64.EFI     build/esp/ELYON/KERNEL.ELF     > build/SHA256SUMS
)

echo "[OK] Elyon 0.0.3 build_id=$ELYON_BUILD_ID"
file "$LOADER" "$KERNEL" 2>/dev/null || true
cat "$ROOT/build/SHA256SUMS"
