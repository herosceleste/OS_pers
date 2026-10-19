# Elyon Kernel 0.0.4 — Scheduler & Kernel Tasks Foundation

Elyon 0.0.4 part de la baseline qualifiée `v0.0.3` et ouvre le chantier de l'ordonnancement noyau SMP.

La version 0.0.3 a établi ACPI/MADT, Local APIC, IOAPIC, timer LAPIC, état per-CPU, démarrage réel des AP par INIT/SIPI, stabilité SMP et une primitive coopérative de changement de contexte. La 0.0.4 transforme cette fondation en infrastructure de tâches noyau et d'ordonnancement préemptif.

## Objectif du jalon

Le jalon doit produire un scheduler noyau déterministe, testable et SMP-safe avec :

- modèle de tâche noyau et identifiants stables ;
- piles PMM dédiées ;
- run queues ;
- ordonnanceur Round-Robin coopératif puis préemptif ;
- contexte CPU sauvegardable ;
- état FPU/SSE/AVX isolé par tâche ;
- scheduler per-CPU et idle task ;
- IPI de rescheduling ;
- sleep/wakeup ;
- synchronisation adaptée au contexte IRQ/SMP ;
- stress tests multi-CPU ;
- qualification finale QEMU/OVMF et VMware.

Le mode utilisateur, Ring 3, les syscalls, les processus isolés, le VFS, le réseau et la GUI restent hors du périmètre 0.0.4.

## Baseline

- branche de développement : `elyon-0.0.4`
- base : `main` après la Release `v0.0.3`
- toolchain Rust : `1.99.1`
- architecture : x86-64 / UEFI
- baseline SMP héritée : 4 vCPU qualifiés QEMU/OVMF et VMware
