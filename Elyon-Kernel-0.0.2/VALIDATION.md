# Validation Elyon Kernel 0.0.2

## Statut actuel

**NON VALIDÉ.**

Le contenu de cette branche constitue une implémentation candidate. Aucune mention `PASS` ne doit être ajoutée avant exécution du test correspondant.

## Baseline héritée

Elyon Kernel 0.0.1+fix1 a été compilé avec Rust 1.99.1 et démarré avec succès sous VMware Workstation Pro 25H2 en UEFI, Secure Boot désactivé, 1 GiB de RAM et 2 vCPU. Le journal série a atteint `ELYON_KERNEL_READY`.

La 0.0.2 part de cette preuve mais doit être validée indépendamment.

## Critères 0.0.2

| ID | Critère | État |
|---|---|---|
| K002-01 | `elyon-loader.efi` et `elyon-kernel.elf` sont des artefacts distincts | À prouver |
| K002-02 | Le chargeur valide puis charge les segments ELF64 | À prouver |
| K002-03 | Le noyau utilise une pile dédiée contrôlée par Elyon | À prouver |
| K003 | La carte mémoire est triée, alignée et vérifiée sans chevauchement | À prouver |
| K005 | Les exceptions 0..31 produisent un diagnostic et s'arrêtent fail-closed | À prouver |
| K006 | Le PMM Elyon alloue des frames physiques | À prouver |
| K007 | CR3 pointe sur les tables de pages Elyon | À prouver |
| K008 | TSS et IST double fault sont actifs | À prouver |
| K009 | Le journal expose un build ID dérivé du commit | À prouver |
| K010 | ACPI RSDP est transmis au noyau lorsqu'il est exposé par le firmware | À prouver |
| K012 | Le framebuffer affiche l'écran Elyon lorsque GOP est utilisable | À prouver |

## Protocole minimal

1. `cargo fmt --all -- --check`.
2. `./scripts/build.sh` sous Rust 1.99.1.
3. Vérification du type PE/COFF du loader et ELF64 du kernel.
4. `./scripts/smoke-test.sh` sous QEMU/OVMF.
5. Génération de l'ISO par `./scripts/build-vmware-iso.sh`.
6. Boot VMware avec journal COM1.
7. Conservation du SHA du commit, des SHA-256 des artefacts et du journal complet.

Le succès nominal requiert `ELYON_KERNEL_READY_0_0_2`.
