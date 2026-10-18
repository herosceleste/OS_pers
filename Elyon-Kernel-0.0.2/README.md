# Elyon Kernel 0.0.2

Statut : **candidat d'implementation, non encore valide au boot**.

Elyon Kernel 0.0.2 poursuit le bootstrap 0.0.1+fix1 validé sous VMware avec Rust 1.99.1. Ce jalon sépare physiquement le chargeur UEFI et le noyau ELF64, introduit une ABI de handoff explicite et commence à transférer au noyau la maîtrise de la mémoire et de la pagination.

## Objectifs techniques

- `elyon-loader.efi` distinct de `elyon-kernel.elf`.
- Chargement ELF64 des segments `PT_LOAD`.
- Pile noyau dédiée, allouée avant `ExitBootServices`.
- `BootInfo` versionné partagé par `elyon-loader` et `elyon-kernel`.
- Carte mémoire UEFI triée et validée après la sortie des Boot Services.
- Détection de l'ACPI RSDP.
- Métadonnées GOP transmises au noyau.
- GDT Elyon avec TSS et pile IST dédiée au double fault.
- IDT diagnostique pour les exceptions CPU 0 à 31.
- Premier allocateur physique de frames par bitmap.
- Premières tables de pagination appartenant à Elyon, avec identité 0-4 GiB en pages de 2 MiB.
- Premier écran graphique Elyon via framebuffer GOP lorsque la zone est accessible.
- Build ID dérivé du commit Git.

Le marqueur attendu du jalon est :

```text
ELYON_KERNEL_READY_0_0_2
```

## Arborescence

```text
Elyon-Kernel-0.0.2/
├── crates/
│   ├── elyon-abi/
│   ├── elyon-loader/
│   └── elyon-kernel/
├── scripts/
│   ├── build.sh
│   ├── run-qemu.sh
│   ├── smoke-test.sh
│   └── build-vmware-iso.sh
└── docs/
```

## Build

```bash
chmod +x scripts/*.sh
./scripts/build.sh
```

Les artefacts attendus sont :

```text
build/esp/EFI/BOOT/BOOTX64.EFI
build/esp/ELYON/KERNEL.ELF
```

Pour produire l'ISO VMware :

```bash
sudo apt install -y xorriso dosfstools mtools
./scripts/build-vmware-iso.sh
```

## Règle de validation

La présence du code n'est pas une preuve de fonctionnement. La 0.0.2 ne sera déclarée validée qu'après compilation avec Rust 1.99.1, smoke test QEMU/OVMF, boot VMware, conservation des journaux et rattachement des preuves au commit testé.
