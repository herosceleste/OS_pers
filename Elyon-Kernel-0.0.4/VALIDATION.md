# Elyon Kernel 0.0.4 — Validation

**EN COURS — K004-01 à K004-06 sont validés. K004-07 à K004-12 restent à qualifier.**

## Baseline héritée

La 0.0.4 est dérivée de la Release `v0.0.3`, dont les critères K003-01 à K003-12 sont qualifiés sous QEMU/OVMF et VMware Workstation en 4 vCPU. La CI 0.0.4 doit conserver ces invariants comme régressions pendant tout le chantier scheduler.

## Critères

| Critère | Description | Statut |
|---|---|---|
| K004-01 | Modèle de tâche noyau et machine d'états | **PASS** |
| K004-02 | Pile et contexte initial dédiés | **PASS** |
| K004-03 | Round-Robin coopératif | **PASS** |
| K004-04 | Préemption LAPIC | **PASS** |
| K004-05 | Contexte CPU scheduler complet | **PASS** |
| K004-06 | Sauvegarde FPU/SSE/AVX | **PASS** |
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

## K004-03 — Round-Robin coopératif avec `yield()` et run queue

Les tâches se passent désormais la main directement, d'une tâche à l'autre. Un scheduler choisit la suivante dans une run queue FIFO.

La logique pure est dans `elyon_core::run_queue`, testée sur l'hôte :

- file FIFO circulaire de capacité fixe, sans allocation, qui refuse les doublons et l'identifiant 0 ;
- `yield_next` : la tâche courante passe en fin de file et la tête reprend la main ;
- `exit_next` : la tête reprend la main sans remettre la tâche courante en file ;
- `is_round_robin` : une trace doit former exactement N tours des tâches, dans l'ordre.

Côté noyau, le registre de tâches conserve le RSP sauvegardé de chaque tâche, écrit et relu par le switch de contexte. Le module `sched` fournit :

- `yield_now()` : la tâche courante passe `READY` et en fin de file, la tête passe `RUNNING`, et le switch se fait directement de tâche à tâche ;
- `exit_now()` : la tâche passe `DEAD` sans être remise en file ; la dernière sortie rend la main au BSP.

Avant chaque switch, `transfer()` vérifie qu'exactement une tâche est `RUNNING` (aucune au retour vers le BSP). Toute erreur dans une tâche arrête le noyau de façon fail-closed.

La qualification place trois tâches dans la run queue. Chacune effectue quatre itérations : elle note son passage dans une trace, puis appelle `yield_now()`, et `exit_now()` après la dernière. Le BSP vérifie :

- une trace en rotation stricte ;
- toutes les tâches `DEAD` et la file vide ;
- au plus une tâche `RUNNING` à tout moment ;
- 9 yields, 3 sorties, 11 switches de tâche à tâche et 2 passages par le BSP ;
- les arguments reçus, et le RSP dans la pile propre de la tâche à chaque passage ;
- les canaris et le RSP sauvegardé du BSP.

Le script `test-sched-rr-qemu.sh` recalcule la rotation à partir de la trace du log, sans se fier au seul jugement du noyau. Il a été vérifié en échec sur des traces altérées (deux passages inversés, un passage manquant).

La qualification a été exécutée sur le commit `ffa06bebafce`, GitHub Actions run **#29** du workflow Elyon 0.0.4, et en local sous QEMU 8.2.2 / OVMF, 4 vCPU.

Preuves :

```text
[sched] tasks=6,7,8 trace=6,7,8,6,7,8,6,7,8,6,7,8
[sched] policy=round-robin cooperative iterations=4 yields=9 exits=3 task_switches=11 bsp_switches=2 running_max=1 args_ok=3 rsp_ok=12 canaries_ok=3 queue_empty=1
ELYON_SCHED_RR_READY
[PASS] K004-03 Round-Robin coopératif: 3 tâches, 4 tours stricts, 11 switches directs tâche→tâche, une seule tâche RUNNING
```

Validation croisée sous VMware® Workstation 17 Pro 17.0.2 build-21581411, UEFI, 4 vCPU, même build :

```text
[PASS] VMware 4 vCPU: Elyon 0.0.4 K003-01..12 et K004-01..03 validés sur build_id=ffa06bebafce
```

