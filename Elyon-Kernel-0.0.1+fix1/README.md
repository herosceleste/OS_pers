# Elyon Kernel 0.0.1 — bootstrap corrige

> Statut : le tag historique `v0.0.1` est conserve pour la tracabilite mais pointe sur le bootstrap initial non compilable. Le commit de correction du 17/10/2026, tague `v0.0.1+fix1`, constitue la base de travail pour 0.0.2. Le tag intermediaire `v0.0.1-corrected` est conserve mais ne doit plus servir de reference : selon SemVer, un suffixe `-xxx` designe une pre-version, classee avant `v0.0.1`. Les limites connues sont documentees dans `docs/KNOWN_LIMITATIONS_0.0.1.md`.

Elyon Kernel 0.0.1 est le premier jalon executable du projet Elyon OS. Il ne s'agit pas d'une distribution Linux, d'un fork BSD ou d'un habillage d'un systeme existant. Cette version est un bootstrap de noyau x86-64 charge directement par un firmware UEFI.

## Ce que fait 0.0.1

La version 0.0.1 initialise une console serie COM1, recupere la carte memoire UEFI definitive, appelle `ExitBootServices`, entre dans le noyau Elyon sans service de boot du firmware, identifie le CPU, installe une GDT Elyon minimale, installe une IDT fail-closed pour les exceptions architecturales, affiche CR0/CR3/CR4 et termine sur une boucle `hlt` controlee.

Le marqueur de succes automatise est :

```text
ELYON_KERNEL_READY
```

La version 0.0.1 ne contient volontairement ni ordonnanceur, ni espace utilisateur, ni pilote de stockage, ni pile reseau. Ces composants appartiennent aux jalons suivants.

## Architecture du depot

```text
Elyon-Kernel-0.0.1/
├── crates/
│   ├── elyon-boot/          chargeur UEFI et handoff
│   └── elyon-kernel/        noyau no_std
├── docs/                    decisions et securite
├── scripts/                 build, QEMU et smoke test
└── .github/workflows/       CI reproductible
```

## Pre-requis Linux de developpement

Le poste de compilation peut etre Linux, macOS ou Windows. Cela ne signifie pas qu'Elyon depend de Linux pour son execution. Pour Ubuntu/Debian, les outils pratiques sont : `qemu-system-x86`, `ovmf`, `git`, `curl` et Rust via rustup.

Rust est fige a 1.99.1 dans `rust-toolchain.toml`.

```bash
rustup target add x86_64-unknown-uefi
./scripts/build.sh
./scripts/run-qemu.sh
```

Pour un test automatise :

```bash
./scripts/smoke-test.sh
```

## Frontiere UEFI / noyau

`elyon-boot` utilise UEFI uniquement pour obtenir l'environnement de demarrage et la carte memoire. L'appel a `uefi::boot::exit_boot_services` constitue la frontiere. Apres cette transition, le noyau n'utilise plus les Boot Services.

Dans 0.0.1, le chargeur et le noyau sont lies dans une meme image PE/COFF UEFI pour reduire le risque du premier bootstrap. La separation en chargeur PE/COFF et noyau ELF `x86_64-unknown-none` est planifiee pour le jalon suivant. Elle est deja preparee dans le toolchain.

## Securite

Le noyau desactive les interruptions avant de remplacer ses tables de descripteurs. L'IDT initiale est fail-closed : une exception non encore diagnostiquee stoppe le CPU au lieu de poursuivre dans un etat inconnu. Aucune memoire UEFI de type `LOADER_CODE` ou `LOADER_DATA` n'est declaree reutilisable dans cette version, car l'image Elyon y reside encore.

Voir `SECURITY.md` et `docs/SECURITY_BASELINE.md`.

## Regle de performance

Aucune revendication du type "plus rapide que Linux" n'est acceptee sans benchmark reproductible. Les futures mesures seront publiees avec materiel, configuration, version, protocole et variance.

## Licence

Aucune licence open source definitive n'est imposee dans 0.0.1. Tant qu'une licence n'est pas formellement choisie par le proprietaire du projet, le code est considere comme non redistribuable sans autorisation. Voir `LICENSE_POLICY.md`.

## Verrouillage des dependances

Les dependances directes sont epinglees a des versions exactes et l'arbre complet des dependances est verrouille dans `Cargo.lock`, versionne dans le depot. `scripts/build.sh` compile avec `--locked` : le build echoue si `Cargo.lock` est absent ou doit etre modifie, et Cargo verifie la somme SHA-256 de chaque crate telechargee par rapport au fichier de verrouillage.
