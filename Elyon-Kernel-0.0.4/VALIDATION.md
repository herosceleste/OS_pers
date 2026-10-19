# Elyon Kernel 0.0.4 — Validation

**DÉMARRAGE DU JALON — K004-01 à K004-12 sont à qualifier.**

## Baseline héritée

La 0.0.4 est dérivée de la Release `v0.0.3`, dont les critères K003-01 à K003-12 sont qualifiés sous QEMU/OVMF et VMware Workstation en 4 vCPU. La CI 0.0.4 doit conserver ces invariants comme régressions pendant tout le chantier scheduler.

## Critères

| Critère | Description | Statut |
|---|---|---|
| K004-01 | Modèle de tâche noyau et machine d'états | À faire |
| K004-02 | Pile et contexte initial dédiés | À faire |
| K004-03 | Round-Robin coopératif | À faire |
| K004-04 | Préemption LAPIC | À faire |
| K004-05 | Contexte CPU scheduler complet | À faire |
| K004-06 | Sauvegarde FPU/SSE/AVX | À faire |
| K004-07 | Scheduler per-CPU + idle task | À faire |
| K004-08 | Reschedule IPI + affinité | À faire |
| K004-09 | Sleep/wakeup | À faire |
| K004-10 | Synchronisation scheduler/IRQ/SMP | À faire |
| K004-11 | Stress SMP | À faire |
| K004-12 | Qualification finale 0.0.4 | À faire |

## Politique de preuve

Chaque PASS doit être lié à un commit qualifié et à un test exécuté. Les marqueurs série seuls ne suffisent pas si le test ne valide pas également l'invariant concerné.

La première étape fonctionnelle sera **K004-01**.