Les contrôles indépendants des scripts K004-02 et K004-03 passent aussi sur le log VMware.

Limites : tout se déroule sur le BSP, interruptions masquées, avec une run queue globale sans verrou. La préemption, les files par CPU et la synchronisation SMP relèvent de K004-04, K004-07 et K004-10. Les piles des tâches `DEAD` ne sont toujours pas récupérées.

**K004-03 : PASS.**

Le prochain critère est **K004-04 : préemption réelle par timer LAPIC.**

## K004-04 — Préemption réelle par timer LAPIC

Des tâches qui n'appellent jamais `yield()` se partagent désormais le CPU : c'est l'interruption du timer LAPIC qui décide du passage de main, quand le quantum de la tâche courante est épuisé.

La logique pure est dans `elyon_core::preempt`, testée sur l'hôte :

- `Quantum` : durée d'une tranche en ticks, au moins un ;
- `SliceClock` : décision Continuer/Préempter à chaque tick ; une tranche épuisée redemande la préemption tant que le scheduler ne l'a pas remise à zéro ;
- `is_rotation` et `min_slices` : rotation stricte d'une trace et part de la tâche la moins servie.

Chemin de préemption côté noyau :

- le stub `elyon_apic_timer_interrupt` sauvegarde les quinze registres généraux ;
- `timer::on_interrupt` envoie l'EOI, puis appelle `sched::on_timer_tick` ;
- tranche épuisée, la tâche courante passe `READY` et en fin de file, la suivante passe `RUNNING`, et le switch se fait depuis l'IRQ, sur la pile de la tâche interrompue ; celle-ci reprendra plus tard par son propre `iretq` ;
- une tâche neuve démarre avec IF=0 dans son frame initial et active elle-même les interruptions.

Politique d'interruptions : `yield_now()` manipule la run queue IRQ masquées et restaure IF au retour (irq-save), `exit_now()` masque les IRQ dès son entrée. Un tick ne peut donc jamais interrompre une mise à jour de la file. Aucun verrou ne traverse un switch.

La qualification lance trois tâches en boucle de calcul pure, sans `yield`, avec un quantum d'un tick à 100 Hz. Après 30 préemptions, un drapeau d'arrêt est levé et les tâches sortent. La trace n'enregistre que les tranches antérieures à l'arrêt : elle compte exactement 31 entrées en rotation stricte, quelle que soit la cadence réelle des ticks. Le BSP vérifie :

- au moins 30 préemptions, la rotation stricte et au moins 10 tranches par tâche ;
- des compteurs de progression non nuls pour chaque tâche ;
- toutes les tâches `DEAD`, la file vide, au plus une tâche `RUNNING` et 3 sorties ;
- les arguments, les canaris et le RSP sauvegardé du BSP ;
- IF=0 au retour sur le BSP, puis l'arrêt du timer sans échec d'EOI.

Le script `test-sched-preempt-qemu.sh` recalcule la trace, la rotation et la répartition à partir du log. Les nombres de préemptions sont vérifiés comme des minimums, le timer continuant de tourner jusqu'à la sortie des tâches. Le script a été vérifié en échec sur des logs altérés (rotation rompue, tâche sans progression, `running_max=2`). La stabilité a été vérifiée sur 7 boots QEMU à 1, 2 et 4 vCPU.

La qualification a été exécutée sur le commit `13271ff4d25b`, GitHub Actions run **#33** du workflow Elyon 0.0.4, et en local sous QEMU 8.2.2 / OVMF, 4 vCPU.

Preuves :

```text
[preempt] tasks=9,10,11 trace=9,10,11,9,10,11,9,10,11,9,10,11,9,10,11,9,10,11,9,10,11,9,10,11,9,10,11,9,10,11,9
[preempt] policy=timer-preemptive quantum_ticks=1 target=30 preemptions=30 task_ticks=30 min_slices=10 counters=594299,574945,594274 exits=3 running_max=1 eoi_failures=0 canaries_ok=3
ELYON_SCHED_PREEMPT_READY
[PASS] K004-04 préemption LAPIC: 3 tâches sans yield, 30 préemptions timer, 31 tranches en rotation stricte, une seule tâche RUNNING
```

