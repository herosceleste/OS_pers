# Validation Elyon Kernel 0.0.2

## Statut actuel

**BOOT NOMINAL VALIDÉ SOUS QEMU/OVMF ET VMWARE.**

Le jalon a désormais passé la compilation Rust 1.99.1, le smoke test QEMU/OVMF et un boot VMware Workstation Pro 25H2 jusqu'au marqueur `ELYON_KERNEL_READY_0_0_2`.

La validation n'est toutefois pas encore complète pour les critères liés aux fautes CPU : les handlers diagnostiques et l'IST de double fault sont installés, mais leur comportement doit encore être prouvé par injection contrôlée d'exceptions avant de déclarer la 0.0.2 totalement validée.

## Baseline héritée

Elyon Kernel 0.0.1+fix1 a été compilé avec Rust 1.99.1 et démarré avec succès sous VMware Workstation Pro 25H2 en UEFI, Secure Boot désactivé, 1 GiB de RAM et 2 vCPU. Le journal série a atteint `ELYON_KERNEL_READY`.

## Validation CI

GitHub Actions, Ubuntu 24.04, Rust 1.99.1 :

- `cargo fmt --all -- --check` : PASS
- compilation `elyon-loader.efi` : PASS
- compilation `elyon-kernel.elf` : PASS
- type loader : PE32+ EFI x86-64
- type kernel : ELF64 x86-64 statique
- smoke test QEMU/OVMF : PASS
- marqueur atteint : `ELYON_KERNEL_READY_0_0_2`

## Validation VMware du 17/10/2026

Configuration de référence :

- VMware Workstation Pro 25H2
- firmware UEFI
- Secure Boot désactivé
- 1 GiB de RAM
- 2 vCPU
- sortie série COM1 vers fichier
- Rust 1.99.1
- build ID : `aa0fb35b87c1`

Empreintes locales :

- `BOOTX64.EFI` : `7a132a33fe4daa2e487ea68dae97b9cd6fc49e41e529acbca4c3af38e4066d4e`
- `KERNEL.ELF` : `cea70428a35336d3840cd6bf71703821bd7b0a6f6ef86a526bfe32a4bf25c273`
- ISO VMware : `eee1fb44b5c15e055957750b08b1887475d731b9786bf4abcffea450f8b6d03f`

Résultats observés :

- loader UEFI démarré ;
- kernel ELF chargé en `0x200000..0x311000`, entrée `0x200000` ;
- ACPI RSDP détecté à `0x0fbf0000` ;
- GOP détecté en 1024x768, stride 1024, framebuffer `0xf0000000` ;
- pile noyau dédiée `0x0d9cc000..0x0d9dc000` ;
- carte mémoire finale : 38 régions, validation = 1, mémoire conventionnelle = 988 MiB ;
- handoff loader/kernel validé ;
- GDT chargée, TSS chargé, IST double fault configuré ;
- IDT diagnostique installée ;
- PMM initialisé sur 1024 MiB, 262144 frames gérées, 253078 libres ;
- auto-test PMM : deux frames allouées ;
- CR3 basculé de `0x000000000fc01000` vers `0x0000000000310000` ;
- framebuffer affiché avec succès en 1024x768 ;
- écran VMware visible : `ELYON OS / KERNEL 0.0.2 / BOOT OK` ;
- marqueur final : `ELYON_KERNEL_READY_0_0_2`.

Journal série de référence :

```text
ELYON LOADER 0.0.2
[loader] firmware=UEFI architecture=x86_64
[loader] kernel file size=1074840 bytes
[loader] kernel ELF loaded phys=0x200000..0x311000 entry=0x200000
[loader] ACPI RSDP=0xfbf0000
[loader] GOP=1024x768 stride=1024 base=0xf0000000
[loader] stack=0xd9cc000..0xd9dc000 bootinfo=0xd9ca000
[loader] leaving UEFI Boot Services
[loader] memory map regions=38 valid=1 conventional=988 MiB
ELYON KERNEL 0.0.2
[kernel] build_id=aa0fb35b87c1
[kernel] handoff=OK regions=38 conventional=988 MiB
[kernel] image=0x200000..0x311000 stack=0xd9cc000..0xd9dc000
[cpu] vendor=GenuineIntel
[arch] GDT=OK TSS=OK IST(#DF)=OK selector=0x8
[arch] IDT=OK exceptions=diagnostic
[memory] PMM=OK managed=1024 MiB frames=262144 free=253078
[memory] PMM self-test frames=0x2000,0x3000
[paging] Elyon CR3=OK old=0x000000000fc01000 new=0x0000000000310000 identity_limit=4096 MiB
[acpi] RSDP=0x000000000fbf0000
[display] framebuffer=OK 1024x768 stride=1024
[cpu] CR0=0x0000000080010033 CR3=0x0000000000310000 CR4=0x0000000000000668
ELYON_KERNEL_READY_0_0_2
elyon> loader/kernel split active; scheduler not started
```

## Critères 0.0.2

| ID | Critère | État |
|---|---|---|
| K002-01 | `elyon-loader.efi` et `elyon-kernel.elf` sont des artefacts distincts | PASS |
| K002-02 | Le chargeur valide puis charge les segments ELF64 | PASS nominal |
| K002-03 | Le noyau utilise une pile dédiée contrôlée par Elyon | PASS |
| K003 | La carte mémoire est triée, alignée et vérifiée sans chevauchement | PASS nominal |
| K005 | Les exceptions 0..31 produisent un diagnostic et s'arrêtent fail-closed | À prouver par injection |
| K006 | Le PMM Elyon alloue des frames physiques | PASS, avec durcissement bas-mémoire à faire |
| K007 | CR3 pointe sur les tables de pages Elyon | PASS |
| K008 | TSS et IST double fault sont actifs | PARTIEL, configuration chargée mais double fault à injecter |
| K009 | Le journal expose un build ID dérivé du commit | PASS |
| K010 | ACPI RSDP est transmis au noyau lorsqu'il est exposé par le firmware | PASS |
| K012 | Le framebuffer affiche l'écran Elyon lorsque GOP est utilisable | PASS |

## Points à fermer avant validation totale

1. Injecter au minimum `#UD` et vérifier le diagnostic série puis l'arrêt fail-closed.
2. Déclencher un test dédié du chemin `#DF`/IST dans un environnement contrôlé.
3. Durcir le PMM pour ne jamais distribuer de frame dans la zone bas-mémoire historique avant qu'une politique explicite ne l'autorise.
4. Vérifier par symboles ELF que toutes les tables de pages statiques et structures critiques sont incluses dans la plage réservée du noyau.
5. Conserver les preuves finales rattachées au commit effectivement testé.

Le succès nominal VMware est acquis. La qualification complète 0.0.2 attend encore les tests de faute contrôlés.
