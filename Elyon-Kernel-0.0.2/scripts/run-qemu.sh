#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ESP="$ROOT/build/esp"

[[ -f "$ESP/EFI/BOOT/BOOTX64.EFI" && -f "$ESP/ELYON/KERNEL.ELF" ]] || "$ROOT/scripts/build.sh"
command -v qemu-system-x86_64 >/dev/null || {
  echo "qemu-system-x86_64 introuvable" >&2
  exit 1
}

find_ovmf() {
  for d in /usr/share/OVMF /usr/share/edk2/ovmf /usr/share/edk2/x64; do
    for pair in "OVMF_CODE_4M.fd OVMF_VARS_4M.fd" "OVMF_CODE.fd OVMF_VARS.fd"; do
      set -- $pair
      if [[ -f "$d/$1" && -f "$d/$2" ]]; then
        echo "$d/$1 $d/$2"
        return 0
      fi
    done
  done
  return 1
}

read -r OVMF_CODE OVMF_VARS_TEMPLATE < <(find_ovmf) || {
  echo "OVMF CODE/VARS introuvables" >&2
  exit 1
}

OVMF_VARS="$ROOT/build/OVMF_VARS.fd"
cp "$OVMF_VARS_TEMPLATE" "$OVMF_VARS"

QEMU_ARGS=(
  -machine q35,accel=tcg
  -m 1024M
  -cpu max
  -drive "if=pflash,format=raw,readonly=on,file=$OVMF_CODE"
  -drive "if=pflash,format=raw,file=$OVMF_VARS"
  -drive "format=raw,file=fat:rw:$ESP"
  -serial stdio
  -no-reboot
  -no-shutdown
)

if [[ "${ELYON_HEADLESS:-0}" == "1" ]]; then
  QEMU_ARGS+=(-display none)
else
  QEMU_ARGS+=(-display gtk)
fi

qemu-system-x86_64 "${QEMU_ARGS[@]}"