Validation croisée sous VMware® Workstation 17 Pro 17.0.2 build-21581411, UEFI, 4 vCPU, même build :

```text
[PASS] VMware 4 vCPU: Elyon 0.0.4 K003-01..12 et K004-01..04 validés sur build_id=13271ff4d25b
[preempt] policy=timer-preemptive quantum_ticks=1 target=30 preemptions=30 task_ticks=30 min_slices=10 counters=3630849,3669487,3718779 exits=3 running_max=1 eoi_failures=0 canaries_ok=3
```

Sous VMware, le timer est asynchrone par rapport à l'exécution virtualisée des tâches : les compteurs y sont environ six fois plus élevés que sous TCG, et la trace est identique.

Limites : la cible `x86_64-unknown-none` compile sans SSE, aucun état FPU n'est donc en jeu ; l'isolation FPU/SSE/AVX relève de K004-06. La préemption ne tourne que sur le BSP avec une run queue globale ; le per-CPU, l'IPI de rescheduling et la synchronisation SMP relèvent de K004-07, K004-08 et K004-10.

**K004-04 : PASS.**

Le prochain critère est **K004-05 : sauvegarde/restauration complète du contexte CPU requis par le scheduler.**

## K004-05 — Contexte CPU complet préservé par le scheduler

Le contexte qu'une tâche doit retrouver intact se compose des quinze registres généraux, de RSP, de RIP et de RFLAGS, y compris les bits qu'elle contrôle elle-même comme DF. Elyon n'utilise qu'un espace d'adressage (un seul CR3), pas de base FS/GS et pas de SSE : aucun autre état n'est en jeu à ce stade.

Ce contexte est sauvegardé par deux chemins :

- **préemption** : le stub d'IRQ timer sauvegarde les quinze registres généraux, le CPU empile RIP, CS, RFLAGS, RSP et SS, et la tâche reprend par `iretq` ;
- **switch coopératif** : `elyon_context_switch` sauvegarde les registres callee-saved, RFLAGS et RSP ; les registres caller-saved n'ont pas à survivre à un appel selon l'ABI SysV.

La logique pure est dans `elyon_core::cpu_context`, testée sur l'hôte : liste des registres et de ceux que préserve chaque chemin, motifs 64 bits uniques par tâche et par registre, et rapport de corruption registre par registre et pour DF.

