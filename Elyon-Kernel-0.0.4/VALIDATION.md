# Elyon Kernel 0.0.4 — Validation

**EN COURS — K004-01 et K004-02 sont validés. K004-03 à K004-12 restent à qualifier.**

## Baseline héritée

La 0.0.4 est dérivée de la Release `v0.0.3`, dont les critères K003-01 à K003-12 sont qualifiés sous QEMU/OVMF et VMware Workstation en 4 vCPU. La CI 0.0.4 doit conserver ces invariants comme régressions pendant tout le chantier scheduler.

## Critères

| Critère | Description | Statut |
|---|---|---|
| K004-01 | Modèle de tâche noyau et machine d'états | **PASS** |
| K004-02 | Pile et contexte initial dédiés | **PASS** |
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

## K004-02 — Pile PMM dédiée et contexte initial propre

Chaque tâche exécutable reçoit sa propre pile de 64 KiB allouée au PMM. Le registre de tâches enregistre la pile de chaque tâche et applique l'invariant de la feuille de route : une pile n'appartient qu'à une seule tâche active. `task::attach_stack` refuse toute pile qui chevauche celle d'une tâche non `DEAD`.

La logique pure est dans `elyon_core::task_stack`, testée sur l'hôte :

- validation de la région : non vide, alignée sur la page, taille minimale, entièrement couverte par l'identity map ;
- détection des chevauchements et position du RSP, y compris la descente dans la zone des canaris ;
- frame initial dans l'ordre de dépilement de `elyon_context_switch` : r15, r14, r13, r12 = entrée, rbp, rbx = argument, rflags, puis `ret` vers `elyon_task_trampoline`, qui transmet l'argument dans `rdi` et appelle l'entrée ;
- sommet de pile aligné sur 16, pour que l'entrée de la tâche respecte l'ABI SysV ;
- canaris de bas de pile.

La qualification crée trois tâches du registre, leur attribue chacune une pile mise à zéro et marquée de canaris, construit leur frame initial, puis les exécute deux fois chacune en coopératif sur le BSP, interruptions masquées. Avant chaque switch, le BSP vérifie que la tâche est `RUNNING` sur le CPU 0. Après chaque retour, il vérifie l'argument reçu, que les RSP observé et sauvegardé sont dans la pile de la tâche et dans aucune autre, et qu'ils restent hors de la zone des canaris. En fin de qualification, il contrôle les canaris, le RSP sauvegardé du BSP et le nombre de switches (12).

Le script `test-task-stacks-qemu.sh` ne se contente pas des marqueurs : il recalcule à partir du log que chaque RSP est dans la pile de sa tâche et que les trois piles sont deux à deux disjointes. Il a été vérifié en échec sur des copies altérées du log (pile qui chevauche une autre, RSP hors de sa pile).

La qualification a été exécutée sur le commit `01a4af9bb731`, GitHub Actions run **#25** du workflow Elyon 0.0.4, et en local sous QEMU 8.2.2 / OVMF, 4 vCPU.

Preuves :

```text
[task-stack] task=3 stack=0x000000000014e000..0x000000000015e000 size=64 KiB initial_rsp=0x000000000015dfc0 observed_rsp=0x000000000015dfd0 saved_rsp=0x000000000015df90 arg=0xe104020000 canary=OK
[task-stack] task=4 stack=0x000000000015e000..0x000000000016e000 size=64 KiB initial_rsp=0x000000000016dfc0 observed_rsp=0x000000000016dfd0 saved_rsp=0x000000000016df90 arg=0xe104020001 canary=OK
[task-stack] task=5 stack=0x000000000016e000..0x000000000017e000 size=64 KiB initial_rsp=0x000000000017dfc0 observed_rsp=0x000000000017dfd0 saved_rsp=0x000000000017df90 arg=0xe104020002 canary=OK
[task-stack] tasks=3 distinct=1 overlaps=0 owned=3 rounds=2 switches=12 args_ok=3 canaries_ok=3
ELYON_TASK_STACKS_READY
[PASS] K004-02 3 tâches sur piles PMM dédiées disjointes, frames initiaux, 2 tours, arguments et canaris validés
```

Validation croisée sous VMware® Workstation 17 Pro 17.0.2 build-21581411, UEFI, 4 vCPU, même build :

```text
[PASS] VMware 4 vCPU: Elyon 0.0.4 K003-01..12, K004-01 et K004-02 validés sur build_id=01a4af9bb731
```

Les piles, RSP et arguments relevés sous VMware sont identiques à ceux de QEMU, l'allocation du PMM étant déterministe. Le contrôle indépendant de `test-task-stacks-qemu.sh` passe aussi sur le log VMware.

Limites : le PMM n'a pas encore de libération, les piles des tâches `DEAD` restent donc allouées. La qualification ne tourne que sur le BSP ; l'ordonnancement relève de K004-03 et des critères suivants.

**K004-02 : PASS.**

Le prochain critère est **K004-03 : Round-Robin coopératif avec `yield()` et run queue.**
