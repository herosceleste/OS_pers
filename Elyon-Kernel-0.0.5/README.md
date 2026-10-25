# Elyon Kernel 0.0.5 — Ring 3 & System Calls Foundation

Elyon 0.0.5 part de la baseline qualifiée `v0.0.4` et ouvre le chantier du mode utilisateur.

La version 0.0.4 a établi un ordonnanceur noyau SMP préemptif sur 4 CPU : tâches à pile dédiée, contexte CPU et état x87/SSE/AVX par tâche, files et idle task par CPU, IPI de rescheduling, sommeil et réveil, spinlocks irq-save à ordre vérifié, récupération des tâches terminées et qualification sous stress. La 0.0.5 y fait tourner un premier processus en ring 3, isolé du noyau, qui n'atteint le noyau que par des appels système.

## Objectif du jalon

Le jalon doit produire une frontière ring 3 / ring 0 sûre et testable avec :

- segments utilisateur et pile noyau `RSP0` par tâche ;
- `swapgs` à chaque entrée depuis le ring 3, et entrées NMI/#DF/#MC robustes ;
- SMEP, SMAP et copies validées entre espace utilisateur et noyau ;
- pages utilisateur rendables et isolées ;
- `syscall`/`sysret` sans fuite de registres ;
- un premier processus utilisateur (`write`, `yield`, `exit`) ;
- des fautes en ring 3 qui tuent le processus, jamais le système ;
- la préemption sur tous les CPU ;
- une contre-pression au lieu d'arrêts sur file pleine ;
- un stress SMP ring 3 / ring 0 ;
- une qualification finale QEMU/OVMF et VMware.

Un CR3 par processus, les fautes de page et `mmap` (0.0.6), le VFS, le chargeur ELF (0.0.7), le stockage, le réseau et la GUI restent hors du périmètre 0.0.5.

## Baseline

- branche de développement : `elyon-0.0.5`
- base : `main` après la Release `v0.0.4`
- toolchain Rust : `1.99.1`
- architecture : x86-64 / UEFI
- baseline héritée : K003-01..12 et K004-01..12 qualifiés sous QEMU/OVMF et VMware, 4 vCPU
