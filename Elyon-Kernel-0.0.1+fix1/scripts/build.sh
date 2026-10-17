#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="x86_64-unknown-uefi"
PROFILE="release"
ESP="$ROOT/build/esp"

command -v cargo >/dev/null || { echo "cargo introuvable" >&2; exit 1; }
command -v rustup >/dev/null || { echo "rustup introuvable" >&2; exit 1; }

rustup target add "$TARGET" >/dev/null
cargo build --locked --manifest-path "$ROOT/Cargo.toml" -p elyon-boot --target "$TARGET" --release

BIN="$ROOT/target/$TARGET/$PROFILE/elyon-boot.efi"
if [[ ! -f "$BIN" ]]; then
  echo "Binaire UEFI introuvable: $BIN" >&2
  exit 1
fi

rm -rf "$ESP"
mkdir -p "$ESP/EFI/BOOT"
cp "$BIN" "$ESP/EFI/BOOT/BOOTX64.EFI"

mkdir -p "$ROOT/build"
sha256sum "$ESP/EFI/BOOT/BOOTX64.EFI" > "$ROOT/build/SHA256SUMS"

echo "[OK] $ESP/EFI/BOOT/BOOTX64.EFI"
cat "$ROOT/build/SHA256SUMS"
