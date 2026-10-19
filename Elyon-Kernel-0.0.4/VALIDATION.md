# Elyon Kernel 0.0.4 — Validation

**EN COURS — K004-01 est validé. K004-02 à K004-12 restent à qualifier.**

## Baseline héritée

La 0.0.4 est dérivée de la Release `v0.0.3`, dont les critères K003-01 à K003-12 sont qualifiés sous QEMU/OVMF et VMware Workstation en 4 vCPU. La CI 0.0.4 doit conserver ces invariants comme régressions pendant tout le chantier scheduler.

## Critères

| Critère | Description | Statut |
|---|---|---|
| K004-01 | Modèle de tâche noyau et machine d'états | **PASS** |
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


## K004-01 — Modèle de tâche noyau

Elyon dispose désormais d'un registre statique borné à 256 tâches noyau et d'identifiants monotones non nuls. La création publie une tâche en état `READY` seulement après initialisation complète de son slot.

La machine d'états qualifiée comprend :

```text
READY
RUNNING
BLOCKED
SLEEPING
DEAD
```

Les transitions sont contrôlées par opérations atomiques. Une tâche `RUNNING` peut céder vers `READY`, se bloquer ou dormir. Les tâches `BLOCKED` et `SLEEPING` peuvent être réveillées vers `READY`. La terminaison place la tâche en `DEAD`.

L'affinité CPU fait déjà partie du modèle. Une tentative d'exécution sur un CPU absent du masque d'affinité est rejetée avant la transition vers `RUNNING`.

La qualification a été exécutée sur le commit `f24c559e510ee2bd6aa9cdf26f6d81afbe166fe5`, GitHub Actions run **#13** du workflow Elyon 0.0.4.

Preuves CI :

```text
[task] ids=1,2 unique=1 capacity=256 created=2 transitions=12 invalid_rejected=2 states_seen=0x1f
[task] states=READY,RUNNING,BLOCKED,SLEEPING,DEAD affinity_enforced=1
ELYON_TASK_MODEL_READY
[PASS] K004-01 modèle de tâche noyau, 5 états, IDs uniques, affinité et transitions invalides rejetées
```

Le même run confirme également la baseline héritée :

```text
[PASS] Elyon 0.0.4 baseline: régressions K003-01..12 cohérentes sur 4 vCPU, boot nominal sans anomalie
[PASS] #UD diagnostiqué puis arrêté en fail-closed
[PASS] #DF pris en charge sur l'IST dédiée puis arrêt fail-closed
```

**K004-01 : PASS.**

Le prochain critère est **K004-02 : donner à chaque tâche exécutable une pile PMM dédiée et un contexte initial propre, puis prouver que plusieurs tâches démarrent réellement sur des piles distinctes sans chevauchement.**
