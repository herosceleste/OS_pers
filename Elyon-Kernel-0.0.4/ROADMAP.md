# Elyon Kernel 0.0.4 — Roadmap

Nom du jalon : **Scheduler & Kernel Tasks Foundation**

La progression reste incrémentale. Aucun critère ne passe à PASS sans code exécuté, test automatisé et preuve attachée à un commit ou à un environnement de qualification.

| Critère | Objectif | État initial |
|---|---|---|
| K004-01 | Modèle de tâche noyau : Task ID, états READY/RUNNING/BLOCKED/SLEEPING/DEAD | À faire |
| K004-02 | Pile PMM dédiée et contexte initial propre par tâche | À faire |
| K004-03 | Round-Robin coopératif avec yield() et run queue | À faire |
| K004-04 | Préemption réelle par timer LAPIC | À faire |
| K004-05 | Sauvegarde/restauration complète du contexte CPU requis par le scheduler | À faire |
| K004-06 | Isolation FPU/SSE/AVX par tâche via XSAVE/XRSTOR ou fallback qualifié | À faire |
| K004-07 | Scheduler per-CPU et idle task officielle sur chaque CPU | À faire |
| K004-08 | IPI de rescheduling, réveil distant et affinité CPU | À faire |
| K004-09 | sleep()/wakeup et file temporelle | À faire |
| K004-10 | Spinlocks/irq-save et invariants de synchronisation scheduler/IRQ | À faire |
| K004-11 | Stress SMP avec nombreuses tâches et volume élevé de context switches | À faire |
| K004-12 | Qualification finale QEMU/OVMF + VMware 4 vCPU, sans régression K003 | À faire |

## Invariants

Une tâche ne doit jamais être RUNNING simultanément sur deux CPU. Une pile de tâche ne peut appartenir qu'à une tâche active. Un changement de contexte ne doit jamais s'effectuer en tenant un verrou non prévu pour traverser le switch. Les chemins IRQ, scheduler et run queue doivent expliciter leur politique d'interruption et de verrouillage.

La préemption ne sera activée qu'après qualification du Round-Robin coopératif. L'état SIMD/FPU sera isolé avant de considérer le scheduler préemptif comme complet.
