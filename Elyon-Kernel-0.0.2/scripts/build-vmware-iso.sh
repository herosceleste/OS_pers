#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ISO="$ROOT/build/Elyon-Kernel-0.0.2-UEFI.iso"
IMG="$ROOT/build/vmware/efiboot.img"
ISO_DIR="$ROOT/build/vmware/iso"

bash "$ROOT/scripts/build.sh"

for cmd in xorriso mkfs.vfat mmd mcopy; do
  command -v "$cmd" >/dev/null || {
    echo "$cmd introuvable (paquets: xorriso dosfstools mtools)" >&2
    exit 1
  }
done

rm -rf "$ROOT/build/vmware"
mkdir -p "$ISO_DIR/EFI/BOOT" "$ISO_DIR/ELYON"

dd if=/dev/zero of="$IMG" bs=1M count=16 status=none
mkfs.vfat -F 16 -n ELYON_002 -i 454C5932 "$IMG" >/dev/null
mmd -i "$IMG" ::/EFI
mmd -i "$IMG" ::/EFI/BOOT
mmd -i "$IMG" ::/ELYON
mcopy -i "$IMG" "$ROOT/build/esp/EFI/BOOT/BOOTX64.EFI" ::/EFI/BOOT/BOOTX64.EFI
mcopy -i "$IMG" "$ROOT/build/esp/ELYON/KERNEL.ELF" ::/ELYON/KERNEL.ELF
cp "$IMG" "$ISO_DIR/efiboot.img"
cp "$ROOT/build/esp/EFI/BOOT/BOOTX64.EFI" "$ISO_DIR/EFI/BOOT/BOOTX64.EFI"
cp "$ROOT/build/esp/ELYON/KERNEL.ELF" "$ISO_DIR/ELYON/KERNEL.ELF"

xorriso -as mkisofs   -iso-level 3   -R   -J   -V "ELYON_002"   -e efiboot.img   -no-emul-boot   -o "$ISO"   "$ISO_DIR" >/dev/null

sha256sum "$ISO"
echo "[OK] $ISO"
