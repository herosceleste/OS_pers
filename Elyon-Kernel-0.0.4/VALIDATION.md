# Elyon Kernel 0.0.4 — Validation

**EN COURS — K004-01 à K004-11 sont validés. K004-12 reste à qualifier.**

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
| K004-07 | Scheduler per-CPU + idle task | **PASS** |
| K004-08 | Reschedule IPI + affinité | **PASS** |
| K004-09 | Sleep/wakeup | **PASS** |
| K004-10 | Synchronisation scheduler/IRQ/SMP | **PASS** |
| K004-11 | Stress SMP | **PASS** |
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

## K004-07 — Scheduler per-CPU et idle task officielle sur chaque CPU

Chaque CPU en ligne a désormais son propre scheduler, et des tâches Elyon s'exécutent sur les AP, chacune sur le CPU désigné par son affinité. Quand sa file est vide, chaque CPU exécute son idle task officielle.

La logique pure est dans `elyon_core::percpu_sched`, testée sur l'hôte :

- `place()` : plus petit CPU en ligne de l'affinité ; une affinité vide ou sans CPU en ligne est refusée ;
- `check_trace()` : chaque passage a lieu sur le CPU propriétaire de la tâche (idle tasks comprises), chaque CPU exécute exactement N tours de sa file dans l'ordre, et chaque CPU exécute son idle task ; l'entrelacement entre CPU est libre.

Côté noyau :

- l'état du scheduler (run queue, RSP du contexte de départ, tâche courante, horloge de tranche, zone FPU) est indexé par CPU logique, retrouvé à partir de l'APIC ID ; l'invariant `RUNNING` est vérifié par CPU ;
- CR4.OSXSAVE et XCR0 étant propres à chaque CPU, `fpu::init_secondary()` applique sur chaque AP la configuration retenue par le BSP, relue par `xgetbv` ; les piles et zones FPU sont pré-allouées par le BSP, le PMM n'étant pas verrouillé ;
- les AP attendent en `sti; hlt` ; le BSP remplit leur file, publie `PERCPU_ARMED[cpu]` (Release) puis les réveille par IPI ; chaque file n'est ensuite touchée que par son CPU ;
- une file vide bascule sur l'idle task du CPU : sur un AP elle tourne indéfiniment en `sti; hlt`, sur le BSP elle repasse `READY` et rend la main au boot.

La qualification place deux tâches par CPU, qui effectuent chacune trois itérations coopératives. Le BSP vérifie la trace multi-CPU, les tâches `DEAD`, les idle tasks des AP `RUNNING` sur leur propre CPU et celle du BSP `READY`, le FPU de chaque AP, au plus une tâche `RUNNING` par CPU et les canaris des douze piles.

