# Elyon Kernel 0.0.4 — Architecture cible

## Héritage 0.0.3

Elyon dispose déjà d'un loader UEFI séparé, d'un PMM, d'une pagination initiale propre au noyau, d'ACPI/MADT, LAPIC/IOAPIC, d'un timer LAPIC, d'un état per-CPU, d'un bootstrap SMP INIT/SIPI et d'une primitive coopérative de changement de contexte.

## Couche tasking 0.0.4

La nouvelle couche introduit trois concepts distincts :

1. **Task** : identité, état, contexte CPU, pile, affinité et métadonnées d'ordonnancement.
2. **RunQueue per-CPU** : ensemble ordonné des tâches READY pouvant être choisies par un CPU.
3. **Scheduler** : politique de sélection et mécanismes de transition RUNNING/READY/BLOCKED/SLEEPING.

Le premier algorithme est Round-Robin pour conserver un comportement déterministe et auditable.

## Préemption

Le timer LAPIC devient une source de préemption. Le frame d'interruption et le contexte coopératif seront unifiés autour d'un modèle de contexte explicite avant activation générale de la préemption.

## SMP

Chaque CPU possède son scheduler local, sa run queue et son idle task. Les IPI servent à demander un rescheduling ou à réveiller un CPU distant. Une tâche RUNNING appartient à exactement un CPU.

## Hors périmètre

Ring 3, syscalls, espaces d'adressage par processus, VFS, drivers de stockage avancés, réseau et GUI ne font pas partie du jalon 0.0.4.
