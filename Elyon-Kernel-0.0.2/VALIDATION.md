# Validation Elyon Kernel 0.0.2

## Statut final

**ELYON KERNEL 0.0.2 VALIDÉ POUR LE PÉRIMÈTRE DU JALON.**

La version 0.0.2 a passé la compilation Rust 1.99.1, le boot nominal sous QEMU/OVMF et VMware Workstation Pro 25H2, le test contrôlé `#UD` en arrêt fail-closed, ainsi qu'un véritable test `#DF` démontrant l'utilisation effective de la pile IST dédiée.

Le périmètre validé couvre le découplage loader/kernel, le handoff UEFI, la carte mémoire validée, le PMM initial, la pagination Elyon et son propre CR3, GDT/TSS/IDT, l'IST du double fault, ACPI RSDP, framebuffer GOP et le build ID.

## Référence technique

- branche : `elyon-0.0.2`
- toolchain : Rust 1.99.1
- architecture : x86-64
- firmware : UEFI
- loader : PE32+ EFI x86-64
- kernel : ELF64 x86-64 statique
- CI de qualification : GitHub Actions, Ubuntu 24.04
- run de qualification des tests CPU : **#70**
- commit de code qualifié : `7ac94d68f575f8ab2758aaa2acad7c1a8071ef05`

## Validation nominale

Le boot nominal atteint :

```text
ELYON_KERNEL_READY_0_0_2
```

La CI confirme :

```text
[PASS] Elyon Kernel 0.0.2 a atteint ELYON_KERNEL_READY_0_0_2
```

Sous VMware, le loader UEFI charge le noyau ELF, quitte les Boot Services, transmet une carte mémoire finale validée, puis le noyau initialise ses structures d'architecture, son PMM, sa pagination et le framebuffer.

Les tables de pages statiques sont vérifiées comme appartenant à la plage réservée du noyau avant le basculement de CR3.

## Validation PMM

Le PMM ne distribue plus de frame dans la zone historique sous 1 MiB par défaut.

Deux protections sont en place :

1. réservation explicite de `0..1 MiB` ;
2. plancher d'allocation dans `allocate_frame()`.

Le boot s'arrête si l'auto-test obtient malgré tout une frame inférieure à `0x100000`.

## Validation #UD

Le test `#UD` compile une variante isolée du noyau et exécute volontairement `ud2`.

Preuves CI :

```text
[test] injecting #UD via ud2
[EXCEPTION] vector=6 #UD invalid opcode
[EXCEPTION] fail-closed halt
[PASS] #UD diagnostiqué puis arrêté en fail-closed (qemu rc=124)
```

Le marqueur nominal `ELYON_KERNEL_READY_0_0_2` ne doit pas être atteint après l'injection.

**K005 : PASS.**

## Validation #DF / IST

Le test double fault est exécuté après installation de la pagination Elyon. Il remplace volontairement `RSP` par une adresse non mappée au-dessus des 4 GiB initialement identity-mappés, puis déclenche `int3`.

L'échec de construction des trames d'exception sur cette pile provoque un double fault. L'entrée IDT du vecteur 8 utilise l'IST dédiée configurée dans le TSS.

Preuves CI :

```text
[test] injecting #DF via invalid stack + int3
[EXCEPTION] vector=8 #DF double fault
[EXCEPTION] #DF IST rsp=0x0000000000308ec0 span=0x0000000000305010..0x0000000000309010 active=1
[EXCEPTION] fail-closed halt
[PASS] #DF pris en charge sur l'IST dédiée puis arrêt fail-closed (qemu rc=124)
```

La valeur `active=1` prouve que le handler s'exécute réellement dans la plage mémoire de la pile IST dédiée.

**K008 : PASS.**

## Critères 0.0.2

| ID | Critère | État |
|---|---|---|
| K002-01 | Loader UEFI et kernel ELF sont des artefacts distincts | PASS |
| K002-02 | Le loader valide et charge les segments ELF64 | PASS |
| K002-03 | Le noyau utilise une pile dédiée contrôlée par Elyon | PASS |
| K003 | Carte mémoire triée, alignée et vérifiée sans chevauchement | PASS |
| K005 | Exceptions diagnostiques et arrêt fail-closed prouvés par `#UD` | PASS |
| K006 | PMM Elyon avec protection bas-mémoire | PASS |
| K007 | CR3 bascule vers les tables de pages Elyon | PASS |
| K008 | TSS et IST double fault actifs, prouvés par injection réelle | PASS |
| K009 | Build ID dérivé du commit | PASS |
| K010 | ACPI RSDP transmis lorsque disponible | PASS |
| K012 | Framebuffer GOP utilisable par Elyon | PASS |

## Limites assumées du jalon

Ces éléments ne sont pas des échecs de la 0.0.2 mais appartiennent aux jalons suivants :

- identity mapping initial de 4 GiB encore RWX, donc W^X/NX non finalisé ;
- PMM bitmap statique plafonné à 32 GiB ;
- pas encore d'APIC, timer, SMP ou scheduler ;
- ACPI transmis mais pas encore intégralement parsé ;
- Secure Boot encore désactivé ;
- reproductibilité bit-à-bit de l'ISO non encore démontrée.

## Conclusion

Le périmètre prévu pour Elyon Kernel 0.0.2 est validé. Le prochain jalon peut démarrer sur une base stable : **APIC, timer, SMP et préparation du scheduler**.