Le script `test-sched-percpu-qemu.sh` revérifie à partir du log que chaque CPU a exécuté exactement `A,B,A,B,A,B` puis son idle task, et qu'aucune tâche n'apparaît sur deux CPU. Il a été vérifié en échec sur des logs altérés (tâche migrée, ordre rompu, idle task manquante, FPU d'un AP non prête). Le même contrôle s'applique au log VMware. La stabilité a été vérifiée sur six boots QEMU à 1, 2 et 4 vCPU.

La qualification a été exécutée sur le commit `3af57041527a`, GitHub Actions run **#48** du workflow Elyon 0.0.4, et en local sous QEMU 8.2.2 / OVMF, 4 vCPU.

Preuves :

```text
[percpu-sched] cpu=0 idle=18 tasks=19,20 runs=19,20,19,20,19,20,18
[percpu-sched] cpu=1 idle=21 tasks=22,23 runs=22,23,22,23,22,23,21
[percpu-sched] cpu=2 idle=24 tasks=25,26 runs=25,26,25,26,25,26,24
[percpu-sched] cpu=3 idle=27 tasks=28,29 runs=28,29,28,29,28,29,27
[percpu-sched] cpus=4 tasks_per_cpu=2 iterations=3 trace_entries=28 affinity_ok=1 rotation_ok=1 idle_ran=4 idle_running_aps=3 ap_fpu_ready=3 running_max=1 canaries_ok=12
ELYON_SCHED_PERCPU_READY
[PASS] K004-07 scheduler per-CPU: 4 CPU, 2 tâches par CPU en Round-Robin sur leur seul CPU, idle officielle sur chaque CPU (3 AP en idle), FPU des AP prête
```

Validation croisée sous VMware® Workstation 17 Pro 17.0.2 build-21581411, UEFI, 4 vCPU, même build :

```text
[PASS] VMware 4 vCPU: Elyon 0.0.4 K003-01..12 et K004-01..07 validés sur build_id=3af57041527a
```

Les traces par CPU relevées sous VMware sont identiques à celles de QEMU, et la console série ne contient aucune ligne entremêlée alors que les quatre CPU s'exécutaient en parallèle.

Limites : l'exécution est coopérative sur les AP, dont les timers LAPIC ne sont pas programmés ; les files sont remplies avant l'armement, sans migration ni réveil à distance (K004-08) ; seul le BSP alloue, le PMM n'étant pas verrouillé.

**K004-07 : PASS.**

Le prochain critère est **K004-08 : IPI de rescheduling, réveil distant et affinité CPU.**

## K004-08 — IPI de rescheduling, réveil à distance et affinité CPU

Un CPU peut désormais placer du travail sur un autre CPU déjà en marche : création d'une tâche sur un AP en idle, réveil d'une tâche bloquée sur un autre CPU, migration d'une tâche `READY` quand son affinité change. Le CPU cible est prévenu par un IPI de rescheduling.

La logique pure est dans `elyon_core`, testée sur l'hôte :

- `sync::RawSpinLock` : spinlock sans donnée et sans `unsafe` ; avec 8 threads réels effectuant chacun 20 000 lectures-écritures non atomiques sous le verrou, aucune mise à jour n'est perdue ;
- `RunQueue::remove()` : retrait d'une tâche pour migration, l'ordre des autres étant conservé ;
- `percpu_sched` : `should_send_ipi()` (jamais vers soi-même), `migrate()` (la tâche reste, change de file ou est placée ; masque vide ou hors ligne refusé), `check_placement()` (chaque passage sur le CPU attendu, nombre exact de passages par tâche).

Côté noyau :

- chaque file par CPU est protégée par un spinlock, pris IRQ masquées par un garde qui le relâche avant tout switch ; jamais deux verrous de file à la fois ;
- le vecteur `0xF2` a son propre stub ; le handler envoie l'EOI puis, si le CPU exécute son idle task et que sa file n'est pas vide, l'idle repasse `READY` et la tâche prend le CPU depuis l'IRQ ; l'idle reprend plus tard au même point et sort par `iretq` ;
- `enqueue()`, `wake()` et `set_affinity()` placent la tâche selon son affinité et n'envoient l'IPI qu'à un autre CPU ;
- `block_now()` passe la tâche à `BLOCKED` et choisit la suivante sous le verrou de la file du CPU : un réveil concurrent, qui doit prendre ce verrou pour remettre la tâche en file, ne peut pas s'intercaler ;
- l'idle task du BSP rend la main au boot à chaque fois que la file du BSP se vide.

La qualification, exécutée après K004-07 alors que les AP sont dans leur idle task, enchaîne trois scénarios :

1. une tâche placée sans IPI sur l'AP 1 change d'affinité vers le CPU 0 : elle est retirée de la file de l'AP 1, et le BSP l'exécute ;
2. une tâche est créée sur l'AP 1 avec IPI : l'idle de l'AP 1 lui cède le CPU ;
3. une tâche placée sur l'AP 2 se bloque ; le BSP attend qu'elle soit `BLOCKED`, la réveille, et elle reprend sur l'AP 2.

Le BSP vérifie le placement et le nombre de passages de chaque tâche, 3 IPI envoyés et autant de reçus, les tâches `DEAD`, le retour des AP dans leur idle task, au plus une tâche `RUNNING` par CPU et les canaris.

Deux injections de faute ont été rejetées : un handler d'IPI qui ne change pas de tâche, et une migration qui ne retire pas la tâche de sa file d'origine. Le script `test-sched-ipi-qemu.sh` revérifie à partir du log les CPU de chaque scénario, l'unicité des tâches par rapport à K004-07, la synthèse et l'ordre des marqueurs ; 13 copies altérées d'un log réel sont toutes refusées. Le même contrôle s'applique au log VMware. K004-03 à K004-07 ont été rejoués après le verrouillage des files.

La qualification a été exécutée sur le commit `bfec2994a411`, GitHub Actions run **#52** du workflow Elyon 0.0.4, et en local sous QEMU 8.2.2 / OVMF, 4 vCPU.

Preuves :

```text
[sched-ipi] remote_spawn task=31 cpu=1 runs=1 ipi=1
[sched-ipi] block_wake task=32 cpu=2 runs=2 blocked_then_woken_by_cpu=0
[sched-ipi] migrate task=30 from=1 to=0 runs=1
[sched-ipi] cpus=4 placement_ok=1 ipi_sent=3 ipi_received=3 blocks=1 wakes=1 migrations=1 contention=0 idle_running_aps=3 running_max=1 canaries_ok=3
ELYON_SCHED_IPI_READY
[PASS] K004-08 rescheduling à distance: 4 CPU, création sur un AP en idle par IPI, blocage puis réveil depuis le BSP, migration AP 1 -> CPU 0, IPI envoyés=reçus=3, 3 AP revenus en idle
```

Validation croisée sous VMware® Workstation 17 Pro 17.0.2 build-21581411, UEFI, 4 vCPU, même build :

```text
[PASS] VMware 4 vCPU: Elyon 0.0.4 K003-01..12 et K004-01..08 validés sur build_id=bfec2994a411
```

Les lignes `[sched-ipi]` relevées sous VMware sont identiques à celles de QEMU.

Limites : seules les tâches `READY` en file migrent, pas une tâche en cours d'exécution ; l'IPI ne fait changer de tâche qu'un CPU en idle, sans préempter une tâche en cours ; aucune contention n'a été observée sur les verrous de file (`contention=0`), qui seront mis sous charge par K004-10 et K004-11 ; seul le BSP alloue, le PMM n'étant pas verrouillé.

**K004-08 : PASS.**

Correction apportée pendant K004-09 : la qualification exigeait autant d'IPI reçus que d'envoyés. Or le LAPIC ne garde qu'une interruption en attente par vecteur, et un IPI de rescheduling envoyé pendant qu'un autre attend est absorbé par lui ; le premier boot local à 2 vCPU après le passage de K004-08 a échoué ainsi, toutes les tâches ayant pourtant terminé. Depuis le commit `44fb34e14b58`, le critère est : 3 IPI envoyés exactement, `1 ≤ reçus ≤ envoyés`, avec `ipi_coalesced` affiché ; l'achèvement de toutes les tâches prouve qu'aucun réveil n'est perdu. Les résultats ci-dessus, sans fusion, restent valables.

Le prochain critère est **K004-09 : sleep/wakeup.**

## K004-09 — Sleep/wakeup et file temporelle

Une tâche peut désormais s'endormir pour une durée donnée, sur n'importe quel CPU, et être réveillée à son échéance par le timer, ou plus tôt par un réveil explicite.

La logique pure est dans `elyon_core::timer_queue`, testée sur l'hôte :

- `TimerQueue<N>` : échéances triées ; à échéance égale, l'ordre d'arrivée est conservé ; `cancel()` retire l'échéance d'une tâche réveillée explicitement ;
- `deadline_for()` : arrondi au tick supérieur, plus un tick, parce que le tick courant est déjà entamé ; sans cette marge, une tâche pourrait se réveiller jusqu'à une période trop tôt ;
- `check_sleep()` : un seul réveil par sommeil ; réveils par le timer jamais en avance, au plus N ticks en retard et dans l'ordre des échéances ; réveil explicite strictement avant l'échéance.

Côté noyau :

- une file temporelle globale, protégée par son propre verrou, est vidée par le tick du BSP, seul timer programmé ; une échéance atteinte réveille la tâche par le chemin de K004-08 : retour dans la file de son CPU, puis IPI ;
- `sleep_ms()` passe la tâche à `SLEEPING`, inscrit son échéance et choisit la suivante sous le verrou de la file du CPU ; ordre des verrous : file du CPU puis file temporelle, et le tick relâche la file temporelle avant chaque réveil ; `sleep_ms(0)` cède seulement le CPU ;
- `wake()` annule d'abord l'échéance en attente : le timer ne peut plus réveiller la tâche une seconde fois ; une échéance dont la tâche ne dort plus est comptée comme périmée.

La qualification, exécutée après K004-08 avec le timer du BSP relancé à 100 Hz, enchaîne quatre scénarios :

1. trois tâches dorment 30, 10 et 20 ms sur trois AP et se réveillent dans l'ordre des échéances ;
2. deux tâches partagent l'AP 1 : la seconde tourne pendant que la première dort, puis l'AP repasse en idle quand les deux dorment ;
3. une tâche endormie pour 300 ms est réveillée par le BSP au bout de 5 ticks ; le BSP attend ensuite au-delà de l'échéance annulée pour vérifier qu'elle n'expire pas ;
4. `sleep_ms(0)` laisse la tâche éveillée.

Le BSP vérifie la trace avec `check_sleep()` (retard maximal de 2 ticks), 6 sommeils, 5 réveils par le timer et 1 explicite, aucune échéance périmée, une file temporelle vide, 12 IPI envoyés, les tâches `DEAD`, le retour des AP en idle et les canaris.

Deux injections de faute ont été rejetées : une expiration un tick trop tôt (`Early`) et un réveil explicite qui n'annule pas l'échéance (`WakeCount`, la tâche étant réveillée deux fois). Le script `test-sched-sleep-qemu.sh` recalcule chaque échéance à partir du début du sommeil, de la durée et de la fréquence du timer ; 21 copies altérées d'un log réel sont toutes refusées, et un log avec des IPI fusionnés est accepté.

La suite CI locale a révélé, à 2 vCPU, la fusion d'IPI décrite dans la correction de K004-08. Après correction, 12 boots à 2 vCPU, avec l'hôte chargé à 100 %, ont tous réussi, dont 5 avec 1 ou 2 IPI fusionnés.

La qualification a été exécutée sur le commit `44fb34e14b58`, GitHub Actions run **#56** du workflow Elyon 0.0.4, et en local sous QEMU 8.2.2 / OVMF, 4 vCPU.

Preuves :

```text
[sched-sleep] timed task=33 cpu=1 ms=30 start=190 deadline=194 woke_at=194 resumed_at=194
[sched-sleep] timed task=34 cpu=2 ms=10 start=190 deadline=192 woke_at=192 resumed_at=192
[sched-sleep] timed task=35 cpu=3 ms=20 start=190 deadline=193 woke_at=193 resumed_at=193
[sched-sleep] shared cpu=1 sleeper=36 runner=37 ran_while_sleeping=1 idle_entries=1
[sched-sleep] explicit task=38 cpu=2 ms=300 start=203 deadline=234 woke_at=209 stale_expiries=0
[sched-sleep] zero task=39 stayed_awake=1
[sched-sleep] cpus=4 hz=100 sleeps=6 timer_wakes=5 explicit_wakes=1 order_ok=1 never_early=1 max_late=0 ipi_sent=12 ipi_received=12 ipi_coalesced=0 timer_queue_empty=1 idle_running_aps=3 running_max=1 canaries_ok=7
ELYON_SCHED_SLEEP_READY
[PASS] K004-09 sleep/wakeup: 4 CPU, sommeils de 30/10/20 ms réveillés dans l'ordre jamais en avance (retard max 0 tick), CPU partagé, réveil explicite sans échéance périmée, sleep(0), IPI envoyés=12 reçus=12 (fusionnés=0)
```

Validation croisée sous VMware® Workstation 17 Pro 17.0.2 build-21581411, UEFI, 4 vCPU, même build :

```text
[PASS] VMware 4 vCPU: Elyon 0.0.4 K003-01..12 et K004-01..09 validés sur build_id=44fb34e14b58
```

Les lignes `[sched-sleep]` relevées sous VMware sont identiques à celles de QEMU, ticks compris.

Limites : la résolution est celle du tick à 100 Hz, soit 10 ms ; seul le timer du BSP est programmé, donc tous les réveils partent du BSP ; la file temporelle est limitée à 16 échéances ; un réveil explicite lancé depuis un AP pendant que le tick du BSP expire la même échéance renverrait une erreur à l'un des deux appelants, ce cas n'étant pas exercé ; seul le BSP alloue, le PMM n'étant pas verrouillé.

**K004-09 : PASS.**

Le prochain critère est **K004-10 : synchronisation scheduler/IRQ/SMP.**

## K004-10 — Spinlocks irq-save et synchronisation scheduler/IRQ/SMP

Les quatre CPU partagent désormais le scheduler, la file temporelle et le PMM sous des verrous qui masquent les interruptions et respectent un ordre fixe ; un AP peut allouer.

La logique pure est dans `elyon_core::lock_order`, testée sur l'hôte :

- rangs `Pmm < RunQueue < TimerQueue < Data` : `check_acquire()` refuse une inversion ou une double prise, `release()` refuse de relâcher un verrou non tenu ;
- `check_switch()` : un changement de contexte n'est permis qu'avec aucun verrou tenu et les interruptions masquées ;
- 9 tests, dont 4 threads réels × 20 000 tours sans interblocage.

Côté noyau :

- `IrqSpinLock<T>` (`sync.rs`) masque les IRQ, contrôle l'ordre des classes tenues par le CPU, et seul le dernier verrou relâché restaure IF ; le garde est `!Send` ;
- l'index du CPU est lu via GS dans une zone per-CPU, recoupé avec l'APIC ID, sans repli sur le slot 0 ;
- `transfer()` vérifie `check_switch()` et réclame l'indicateur `on_cpu` de la tâche suivante par `compare_exchange` ; le contexte suivant l'efface (`finish_switch`). Une tâche ne peut plus tourner sur deux CPU ; un réveil, un déplacement d'affinité ou une remise en file d'une tâche mal placée attend d'abord, de façon bornée, que l'indicateur retombe ;
- invariant d'affinité : une tâche dans la file du CPU c a toujours c dans son masque ; chaque mise en file relit le masque sous le verrou de la file cible, et une tâche qui cède un CPU exclu de son masque migre après le switch ;
- `wake()` est transactionnel et typé : `Woken` ou `NotAsleep`, et l'erreur `WakeDead` pour une tâche terminée. Chaque attente, sommeil ou blocage, arme un ticket avant le changement d'état ; seul celui qui le réclame, réveil explicite ou tick à l'échéance, réveille la tâche, après qu'elle a quitté son CPU. Un réveil tardif ne peut donc atteindre l'attente suivante, ni une échéance périmée réveiller la tâche ;
- PMM : jeton `FrameRange` qui interdit à la compilation la double libération, bitmap des pages libérables, effacement de chaque page allouée, recherche next-fit ;
- toute violation de verrou, tout `fail()` et toute exception CPU arrêtent tous les CPU par NMI (`fatal::halt_system`).

La qualification, exécutée après K004-09 sur les 4 CPU en même temps, enchaîne cinq scénarios :

1. six tâches incrémentent un compteur partagé 20 000 fois chacune sous le verrou `Data` ;
2. trois tâches sur trois AP allouent 64 pages chacune : aucun doublon, chaque page effacée, toutes rendues ;
3. ping-pong de réveils entre les AP 1 et 2, 50 allers-retours ;
4. trois tâches réveillent la même tâche bloquée : une seule gagne, la cible ne tourne qu'une fois puis, les trois tentatives rendues, se bloque à nouveau ; elle doit rester bloquée, aucun réveil de la course ni ticket résiduel n'atteignant cette seconde attente, et seul le BSP la libère ;
5. tempête : six tâches font 300 tours, cèdent le CPU deux tours sur trois et se bloquent au troisième, pendant que le BSP change en boucle leur affinité d'un AP à l'autre et les réveille ; elles doivent toutes terminer et changer réellement de CPU.

Le BSP vérifie le total du compteur, les 192 allocations sans doublon ni page non effacée et le retour du nombre de pages libres, les 100 réveils du ping-pong, 1 gagnant et 2 perdants avec la seconde attente intacte, des migrations effectives, une prise du verrou `Data` par incrément, les tâches `DEAD`, le retour des AP en idle, une seule tâche en cours par CPU et les canaris ; le script recalcule ces valeurs à partir du nombre d'AP.

Quatre injections de faute ont été détectées : ordre des verrous inversé (`Inversion`, puis arrêt des 3 autres CPU), verrou de file tenu pendant un switch (`LockHeld`), compteur sans verrou (36 à 61 % de mises à jour perdues) et retrait de l'attente `on_cpu` (« switching into a task still running on another CPU », 3 boots sur 3). Le script `test-sched-sync-qemu.sh` refuse 29 copies altérées d'un log réel et accepte 2 variantes légitimes. Sur le commit `0670a6e`, 12 boots sous charge, à 2 et 4 vCPU avec l'hôte à 100 %, ont tous réussi K004-08, K004-09 et K004-10, et K004-02 à K004-09 ont été rejoués après chaque lot de corrections. Sur le commit `0c2911c`, après le correctif du ticket d'attente : suite CI locale de 23 tests QEMU réussie, puis 36 boots répétés de K004-08, K004-09 et K004-10 à 4 et 2 vCPU, dont 16 avec l'hôte à 100 % ; 35 ont réussi. Les 11 boots qui ont atteint K004-10 l'ont tous validé, avec un seul gagnant à chaque course au réveil. Le seul échec, `[smp] STABILITY FAILED reason=AP stability heartbeat missing`, est survenu avant le scheduler, dans le contrôle de stabilité de K003, après la fin de la charge synthétique.

Le code a été relu par les agents ECC rust-reviewer, security-reviewer, silent-failure-hunter et performance-optimizer, puis par une seconde passe rust et sécurité. Une troisième passe indépendante (rust-reviewer, architect, security-reviewer) a trouvé la course au réveil corrigée par le ticket d'attente : un réveil explicite pouvait lire le ticket avant qu'il soit armé et laisser une échéance périmée, et un réveilleur tardif pouvait atteindre l'attente suivante. Le correctif a été relu une quatrième fois sans point bloquant. Toutes les corrections figurent dans la PR #20.

La qualification a été exécutée sur le commit `0c2911c70ccd`, GitHub Actions run **#61** du workflow Elyon 0.0.4, et en local sous QEMU 8.2.2 / OVMF, 4 vCPU, hôte chargé.

Preuves :

```text
[sched-sync] counter tasks=6 rounds=20000 total=120000 expected=120000
[sched-sync] pmm tasks=3 rounds=64 allocations=192 duplicates=0 failures=0 dirty=0 free_before=249258 free_after=249258
[sched-sync] pingpong ping_cpu=1 pong_cpu=2 rounds=50 wakes=100 retries=1
[sched-sync] wake_race target=51 wakers=3 wins=1 losses=2 target_runs=1
[sched-sync] storm tasks=6 rounds=300 migrations=675 moves=877 deferred=139 misplaced=11 forced=114 wakes=600 on_cpu_waits=77
[sched-sync] locks acquisitions=426,17712,117,120000 contention=3,16213,0,204797 classes=pmm,run_queue,timer_queue,data switch_checks=1975
[sched-sync] cpus=4 ipi_sent=1921 ipi_received=991 ipi_coalesced=930 idle_running_aps=3 running_max=1 canaries_ok=20
ELYON_SCHED_SYNC_READY
[PASS] K004-10 synchronisation: 4 CPU, compteur 120000/120000 sous verrou (contention 204797), PMM 192 allocations sans doublon, ping-pong 100 réveils, course au réveil 1 gagnant, tempête 675 migrations (77 attentes on_cpu), 1975 switchs contrôlés, aucune violation d'ordre
```

Validation croisée sous VMware® Workstation 17 Pro 17.0.2 build-21581411, UEFI, 4 vCPU, même build :

```text
[sched-sync] storm tasks=6 rounds=300 migrations=592 moves=29 deferred=7 misplaced=0 forced=7 wakes=600 on_cpu_waits=0
[sched-sync] locks acquisitions=426,25643,73,120000 contention=0,1,0,272774 classes=pmm,run_queue,timer_queue,data switch_checks=1625
[sched-sync] cpus=4 ipi_sent=789 ipi_received=774 ipi_coalesced=15 idle_running_aps=3 running_max=1 canaries_ok=20
[PASS] K004-10 synchronisation: 4 CPU, compteur 120000/120000 sous verrou (contention 272774), PMM 192 allocations sans doublon, ping-pong 100 réveils, course au réveil 1 gagnant, tempête 592 migrations (0 attentes on_cpu), 1625 switchs contrôlés, aucune violation d'ordre
[PASS] VMware 4 vCPU: Elyon 0.0.4 K003-01..12 et K004-01..10 validés sur build_id=0c2911c70ccd
```

`misplaced` compte les tâches qu'un CPU a trouvées dans sa file alors que leur affinité venait de l'exclure : `pop_allowed()` les écarte et les remet dans la file d'un CPU autorisé, si bien qu'aucune ne démarre sur un CPU exclu. Les IPI envoyés et non reçus sont tous fusionnés (`991 + 930 = 1921`, `774 + 15 = 789`), comme admis depuis K004-08.

Limites : sous VMware, la tempête n'a déclenché aucune attente `on_cpu` ; ce chemin n'est prouvé que sous QEMU et par l'injection de faute ; les spinlocks ne sont pas équitables, un ticket lock ne sera ajouté que si K004-11 révèle une famine ; `swapgs`, l'entrée NMI/#DF/#MC « paranoïaque » et le RSP0 par tâche, nécessaires au ring 3, sont reportés à la 0.0.5 (`ROADMAP.md` à la racine du dépôt, section Sécurité, exigées avant la qualification des 0.0.5 et 0.0.6) ; une panique Rust et l'échec de la qualification de K004-10 n'arrêtent encore que le CPU concerné, et non tous les CPU par `halt_system` ; le contrôle de stabilité SMP de K003 attend chaque heartbeat d'AP pendant un délai PIT fixe, en temps réel, et peut échouer quand l'hôte retarde un vCPU, ce que K004-11 devra prendre en compte.

**K004-10 : PASS.**

Le prochain critère est **K004-11 : stress SMP.**

## K004-11 — Stress SMP, récupération des tâches et volume de context switches

Une tâche terminée est désormais récupérée : sa pile et sa zone FPU retournent au PMM et son slot est réutilisé. Le stress crée ainsi des milliers de tâches sur les 4 CPU sans épuiser les 256 slots du registre ni la mémoire.

La logique pure est dans `elyon_core::stress`, testée sur l'hôte :

- `check_stress()` refuse, chacun avec son erreur, un nombre de CPU ou de tâches inattendu, une tâche non terminée ou non récupérée, trop peu de switchs, une tâche sur deux CPU, une corruption SIMD, un canari abîmé, des pages ou des slots qui ne reviennent pas exactement à leur valeur de départ (moins : fuite ; plus : double libération), et une famine ;
- `TimerQueue::any()` permet de vérifier qu'aucune échéance ne désigne une tâche récupérée ;
- 18 tests.

Côté noyau :

- piles et zones FPU viennent de `allocate_owned` ; leurs jetons `FrameRange` sont gardés par slot (`task_frames.rs`), sous un verrou de classe `Pmm` jamais tenu pendant un appel au PMM ;
- `sched::reap()` récupère une tâche DEAD : elle ne doit être dans aucune file ni avoir d'échéance ou de ticket d'attente, et doit avoir une pile aux canaris intacts ; le passage DEAD → REAPING est atomique, une tâche encore `on_cpu` repasse à DEAD (`NotYet`) ; la pile est retirée du slot avant que ses pages soient rendues, puis le slot redevient libre. Les identifiants ne sont jamais réattribués : un identifiant périmé ne désigne plus rien ;
- files de CPU et file temporelle passent de 8 et 16 à 64 entrées ;
- un compteur de switchs par CPU, incrémenté dans `transfer()`, sert de mesure de famine indépendante du temps ;
- une panique Rust et tout échec de qualification après le démarrage des AP arrêtent tous les CPU par `fatal::halt_system` (limite de K004-10 levée) ;
- le contrôle de stabilité SMP de K003 attend chaque heartbeat d'AP au plus 500 ms par tour au lieu d'un délai PIT fixe (limite de K004-10 levée).

La qualification, exécutée après K004-10 avec le timer du BSP à 100 Hz, enchaîne 9 vagues de 40 tâches par AP, soit 1080 tâches à 4 CPU. Chaque vague est entièrement mise en file avant qu'un AP ne démarre, puis récupérée avant la suivante. Chaque tâche fait 100 tours selon son rôle :

1. yield : cède le CPU et mesure combien de switchs son CPU fait avant de la reprendre ;
2. simd : charge XMM0..15 et un MXCSR propres à la tâche, cède le CPU, relit ; le code Rust du noyau, compilé sans SSE, ne touche jamais ces registres ;
3. sleep : dort 10 ms un tour sur dix ;
4. block : se bloque un tour sur cinq ; le BSP la réveille et fait tourner son affinité d'un AP à l'autre.

Le BSP vérifie avec `check_stress()` 1080 tâches créées, terminées et récupérées, au moins la moitié des tours en switchs, aucune tâche sur deux CPU, aucune corruption SIMD, le retour exact des pages et des slots libres, le retour des AP en idle, et une attente d'au plus 64 switchs, longueur maximale d'une file. En Round-Robin, chaque tâche placée devant une autre passe une fois avant elle ; une vague met au plus 50 tâches dans une file. L'attente est mesurée entre la mise en file et le premier passage, relevée sous le verrou de la file, et entre deux passages, pour les rôles à affinité fixe.

Trois injections de faute ont été détectées, chacune arrêtant tous les CPU : la zone FPU non rendue (`free frames not restored after reaping`, 1080 pages manquantes), la sauvegarde FPU sautée pendant le stress (`registers or FPU state corrupted`, 26 924 tours corrompus) et une tâche toujours contournée dans sa file (`a stress task was starved`, attente initiale de 3627 switchs). Cette dernière injection est passée d'abord inaperçue, l'attente n'étant mesurée qu'entre deux passages : l'attente initiale a été ajoutée. Le script `test-sched-stress-qemu.sh` recalcule chaque valeur à partir du nombre d'AP ; 31 copies altérées d'un log réel sont toutes refusées et 3 variantes légitimes acceptées.

Le code a été relu par les agents ECC rust-reviewer, security-reviewer, silent-failure-hunter et architect, sans point bloquant ni chemin d'utilisation après libération. Les corrections retenues figurent dans la PR #21 : course entre le compte de sortie d'une tâche et sa terminaison, qui aurait arrêté à tort le système, borne de famine resserrée de 128 à 64, contrôles de file et d'échéance faits avant le passage à REAPING, retour à DEAD d'une tâche encore sur son CPU, libération des deux plages même si la première échoue, échéance d'une tâche récupérée comptée comme périmée.

Le premier boot VMware, sur le commit `5d5dc4b`, a échoué fail-closed : 50 515 switchs pour 54 000 exigés, et une attente initiale de 64. Le BSP relevait le compteur de l'AP avant de mettre la tâche en file et envoyait un IPI par tâche ; sous VMware, où chaque IPI sort de la VM, comme sur un hôte QEMU chargé (attente faussement mesurée à 103), les premières tâches d'une vague tournaient seules et l'AP accumulait des switchs avant la mise en file. Le relevé est désormais fait sous le verrou de la file et la vague démarre d'un seul IPI par AP.

La qualification a été exécutée sur le commit `cc6ed4ab3631`, GitHub Actions run **#66** du workflow Elyon 0.0.4, et en local sous QEMU 8.2.2 / OVMF, à 4 et 2 vCPU.

Preuves :

```text
[stress] cpus=4 waves=9 tasks_per_wave=120 rounds=100 created=1080 exited=1080 reaped=1080 switches=109594
[stress] roles simd_rounds=27000 sleeps=2700 blocks=5400 wakes=5400 moves=8766 corruptions=0 canary_failures=0 running_max=1
[stress] fairness min_progress=100 max_wait_switches=41 initial_wait=40 yield_wait=41 limit=64 idle_running_aps=3
[stress] reclaim frames_before=249060 frames_after=249060 slots_before=197 slots_after=197
ELYON_SCHED_STRESS_READY
[PASS] K004-11 stress SMP: 4 CPU, 1080 tâches en 9 vagues créées, terminées et récupérées, 109594 switchs, 27000 tours SIMD sans corruption, 5400 blocages réveillés, attente max 41 switchs (borne 64), pages et slots restitués
[PASS] K004-11 stress SMP: 2 CPU, 360 tâches en 9 vagues créées, terminées et récupérées, 36385 switchs, 9000 tours SIMD sans corruption, 1800 blocages réveillés, attente max 40 switchs (borne 64), pages et slots restitués
```

Validation croisée sous VMware® Workstation 17 Pro 17.0.2 build-21581411, UEFI, 4 vCPU, même build :

```text
[stress] cpus=4 waves=9 tasks_per_wave=120 rounds=100 created=1080 exited=1080 reaped=1080 switches=100172
[stress] roles simd_rounds=27000 sleeps=2700 blocks=5400 wakes=5400 moves=87 corruptions=0 canary_failures=0 running_max=1
[stress] fairness min_progress=100 max_wait_switches=41 initial_wait=40 yield_wait=41 limit=64 idle_running_aps=3
[stress] reclaim frames_before=271582 frames_after=271582 slots_before=197 slots_after=197
[PASS] K004-11 stress SMP: 4 CPU, 1080 tâches en 9 vagues créées, terminées et récupérées, 100172 switchs, 27000 tours SIMD sans corruption, 5400 blocages réveillés, attente max 41 switchs (borne 64), pages et slots restitués
[PASS] VMware 4 vCPU: Elyon 0.0.4 K003-01..12 et K004-01..11 validés sur build_id=cc6ed4ab3631
```

`moves` ne compte que les tâches « block » déplacées alors qu'elles étaient en file : sous VMware, le BSP les trouve presque toujours bloquées et le nouveau masque ne s'applique qu'à leur réveil.

Sur le même commit, 10 boots répétés de K004-11, 5 à 4 vCPU et 5 à 2 vCPU, dont les 5 premiers avec l'hôte chargé à 100 %, ont tous réussi : attente initiale de 40 et entre yields de 41 à 4 vCPU, 39 et 40 à 2 vCPU, à chaque boot ; 108 013 à 109 789 switchs à 4 vCPU, 35 955 à 36 448 à 2 vCPU. La fausse famine observée sous charge avant le correctif n'est pas réapparue.

Limites : les AP n'ont pas de timer, le stress y est donc coopératif et une tâche qui ne cède jamais son CPU n'est pas détectée par la mesure en switchs ; une file ou la file temporelle pleine est une erreur fatale, il faudra une contre-pression avant le ring 3 ; `running_count_on` parcourt les 256 slots à chaque switch ; la clé de la file temporelle limite le ticket d'attente à 32 bits, soit environ 4 milliards d'attentes avant l'arrêt `TicketOverflow` ; les tâches des qualifications précédentes ne sont pas récupérées et gardent leurs slots et leurs pages ; `canary_failures` vaut toujours 0 dans le rapport, `reap()` arrêtant le système au premier canari abîmé ; `begin_reap` suppose qu'une tâche DEAD n'est dans aucune file, ce que garantit la sortie depuis RUNNING mais pas un `terminate` d'une tâche en file.

**K004-11 : PASS.**

Le prochain critère est **K004-12 : qualification finale 0.0.4.**
