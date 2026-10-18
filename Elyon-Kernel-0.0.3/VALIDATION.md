# Validation Elyon Kernel 0.0.3

## Statut

**EN COURS — aucun critère 0.0.3 n'est considéré PASS sans preuve exécutée.**

Baseline de départ : fusion 0.0.2 dans `main`, commit `303feeeff93b4ed1b5b3092cc16a4d8c4489f252`.

## Critères

| ID | Critère | État |
|---|---|---|
| K003-01 | RSDP validé par signature et checksum | À faire |
| K003-02 | XSDT/RSDT parcourue avec validation des tables | À faire |
| K003-03 | MADT parsée et topologie CPU/APIC journalisée | À faire |
| K003-04 | Local APIC du BSP activé et APIC ID confirmé | À faire |
| K003-05 | IOAPIC découvert et redirections initiales maîtrisées | À faire |
| K003-06 | Timer noyau produit des interruptions répétables | À faire |
| K003-07 | État per-CPU avec pile noyau, TSS et IST dédiées | À faire |
| K003-08 | Au moins un AP est réellement démarré par INIT/SIPI | À faire |
| K003-09 | Chaque AP atteint `ELYON_AP_READY` avec son APIC ID | À faire |
| K003-10 | Le BSP reste stable après activation SMP | À faire |
| K003-11 | Base de changement de contexte noyau testée | À faire |
| K003-12 | CI nominale et SMP sans régression des critères 0.0.2 | À faire |

## Preuves exigées

Un critère ne passe à PASS que si les trois éléments suivants existent :

1. code présent sur un commit identifié ;
2. test exécuté dans un environnement identifié ;
3. journal ou assertion CI démontrant le comportement attendu.

La simple présence de structures ACPI, APIC, TSS ou d'un trampoline dans le code ne constitue pas une preuve de fonctionnement.

## Environnements de qualification

- GitHub Actions + QEMU/OVMF pour les tests automatisables ;
- VMware Workstation Pro pour une validation croisée sur 2 puis 4 vCPU ;
- Rust 1.99.1 tant qu'une montée de toolchain n'est pas explicitement décidée et qualifiée.

## Régressions interdites

La 0.0.3 doit continuer à atteindre le boot nominal, préserver les protections PMM bas-mémoire, le CR3 Elyon, les diagnostics d'exception et le chemin #DF/IST validé en 0.0.2.