La qualification fait tourner trois tâches sous préemption timer (quantum d'un tick, 30 préemptions). Chacune exécute `elyon_context_integrity_loop`, une boucle en assembleur qui charge son motif dans les quinze registres généraux, positionne DF, puis revérifie chaque registre et DF à chaque itération, jusqu'au drapeau d'arrêt. Au premier écart, la boucle écrit l'état observé et le noyau s'arrête fail-closed en nommant le registre corrompu. Les motifs étant propres à chaque tâche, un registre qui passerait d'une tâche à une autre est détecté de la même façon.

Le BSP vérifie en plus qu'au moins 10 préemptions par tâche ont eu lieu pendant la boucle de vérification, que chaque tâche a effectivement itéré, ainsi que la rotation, les états, l'invariant `RUNNING`, les canaris et l'absence d'échec d'EOI.

La vérification a été confrontée à des fautes réelles. Deux injections locales, non conservées dans le code, modifient le stub d'IRQ timer pour corrompre le contexte sauvegardé juste avant `iretq` :

```text
r9 ^= 1   → [context-integrity] FAILED task_index=0 register=r9 expected=0xe10500095a5a0008 observed=0xe10500095a5a0009 mask=0x0100 df_lost=0
DF effacé → [context-integrity] FAILED task_index=0 df_lost=1 rflags=0x246
```

Dans les deux cas, la qualification K004-04 passait encore : c'est ce critère qui établit l'intégrité du contexte. Le script `test-sched-context-qemu.sh` rejette le log réel de l'injection ainsi que des logs altérés (préemptions en boucle insuffisantes, `corruptions=1`, itérations nulles).

La qualification a été exécutée sur le commit `6daf803f0dcc`, GitHub Actions run **#37** du workflow Elyon 0.0.4, et en local sous QEMU 8.2.2 / OVMF, 4 vCPU.

Preuves :

```text
[context-integrity] tasks=12,13,14 registers=15 df=1 preemptions=30 min_slices=10 busy_preemptions=10,10,10 iterations=3100652,3196159,3177120 corruptions=0 exits=3 running_max=1 eoi_failures=0 canaries_ok=3
ELYON_SCHED_CONTEXT_READY
[PASS] K004-05 contexte CPU: 15 registres + DF intacts sous préemption, 10,10,10 préemptions en boucle, aucune corruption
```

Validation croisée sous VMware® Workstation 17 Pro 17.0.2 build-21581411, UEFI, 4 vCPU, même build :

```text
[PASS] VMware 4 vCPU: Elyon 0.0.4 K003-01..12 et K004-01..05 validés sur build_id=6daf803f0dcc
[context-integrity] tasks=12,13,14 registers=15 df=1 preemptions=30 min_slices=10 busy_preemptions=10,10,10 iterations=3426121667,20693287,35121995 corruptions=0 exits=3 running_max=1 eoi_failures=0 canaries_ok=3
```

Environ 3,5 milliards de vérifications sous virtualisation matérielle, sans corruption. Sur ce boot, la durée réelle des tranches a été très inégale (de 20 millions à 3,4 milliards d'itérations par tâche), signe de ticks délivrés avec retard, vraisemblablement par mise en pause du vCPU côté hôte ; la rotation est restée exacte. L'équité en temps réel n'est pas un critère de K004-05.

Limites : pas d'état FPU/SSE (K004-06), un seul espace d'adressage, pas de ring 3, BSP uniquement.

**K004-05 : PASS.**

Le prochain critère est **K004-06 : isolation FPU/SSE/AVX par tâche via XSAVE/XRSTOR ou fallback qualifié.**

## K004-06 — Isolation FPU/SSE/AVX par tâche

Avant ce critère, `CR4=0x668` portait déjà OSFXSR et OSXMMEXCPT, hérités de l'UEFI, et `CR0` avait EM=0 et TS=0 : les instructions SSE s'exécutaient sans piège, mais sans aucune isolation entre tâches. Le noyau étant compilé en soft-float, seul le code des tâches touche aux registres x87/SSE/AVX.

La logique pure est dans `elyon_core::fpu`, testée sur l'hôte :

- `choose()` : XSAVE avec x87|SSE, plus AVX seulement si CPUID et la feuille XSAVE le rapportent, jamais de composant au-delà d'AVX ; repli FXSAVE sinon ; refus des CPU sans FXSR ou SSE2 ;
- `area_layout()` : 512 octets alignés sur 16 pour FXSAVE, taille CPUID entre 576 et 4096 octets alignée sur 64 pour XSAVE ;
- `write_init_image()` : FCW `0x037F`, MXCSR `0x1F80`, en-tête XSAVE vide ;
- motifs XMM, moitiés hautes YMM et MXCSR propres à chaque tâche, et comparaison.

Côté noyau :

- activation : CR0 (EM=0, MP=1, TS=0), CR4 (OSFXSR, OSXMMEXCPT, et OSXSAVE si XSAVE est retenu), `xsetbv` puis relecture de XCR0 par `xgetbv` ; taille de zone lue dans CPUID.0xD ;
- chaque tâche reçoit une zone de sauvegarde PMM copiée depuis l'image initiale ; le BSP a la sienne ;
- `transfer()` sauvegarde l'état SIMD du contexte sortant et restaure celui du contexte entrant à chaque switch (stratégie eager) ; une zone manquante arrête le noyau fail-closed ;
- un build avec la variable `ELYON_FPU_FORCE_FXSAVE` définie force le repli FXSAVE, pour le qualifier sur un CPU qui dispose de XSAVE.

La qualification fait tourner trois tâches sous préemption timer (quantum d'un tick, 30 préemptions). Chacune exécute `elyon_simd_integrity_loop`, qui charge son MXCSR et ses motifs dans XMM0..15, ainsi que dans les moitiés hautes de YMM0..15 si AVX est actif, puis les revérifie à chaque itération. Au premier écart, le noyau s'arrête fail-closed en nommant le registre (`xmmN`, `ymmN_high` ou `mxcsr`). Le BSP vérifie en plus au moins 10 préemptions par tâche pendant la boucle, la progression, la rotation, les états, l'invariant `RUNNING`, les canaris et l'absence d'échec d'EOI.

La vérification a été confrontée à une faute réelle : une injection locale, non conservée dans le code, désactive la sauvegarde et la restauration dans `transfer()`.

```text
xsave  → [fpu-integrity] FAILED task_index=0 register=xmm0 expected=0xf1e600015a5a0000f0e600015a5a0000 observed=0xf1e602015a5a0200f0e602015a5a0200 xmm_mask=0xffff ymm_mask=0xffff
fxsave → [fpu-integrity] FAILED task_index=0 register=xmm0 … xmm_mask=0xffff ymm_mask=0x0000
```

La tâche 0 y lit exactement les motifs de la tâche 2 dans tous ses registres : c'est la fuite que ce critère empêche. K004-05 passait encore. Le script `test-sched-fpu-qemu.sh` exige XSAVE quand le CPU le propose, ou le repli FXSAVE en build forcé ; il rejette les deux logs réels d'injection, un log par défaut présenté comme FXSAVE forcé et un XCR0 débordant (`0x27`). La CI l'exécute sur les deux chemins.

La qualification a été exécutée sur le commit `3063cd63747b`, GitHub Actions run **#41** du workflow Elyon 0.0.4, et en local sous QEMU 8.2.2 / OVMF, 4 vCPU, sur les deux chemins.

Preuves :

```text
[fpu] fxsr=1 sse2=1 xsave=1 avx=1 xcr0_supported=0x21f strategy=xsave xcr0=0x7 avx_enabled=1 area=832 align=64 forced_fxsave=0 cr0=0x80010033 cr4=0x40668
[fpu-integrity] tasks=15,16,17 strategy=xsave avx=1 xmm=16 ymm_high=16 mxcsr=1 preemptions=30 min_slices=10 busy_preemptions=10,10,10 iterations=384308,405060,395278 corruptions=0 exits=3 running_max=1 eoi_failures=0 canaries_ok=3
ELYON_SCHED_FPU_READY
[PASS] K004-06 isolation FPU par tâche (xsave, avx=1): xmm=16 ymm_high=16 mxcsr intacts sous préemption, 10,10,10 préemptions en boucle, aucune corruption
[PASS] K004-06 isolation FPU par tâche (fxsave, avx=0): xmm=16 ymm_high=0 mxcsr intacts sous préemption, 10,10,10 préemptions en boucle, aucune corruption
```

Validation croisée sous VMware® Workstation 17 Pro 17.0.2 build-21581411, UEFI, 4 vCPU, même build :

```text
[PASS] VMware 4 vCPU: Elyon 0.0.4 K003-01..12 et K004-01..06 validés sur build_id=3063cd63747b
[fpu] fxsr=1 sse2=1 xsave=1 avx=1 xcr0_supported=0xe7 strategy=xsave xcr0=0x7 avx_enabled=1 area=832 align=64 forced_fxsave=0 cr0=0x80010033 cr4=0x40668
[fpu-integrity] tasks=15,16,17 strategy=xsave avx=1 xmm=16 ymm_high=16 mxcsr=1 preemptions=30 min_slices=10 busy_preemptions=10,10,10 iterations=5185283,5227557,5137738 corruptions=0 exits=3 running_max=1 eoi_failures=0 canaries_ok=3
```

VMware expose AVX-512 (`xcr0_supported=0xe7` : opmask, ZMM_Hi256, Hi16_ZMM) ; seuls x87|SSE|AVX (`0x7`) sont activés et la zone reste à 832 octets.

Limites : sauvegarde systématique à chaque switch, sans mode lazy (CR0.TS) ; AVX-512 volontairement non pris en charge ; le noyau lui-même n'utilise pas le SIMD ; BSP uniquement ; le repli FXSAVE n'est exercé qu'en build forcé, QEMU et VMware proposant tous deux XSAVE.

**K004-06 : PASS.**

Le prochain critère est **K004-07 : scheduler per-CPU et idle task officielle sur chaque CPU.**
