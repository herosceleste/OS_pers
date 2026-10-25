# Elyon Kernel 0.0.5 — Roadmap

Nom du jalon : **Ring 3 & System Calls Foundation**

La progression reste incrémentale. Aucun critère ne passe à PASS sans code exécuté, test automatisé et preuve attachée à un commit ou à un environnement de qualification.

| Critère | Objectif | État initial |
|---|---|---|
| K005-01 | Segments utilisateur dans la GDT, `RSP0` du TSS propre à chaque tâche | À faire |
| K005-02 | `swapgs` à chaque entrée depuis le ring 3 (IRQ, exceptions), `lfence`, politique `CR4.FSGSBASE` | À faire |
| K005-03 | Entrée « paranoïaque » NMI, #DF et #MC, piles IST dédiées NMI et #MC | À faire |
| K005-04 | SMEP et SMAP actifs ; copie validée entre espace utilisateur et noyau (`stac`/`clac`) | À faire |
| K005-05 | Pages utilisateur prises par `allocate_owned`, bit U/S strict, jamais d'accès noyau par l'identity map | À faire |
| K005-06 | `syscall`/`sysret` : MSR `STAR`, `LSTAR`, `SFMASK` (IF, DF, AC), RIP canonique avant `sysret`, nettoyage des registres | À faire |
| K005-07 | Premier processus utilisateur isolé : entrée par `iretq`, appels `write`, `yield`, `exit` | À faire |
| K005-08 | Une faute en ring 3 (#PF, #GP, #UD, #DE…) tue le processus sans affecter le noyau ni les autres CPU | À faire |
| K005-09 | Timer LAPIC sur chaque AP : préemption d'une tâche ring 3 sur tous les CPU | À faire |
| K005-10 | Contre-pression : une file ou la file temporelle pleine refuse proprement au lieu d'arrêter le système | À faire |
| K005-11 | Stress SMP ring 3 / ring 0 et appels système aux arguments invalides | À faire |
| K005-12 | Qualification finale QEMU/OVMF + VMware 4 vCPU, sans régression K003 ni K004 | À faire |

## Invariants

Une entrée depuis le ring 3 ne doit jamais pouvoir arrêter le système : les fautes et les erreurs atteignables depuis un processus tuent ce processus, et l'arrêt global (`halt_system`) reste réservé aux invariants du noyau. Le noyau ne lit ni n'écrit jamais une page utilisateur hors des copies validées, et n'exécute jamais une page utilisateur. Le GS utilisateur n'est jamais utilisé par le noyau : l'index du CPU et la zone per-CPU sont lus via le GS noyau, après `swapgs`. Aucun registre noyau ne fuit vers le ring 3 à la sortie d'un appel système.

Les exigences de la section Sécurité de `ROADMAP.md`, à la racine du dépôt, sont obligatoires pour qualifier la 0.0.5. La mémoire virtuelle par processus (un CR3 par processus) relève de la 0.0.6.
