# Elyon Kernel 0.0.5 — Validation

**EN COURS — K005-01 à K005-04 sont validés. K005-05 à K005-12 restent à qualifier.**

## Baseline héritée

La 0.0.5 est dérivée de la Release `v0.0.4`, dont les critères K003-01 à K003-12 et K004-01 à K004-12 sont qualifiés sous QEMU/OVMF et VMware Workstation en 4 vCPU (voir `Elyon-Kernel-0.0.4/QUALIFICATION_0.0.4.md`). La CI 0.0.5 conserve ces invariants comme régressions pendant tout le chantier ring 3 : chaque test K003 et K004 de la 0.0.4 tourne à chaque commit, et le gate de release applique le gate final K003-01..12 et K004-01..11 au boot QEMU/OVMF à 4 vCPU.

## Critères

| Critère | Description | Statut |
|---|---|---|
| K005-01 | Segments utilisateur, `RSP0` par tâche | **PASS** |
| K005-02 | `swapgs` à chaque entrée depuis le ring 3 | **PASS** |
| K005-03 | Entrée « paranoïaque » NMI/#DF/#MC | **PASS** |
| K005-04 | SMEP, SMAP et copies validées | **PASS** |
| K005-05 | Pages utilisateur isolées et rendables | À faire |
| K005-06 | `syscall`/`sysret` sans fuite de registres | À faire |
| K005-07 | Premier processus utilisateur | À faire |
| K005-08 | Une faute en ring 3 tue le processus, jamais le système | À faire |
| K005-09 | Timer LAPIC et préemption sur chaque AP | À faire |
| K005-10 | Contre-pression sur file pleine | À faire |
| K005-11 | Stress SMP ring 3 / ring 0 | À faire |
| K005-12 | Qualification finale 0.0.5 | À faire |

## Politique de preuve

Chaque PASS doit être lié à un commit qualifié et à un test exécuté. Les marqueurs série seuls ne suffisent pas si le test ne valide pas également l'invariant concerné. Une entrée depuis le ring 3 étant hors du contrôle du noyau, chaque critère de frontière est aussi prouvé par des injections de faute : un processus qui tente d'enfreindre l'invariant doit être tué, et le système doit continuer.

La première étape fonctionnelle sera **K005-01**.

## K005-01 — Segments utilisateur et RSP0 par tâche

La GDT de chaque CPU contient désormais les segments du ring 3, et le TSS de chaque CPU donne, à chaque instant, la pile noyau de la tâche qui s'y exécute : celle que prendra toute entrée depuis le ring 3. Aucun code ne tourne encore en ring 3.

La logique pure est dans `elyon_core::segments`, testée sur l'hôte :

- disposition `LAYOUT` : code noyau `0x08`, données noyau `0x10`, données utilisateur `0x18`, code utilisateur `0x20`, TSS `0x28` sur deux entrées. `sysret` en mode 64 bits prend SS = base + 8 et CS = base + 16 : les données utilisateur doivent précéder le code utilisateur ;
- `star()` calcule `STAR` (`0x0013000800000000`) et refuse une disposition incompatible avec `syscall`/`sysret` ;
- `check_gdt()` décode les descripteurs bruts relus dans la GDT chargée et refuse, chacun avec son erreur, une première entrée non nulle, un code noyau atteignable depuis le ring 3, un code utilisateur en DPL 0 ou sans le bit L, un code « conforming », des données en lecture seule ou « expand-down », un segment absent, un TSS de DPL non nul ou de taille suffisante pour un bitmap d'E/S ;
- `check_iomap()` exige un `iomap_base` au-delà de la limite du TSS : le ring 3 n'a accès à aucun port ;
- `rsp0_for()` rend le sommet aligné et canonique de la pile d'une tâche ; `check_observed_rsp0()` contrôle un RSP0 relu contre le RSP réel de la tâche et contre des piles réservées ;
- 32 tests.

Côté noyau :

- `gdt.rs` ajoute à la GDT de chaque CPU les données puis le code utilisateur (DPL 3), avant le TSS, et vérifie les sélecteurs contre `LAYOUT` ; le TSS passe de `0x18` à `0x28`, lu partout par `tss_selector_for` ;
- `transfer()` écrit, juste avant le switch et IRQ masquées, le sommet de la pile de la tâche entrante dans le RSP0 du TSS de ce CPU, et 0 hors tâche, où aucune entrée ring 3 n'est possible. RSP0 est lu et écrit en deux moitiés `u32` volatiles : le TSS est `packed(4)` et RSP0, à l'offset 4, n'est aligné que sur 4 octets ;
- `loaded_entries()` relit la GDT réellement chargée via `sgdt`.

La qualification, exécutée après K004-11, lance 4 tâches par AP qui cèdent le CPU 50 fois. À chaque reprise, IRQ masquées, une tâche relit la GDT chargée sur son CPU et son `iomap_base`, compare le RSP0 du TSS de ce CPU au sommet de sa pile, et le contrôle indépendamment contre son RSP réel, lu par `mov rsp`, et contre la pile IST #DF de son CPU et la pile de boot du BSP. Le BSP vérifie sa propre GDT, un RSP0 nul hors tâche, puis récupère les tâches.

Trois injections de faute ont été détectées, chacune arrêtant tous les CPU : RSP0 jamais écrit par `transfer()` (`checks=0 mismatches=600`), code utilisateur remplacé par un code DPL 0 (`user code segment is not a present ring 3 long-mode code segment`, dès la GDT du BSP), et `rsp0_of` faussé pour `transfer()` comme pour la comparaison directe : celle-ci passe (`checks=600`), seul le contrôle contre le RSP réel le détecte (`live_rsp_failures=600`). Le script `test-segments-qemu.sh` recalcule la disposition, `STAR` et chaque compteur à partir du nombre d'AP, et vérifie le sélecteur `tr` de chaque AP.

Le code a été relu par les agents ECC rust-reviewer et security-reviewer, sans point bloquant. Corrections retenues : accès volatile mal aligné à RSP0, panique possible de `VirtAddr::new` sur une adresse non canonique, contrôle trop permissif des types de segments et du TSS, contrôle de RSP0 circulaire.

La qualification a été exécutée sur le commit `0a1962375612`, GitHub Actions run **#3** du workflow Elyon 0.0.5, et en local sous QEMU 8.2.2 / OVMF, 4 vCPU, avec la suite complète de 25 tests QEMU.

Preuves :

```text
[segments] layout kernel_code=0x8 kernel_data=0x10 user_data=0x1b user_code=0x23 tss=0x28 star=0x0013000800000000 gdt_cpus=0xf
[segments] rsp0 cpus=4 tasks=12 rounds=50 checks=600 mismatches=0 live_rsp_ok=600 live_rsp_failures=0 gdt_failures=0 distinct=12 bsp_rsp0=0x0 running_max=1
ELYON_SEGMENTS_READY
[PASS] K005-01 segments: GDT vérifiée sur 4 CPU (code/données noyau, données/code utilisateur DPL 3, TSS 0x28), STAR=0x0013000800000000, RSP0 = sommet de la pile de la tâche, confirmé par son RSP réel, à chacun des 600 contrôles (12 tâches distinctes), 0 hors tâche
```

Validation croisée sous VMware Workstation Pro, UEFI, 4 vCPU, même build :

```text
[smp] AP online cpu=1 apic_id=1 selector=0x8 tr=0x28
[smp] AP online cpu=2 apic_id=2 selector=0x8 tr=0x28
[smp] AP online cpu=3 apic_id=3 selector=0x8 tr=0x28
[segments] layout kernel_code=0x8 kernel_data=0x10 user_data=0x1b user_code=0x23 tss=0x28 star=0x0013000800000000 gdt_cpus=0xf
[segments] rsp0 cpus=4 tasks=12 rounds=50 checks=600 mismatches=0 live_rsp_ok=600 live_rsp_failures=0 gdt_failures=0 distinct=12 bsp_rsp0=0x0 running_max=1
[PASS] VMware 4 vCPU: Elyon 0.0.5 K003-01..12, K004-01..11 et K005-01 validés sur build_id=0a1962375612
```

Limites : RSP0 est le sommet de la pile sur laquelle la tâche s'exécute aussi en ring 0 ; une entrée depuis le ring 3 ne devra donc avoir lieu que pile noyau vide, ce que K005-07 devra garantir ; les switchs historiques de la qualification K004-02 (`task_stack.rs`) ne passent pas par `transfer()` et n'écrivent pas RSP0, sans conséquence tant qu'aucun code ring 3 ne s'y exécute ; une erreur de `rsp0_of` dans `transfer()` survient après la réclamation `on_cpu` et reste fatale.

**K005-01 : PASS.**

Le prochain critère est **K005-02 : `swapgs` à chaque entrée depuis le ring 3.**

## K005-02 — `swapgs` à chaque entrée depuis le ring 3

Du code s'exécute désormais en ring 3. Toute entrée depuis le ring 3 échange GS_BASE et KERNEL_GS_BASE avant que le noyau ne touche à sa zone per-CPU, et la sortie vers le ring 3 les rééchange : le noyau ne tourne jamais avec le GS d'un processus, et un processus ne voit jamais celui du noyau.

La logique pure est dans `elyon_core::ring3`, testée sur l'hôte :

- `from_user()` : une entrée vient du ring 3 quand le RPL du CS sauvegardé vaut 3, seul critère des stubs d'entrée ;
- région utilisateur `USER_BASE` dans PML4[1], à 512 Gio : le mappage identité du noyau, dans PML4[0], ne reçoit jamais le bit U/S ;
- `table_entry()`, `check_user_entry()` et `check_kernel_entry()` construisent et contrôlent les entrées de table de pages des deux chemins ;
- `iret_frame()` construit le cadre `iretq` vers le ring 3 (CS `0x23`, SS `0x1b`, RFLAGS `0x202`, IOPL 0) et refuse une cible ou une pile hors de la région utilisateur, ou une pile mal alignée ; sa disposition C est figée par un test, l'asm la lisant à des offsets fixes ;
- 14 tests.

Côté noyau :

- macros `SWAPGS_ON_ENTRY` et `SWAPGS_ON_EXIT` dans `idt.rs` : test du RPL du CS sauvegardé, `swapgs` si l'entrée vient du ring 3, `lfence` sur les deux chemins de l'entrée (variante Spectre SWAPGS), `swapgs` juste avant `iretq` vers le ring 3. Elles équipent les stubs du timer, de l'IPI de rescheduling, de l'IPI de stabilité et le chemin commun des exceptions ; le vecteur spurious n'accède pas à GS ;
- `elyon_enter_user` masque les IRQ, empile le cadre, efface les 15 registres généraux, fait `swapgs` vers le GS utilisateur préparé dans KERNEL_GS_BASE, puis `iretq` ;
- tables utilisateur propres (PDPT, PD, PT) sous PML4[1] ; `check_user_probe()` relit le bit U/S à chaque niveau du chemin utilisateur et son absence sur toutes les entrées du mappage identité de 4 Gio ;
- `int 0x81` est la seule porte DPL 3 ; tout autre `int n` depuis le ring 3 lève #GP ; la porte n'est acceptée que pendant une sonde, sur le CPU sondé ;
- `CR4.FSGSBASE` reste inactif : le ring 3 ne peut pas écrire sa base GS ; `EFER.SCE` est vérifié et forcé à 0 sur chaque AP, pour qu'un `syscall` lève #UD tant qu'il n'est pas configuré (K005-06).

La qualification, exécutée après K005-01, sonde chaque AP à tour de rôle. Une page de code, en lecture seule, et une page de pile sont mappées dans la région utilisateur ; une tâche noyau du CPU sondé écrit un GS utilisateur propre à ce CPU dans KERNEL_GS_BASE et entre en ring 3, dans une boucle qui attend un drapeau au bas de sa page de pile. Le BSP lui envoie des IPI de rescheduling ; à chaque entrée depuis le ring 3, le dispatcher vérifie par les MSR, sans passer par GS, que GS_BASE est la zone per-CPU de ce CPU et que KERNEL_GS_BASE garde le GS utilisateur. Après 20 entrées, le noyau lève le drapeau ; la sonde sort par `int 0x81`, dont le dispatcher refait les mêmes contrôles, efface KERNEL_GS_BASE et termine la tâche, récupérée ensuite.

Trois injections de faute ont été détectées : `swapgs` retiré à l'entrée (le noyau lit sa zone per-CPU à travers le GS de la sonde et prend une #PF à `CR2=0x8000101000`, exactement ce GS sur le CPU 1), `swapgs` retiré à la sortie (même faute à l'entrée suivante) et `swapgs` inconditionnel (les IRQ prises en mode noyau basculent sur le mauvais GS ; le scheduler s'arrête dès les régressions K004 : `task runs on a CPU outside its affinity`). Le script `test-ring3-swapgs-qemu.sh` exige une ligne par AP dans l'ordre, au moins 20 entrées, aucune erreur de GS, une sortie contrôlée, FSGSBASE et SCE inactifs, et relit CR4 en fin de boot, indépendamment.

Le code a été relu par les agents ECC rust-reviewer et security-reviewer, sans point bloquant. Corrections retenues : section asm de la sonde non restaurée (`.pushsection`/`.popsection`), qui pouvait faire tomber les stubs de l'IDT dans `.rodata`, porte `int 0x81` liée à la sonde active, `EFER.SCE` contrôlé, KERNEL_GS_BASE effacé après la sonde, provenance des adresses du code copié, nettoyage de `ACTIVE` en cas d'échec.

La qualification a été exécutée sur le commit `913dab778016`, GitHub Actions run **#7** du workflow Elyon 0.0.5, et en local sous QEMU 8.2.2 / OVMF, 4 vCPU, avec la suite complète de 26 tests QEMU.

Preuves :

```text
[ring3] cpu=1 user_entries=20 gs_failures=0 exits=1 exit_gs_ok=1 fsgsbase_on=0 sce_on=0
[ring3] cpu=2 user_entries=20 gs_failures=0 exits=1 exit_gs_ok=1 fsgsbase_on=0 sce_on=0
[ring3] cpu=3 user_entries=20 gs_failures=0 exits=1 exit_gs_ok=1 fsgsbase_on=0 sce_on=0
[ring3] swapgs aps=3 target_entries=20 cr4_fsgsbase=0 user_base=0x8000000000
ELYON_RING3_SWAPGS_READY
[PASS] K005-02 swapgs: code exécuté en ring 3 sur 3 AP, 60 entrées depuis le ring 3 avec GS_BASE noyau et GS utilisateur préservé, 3 sorties par int 0x81 contrôlées, CR4.FSGSBASE inactif (0x0000000000040668)
```

Validation croisée sous VMware Workstation Pro, UEFI, 4 vCPU, même build :

```text
[ring3] cpu=1 user_entries=20 gs_failures=0 exits=1 exit_gs_ok=1 fsgsbase_on=0 sce_on=0
[ring3] cpu=2 user_entries=20 gs_failures=0 exits=1 exit_gs_ok=1 fsgsbase_on=0 sce_on=0
[ring3] cpu=3 user_entries=20 gs_failures=0 exits=1 exit_gs_ok=1 fsgsbase_on=0 sce_on=0
[ring3] swapgs aps=3 target_entries=20 cr4_fsgsbase=0 user_base=0x8000000000
[PASS] VMware 4 vCPU: Elyon 0.0.5 K003-01..12, K004-01..11 et K005-01..02 validés sur build_id=913dab778016
```

Limites : seuls les stubs d'IRQ et la porte `int 0x81` sont exercés depuis le ring 3 ; une exception venue du ring 3 (#GP, #PF, #UD) fait `swapgs` mais arrête encore le système, K005-08 tuera le processus ; une NMI ou une #MC entre `swapgs` et `iretq`, ou une faute de `iretq` lui-même, trouve un CS noyau avec le GS utilisateur chargé : l'entrée « paranoïaque » de K005-03 le traitera ; NX, SMEP, SMAP et UMIP ne sont pas encore actifs (K005-04, K005-05) : la page de pile utilisateur est exécutable et le ring 3 peut lire les adresses de la GDT et de l'IDT par `sgdt`/`sidt` ; KERNEL_GS_BASE n'est pas sauvegardé par tâche ; la sonde voit l'image FPU propre de sa tâche, les vrais processus recevront une image propre explicite (K005-07).

**K005-02 : PASS.**

Le prochain critère est **K005-03 : entrée « paranoïaque » NMI/#DF/#MC.**

## K005-03 — Entrée « paranoïaque » NMI/#DF/#MC

Une NMI, une #DF ou une #MC peut arriver n'importe où, y compris entre le `swapgs` de sortie et l'`iretq` vers le ring 3 : le CS sauvegardé est alors celui du noyau alors que GS_BASE porte déjà le GS utilisateur. L'entrée de ces trois vecteurs ne se fie donc plus au RPL du CS : elle relit GS_BASE et le compare à la valeur que le noyau a rangée pour ce CPU.

La logique pure est dans `elyon_core::paranoid`, testée sur l'hôte :

- une IST dédiée par vecteur : #DF=1, NMI=2, #MC=3 ;
- `ist_pointer()` : le pointeur IST du TSS laisse au sommet de chaque pile un emplacement de 16 octets qui contient le GS_BASE noyau du CPU, et refuse une pile nulle, non canonique ou de moins de 16 Kio ;
- `check_spans()` : toutes les piles IST de tous les CPU sont valides et deux à deux disjointes ;
- `SLOT_OFFSET` donne la position de ce mot depuis RSP dans le stub ; un test la fige ;
- `decode_gate()` et `check_gate()` contrôlent une porte relue dans l'IDT chargée. Elle doit être présente, DPL 0, de type interruption (une porte de trappe laisserait IF actif), sur son IST, et viser le bon sélecteur et le bon gestionnaire ;
- `check_observation()` exige le GS d'entrée attendu (sinon la condition testée n'a pas eu lieu), le GS noyau au moment du dispatch, l'index du CPU lu via GS et un RSP dans l'IST ;
- `check_mce()` exige CR4.MCE quand CPUID annonce MCE : sans lui, une #MC met le CPU en arrêt ;
- 22 tests.

Côté noyau :

- les stubs NMI, #DF et #MC (`elyon_paranoid_common` dans `idt.rs`) poussent les registres, lisent le MSR GS_BASE et l'empilent. S'il diffère du mot au sommet de l'IST, ils y écrivent la valeur noyau par `wrmsr`, puis exécutent `lfence` sur les deux chemins. Au retour, ils réécrivent le GS_BASE d'entrée et font `iretq`. Il n'y a pas de `swapgs`, et KERNEL_GS_BASE n'est jamais touché ;
- `gdt::prepare_for_cpu()` vérifie les trois piles avant de les installer. Il écrit le mot GS noyau puis le pointeur IST du TSS, et s'arrête avec le message d'erreur si une pile est invalide ou en chevauche une autre. Chaque AP reçoit trois piles IST de 16 Kio, et le BSP reçoit des piles statiques ;
- la sortie vers le ring 3 passe par un point unique : `elyon_return_to_user: swapgs` suivi de `elyon_user_iret: iretq`. Il sert aux stubs d'IRQ (macro `IRET_TO_ORIGIN`) et à `elyon_enter_user`. Une faute de cet `iretq` arrive avec un CS noyau et le GS utilisateur : `elyon_exception_common` fait donc `swapgs` quand le RIP sauvegardé est `elyon_user_iret`, en plus du cas RPL 3 ;
- CR4.MCE est activé sur le BSP après l'IDT, et sur chaque AP après le chargement de l'IDT ;
- `apic::send_nmi()` envoie une vraie NMI à un AP.

La qualification s'exécute après K005-02. Sur chaque AP, le BSP envoie une vraie NMI pendant que l'AP est en mode noyau. Il en envoie une deuxième pendant que l'AP attend dans une fenêtre qui reproduit la sortie : GS_BASE est remplacé par un GS utilisateur non mappé propre au CPU, après le `swapgs` et avant l'`iretq`. L'AP exécute ensuite `int 18` dans la même fenêtre. Sur le BSP, `int 2` et `int 18` sont exécutés dans la fenêtre. Chaque événement est vérifié par les MSR, sans passer par GS : GS d'entrée, GS noyau pendant le dispatch, index du CPU, RSP dans l'IST dédiée, puis GS d'entrée restauré après `iretq`. Le gestionnaire de sonde n'accepte qu'un seul événement à la fois, uniquement sur le CPU sondé, et refuse une vraie #MC matérielle (MCG_STATUS.MCIP). La qualification relit aussi les portes 2, 8 et 18 dans l'IDT chargée, les 12 mots GS au sommet des IST et CR4.MCE sur chaque CPU.

Deux régressions complètent la qualification :

- `test-df-qemu.sh` lève une #DF avec un GS utilisateur dans GS_BASE ;
- `test-user-iret-qemu.sh` fait échouer l'`iretq` de sortie avec un CS invalide (#GP 0x38). Le diagnostic doit se faire avec le GS noyau et en une seule exception.

Six injections de faute ont été détectées :

| Injection | Détection |
|---|---|
| `wrmsr` d'entrée retiré | DispatchGs sur les événements de la fenêtre |
| GS d'entrée non restauré à la sortie | `restored=1` |
| NMI sans IST | `gates_ok=0` |
| CR4.MCE absent sur les AP | `cr4_mce=0` |
| Contrôle du RIP `elyon_user_iret` retiré | cascade de #PF et échec de la régression `iretq` |
| #DF fondée sur le CS au lieu de GS_BASE | `replaced=0` et échec de la régression #DF |

Le script `test-paranoid-qemu.sh` exige :

- la ligne BSP ;
- une ligne par AP, dans l'ordre ;
- le résumé ;
- l'ordre des marqueurs.

Il relit aussi CR4 en fin de boot, de façon indépendante.

Le code a été relu par les agents ECC rust-reviewer et security-reviewer, sans point bloquant. Corrections retenues :

- gestionnaire de sonde à un seul événement, lié au CPU et au GS de la sonde ;
- refus d'une #MC matérielle ;
- sonde BSP à l'exécution ;
- commentaire SAFETY et message d'erreur dans `prepare_for_cpu` ;
- recompilation forcée et variables de test effacées dans `build.sh` ;
- invariants du chemin paranoïaque documentés dans l'asm.

La qualification a été exécutée sur le commit `41589af0a684` : run GitHub Actions **#11** du workflow Elyon 0.0.5, et en local sous QEMU 8.2.2 / OVMF, 4 vCPU, avec la suite complète de 35 tests (34 rejoués sur `a69deeb`, `test-df-qemu.sh` sur `41589af`). Les sondes paranoïaque et `iretq` ont été rejouées sur `a69deeb`, dont le code noyau est identique ; `41589af` ne modifie que `test-df-qemu.sh`, rejoué sur ce commit.

Preuves :

```text
[paranoid] bsp nmi_soft_window=1 mc_window=1 replaced=2 restored=2
[paranoid] cpu=1 nmi_kernel=1 nmi_window=1 mc_window=1 replaced=2 restored=3 gates_ok=1 cr4_mce=1
[paranoid] cpu=2 nmi_kernel=1 nmi_window=1 mc_window=1 replaced=2 restored=3 gates_ok=1 cr4_mce=1
[paranoid] cpu=3 nmi_kernel=1 nmi_window=1 mc_window=1 replaced=2 restored=3 gates_ok=1 cr4_mce=1
[paranoid] entry aps=3 cpus=4 ist_df=1 ist_nmi=2 ist_mc=3 slots_ok=12/12 bsp_gates_ok=1 bsp_cr4_mce=1
ELYON_PARANOID_ENTRY_READY
[PASS] K005-03 paranoid: sur 3 AP, NMI en mode noyau puis NMI et #MC dans la fenêtre swapgs/iretq servis avec le GS noyau sur leur IST dédiée, GS d'entrée restauré ; BSP : int 2 et int 18 dans la fenêtre ; 12 piles IST disjointes (#DF=1, NMI=2, #MC=3) ; CR4.MCE actif (0x0000000000040668)
[EXCEPTION] paranoid entry_gs=0x0000008000300000 gs_base=0x000000000020f040 replaced=1
[PASS] #DF pris en charge sur l'IST dédiée, GS utilisateur 0x0000008000300000 remplacé par l'entrée paranoïaque, puis arrêt fail-closed (qemu rc=124)
[EXCEPTION] user iret fault gs_base=0x000000000020f040 kernel_gs=1
[PASS] K005-03 iretq: #GP de l'iretq de sortie (0x00000000002012b3) diagnostiqué avec le GS noyau, GS utilisateur 0x0000008000300000 écarté, arrêt fail-closed (qemu rc=124)
```

Validation croisée sous VMware Workstation Pro, UEFI, 4 vCPU, même build (ISO sha256 `9cdb1b0c09de53067891339a97782390c2e69000c807d2e271bb28adbadd1b88`) :

```text
[kernel] build_id=41589af0a684
[paranoid] bsp nmi_soft_window=1 mc_window=1 replaced=2 restored=2
[paranoid] cpu=1 nmi_kernel=1 nmi_window=1 mc_window=1 replaced=2 restored=3 gates_ok=1 cr4_mce=1
[paranoid] cpu=2 nmi_kernel=1 nmi_window=1 mc_window=1 replaced=2 restored=3 gates_ok=1 cr4_mce=1
[paranoid] cpu=3 nmi_kernel=1 nmi_window=1 mc_window=1 replaced=2 restored=3 gates_ok=1 cr4_mce=1
[paranoid] entry aps=3 cpus=4 ist_df=1 ist_nmi=2 ist_mc=3 slots_ok=12/12 bsp_gates_ok=1 bsp_cr4_mce=1
ELYON_PARANOID_ENTRY_READY
[PASS] VMware 4 vCPU: Elyon 0.0.5 K003-01..12, K004-01..11 et K005-01..03 validés sur build_id=41589af0a684
```

Limites :

- les piles IST n'ont pas de page de garde : un débordement écrase la mémoire voisine sans faute ;
- une NMI imbriquée (une NMI arrivée après un `iretq` qui rouvre la fenêtre NMI pendant le gestionnaire) réutiliserait la même IST : le dispatch paranoïaque ne fait donc ni `sti`, ni `iretq`, ni faute récupérable ;
- la #MC n'est exercée que par `int 18`, pas par une vraie erreur machine ;
- une NMI hors sonde (watchdog, panne) reste fatale : elle est diagnostiquée puis le CPU s'arrête ;
- les cadres alloués au boot ne sont pas rendus si une allocation IST échoue, ce qui arrête le boot.

**K005-03 : PASS.**

Le prochain critère est **K005-04 : SMEP et SMAP actifs ; copie validée entre espace utilisateur et noyau (`stac`/`clac`).**

## K005-04 — SMEP et SMAP actifs ; copie validée entre espace utilisateur et noyau

SMEP interdit au noyau d'exécuter une page utilisateur. SMAP lui interdit d'y lire ou d'y écrire, sauf entre `stac` et `clac` (RFLAGS.AC). Le noyau n'ouvre cette fenêtre qu'à un seul endroit : l'instruction de copie de `elyon_user_copy`, atteinte seulement avec une plage validée.

La logique pure est dans `elyon_core::user_access`, testée sur l'hôte :

- `features()`, `check_support()`, `cr4_bits()` et `check_cr4()` : SMEP et SMAP sont lus dans la feuille CPUID 7, ignorée si le CPU ne l'annonce pas. Ils sont exigés. UMIP est activé quand le CPU le gère ;
- `check_write_protect()` : CR0.WP doit être actif, sinon le noyau écrirait sur une page en lecture seule ;
- `user_range()` : une plage est acceptée si elle est non vide, de 1 Mio au plus, sans débordement, et entièrement dans la région utilisateur `[USER_BASE, USER_LIMIT)` ;
- `copy_resume()` : une #PF n'est reprise au point de reprise de la copie que si toutes ces conditions sont réunies :
  - elle vient du noyau ;
  - elle a eu lieu sur l'instruction de copie ;
  - son code d'erreur ne porte que les bits P et W ;
  - AC était levé au moment de la faute ;
  - CR2 est dans la région utilisateur.

  Toute autre faute (tampon noyau, RSVD, PK, SGX, exécution, faute sans AC) reste fatale ;
- `check_smap_fault()` et `check_smep_fault()` : signatures exactes d'une violation SMAP (lecture de données, page présente, mode superviseur, AC nul) et SMEP (exécution, page présente, RIP = CR2 = cible) ;
- 26 tests.

Côté noyau :

- SMEP et SMAP sont exigés avant le chargement de l'IDT, sur le BSP comme sur chaque AP : les stubs d'entrée exécutent `clac`, qui lève #UD sur un CPU sans SMAP. Le BSP exécute aussi un `clac` explicite, puisque le firmware ne garantit pas AC nul ;
- `cpu::enable_supervisor_protection()` active CR4.SMEP, CR4.SMAP, CR4.UMIP (si pris en charge) et CR0.WP. Sur le BSP, l'appel a lieu après l'installation des tables d'Elyon, dont le mappage identité ne porte jamais le bit U/S. Sur chaque AP, il a lieu après le chargement de l'IDT ;
- `clac` est la première instruction de chaque entrée : macro `SWAPGS_ON_ENTRY` (IRQ et `int 0x81`), `elyon_exception_common` et `elyon_paranoid_common`. Une porte d'interruption n'efface pas AC, et le ring 3 peut le lever par `popfq` ; `iretq` rend au code interrompu son propre AC ;
- `user_copy::copy_from_user()` et `copy_to_user()` valident la plage, puis appellent `elyon_user_copy` (`stac ; rep movsb ; clac`). Une #PF reprise saute au point de reprise, qui refait `clac` et rend les octets restants. La copie échoue alors avec `Fault`, sans arrêter le système. Après une erreur, une partie des octets a pu être copiée ;
- `elyon_exception_common` peut maintenant revenir par `iretq`, mais seulement pour une faute du noyau reprise. Le RIP est réécrit vers un point de reprise du noyau, sans `swapgs`.

Un défaut antérieur a été trouvé par la qualification : le trampoline des AP ne pose que PE et PG dans CR0. Avec CR0.WP nul, le noyau pouvait écrire, sur les AP, dans n'importe quelle page en lecture seule, utilisateur ou noyau, et une copie vers une page utilisateur en lecture seule réussissait sur les CPU 1 à 3. CR0.WP est maintenant forcé sur chaque CPU et contrôlé par la sonde.

La qualification s'exécute après K005-03. Le BSP mappe, dans la table de la sonde K005-02, trois pages :

- une page de code en lecture seule (`mov eax, 2 ; ret`) ;
- une page de données ;
- une page jamais mappée.

Sur chaque CPU, le BSP d'abord puis chaque AP dans une tâche, IRQ masquées, la sonde vérifie :

- CR4 et CR0.WP ;
- une lecture directe de la page de données sans `stac` : elle doit lever une #PF de signature SMAP ;
- un appel direct de la page de code : il doit lever une #PF de signature SMEP (sans SMEP, la page rendrait 2) ;
- un aller-retour `copy_to_user` puis `copy_from_user` d'un motif propre au CPU, relu aussi par le mappage identité ;
- trois fautes de copie reprises : écriture sur la page de code (page intacte), lecture de la page jamais mappée, lecture qui déborde sur elle (les 8 octets d'avant la faute sont copiés) ;
- cinq plages refusées sans aucun accès : adresse noyau, plage vide, plage sous `USER_BASE`, plage au-delà de `USER_LIMIT`, débordement ;
- AC nul après la sonde et dans le gestionnaire des fautes de copie, prises pourtant AC levé.

La sonde ring 3 de K005-02 lève maintenant AC par `popfq` avant sa boucle : chaque entrée dans le noyau doit le trouver effacé, et la sortie `int 0x81` vérifie que le ring 3 l'avait bien levé.

Le script `test-user-access-qemu.sh` exige :

- la ligne CPUID ;
- une ligne par CPU, dans l'ordre ;
- une ligne ring 3 par AP ;
- le résumé ;
- l'ordre des marqueurs.

Il relit aussi CR0 et CR4 en fin de boot, de façon indépendante. La régression `test-smap-violation-qemu.sh` (mode `ELYON_TEST_SMAP`) lit directement la page de données hors de la copie, après la qualification. Elle exige une seule exception, une #PF error=0x1 sur cette adresse, sans reprise, puis un arrêt fail-closed.

Sept injections de faute ont été détectées :

| Injection | Détection |
|---|---|
| `clac` retiré des entrées d'IRQ | `ac_leaks=21` sur chaque AP |
| `clac` retiré de l'entrée des exceptions | `handler_ac=3` |
| CR4.SMAP non activé | `smap=0`, `cr4_ok=0` |
| CR4.SMEP non activé | `smep=0`, `cr4_ok=0` |
| CR0.WP non forcé | `cr0_wp=0` et `ro_refused=0` sur les AP |
| validation de plage retirée | #PF fatale à `CR2=0x7ffffffff8`, sous la région utilisateur |
| `stac` retiré | #PF fatale (error=0x3) dans la copie |

Le code a été relu par les agents ECC rust-reviewer et security-reviewer, sans point bloquant. Corrections retenues :

- seuls les bits P et W du code d'erreur autorisent une reprise ;
- SMEP et SMAP sont contrôlés avant l'IDT sur les AP ;
- `clac` explicite au boot ;
- gestionnaires de sonde plus stricts (CR2 pour SMAP, RSP aligné pour SMEP) ;
- copies partielles et invariant « jamais de page superviseur dans la région utilisateur » documentés.

La qualification a été exécutée sur le commit `c76338e541db`, en local sous QEMU 8.2.2 / OVMF, 4 vCPU (`-cpu max`). La suite complète de 37 tests est passée sur le commit précédant les corrections de revue. Sur le commit final, les deux tests K005-04 ont été rejoués et passent ; le workflow GitHub Actions Elyon 0.0.5 rejoue la suite complète.

Preuves :

```text
[cpu] smep=1 smap=1 umip=1
[uaccess] cpu=0 cr4_ok=1 cr0_wp=1 smap=1 smep=1 copy=1 ro_refused=1 unmapped_refused=1 partial=1 faults=3 rejects=5 handler_ac=0 ac_clear=1
[uaccess] cpu=1 cr4_ok=1 cr0_wp=1 smap=1 smep=1 copy=1 ro_refused=1 unmapped_refused=1 partial=1 faults=3 rejects=5 handler_ac=0 ac_clear=1
[uaccess] cpu=2 cr4_ok=1 cr0_wp=1 smap=1 smep=1 copy=1 ro_refused=1 unmapped_refused=1 partial=1 faults=3 rejects=5 handler_ac=0 ac_clear=1
[uaccess] cpu=3 cr4_ok=1 cr0_wp=1 smap=1 smep=1 copy=1 ro_refused=1 unmapped_refused=1 partial=1 faults=3 rejects=5 handler_ac=0 ac_clear=1
[uaccess] ring3 cpu=1 user_ac=1 entries=20 ac_leaks=0
[uaccess] ring3 cpu=2 user_ac=1 entries=20 ac_leaks=0
[uaccess] ring3 cpu=3 user_ac=1 entries=20 ac_leaks=0
[uaccess] protection cpus=4 smep=1 smap=1 umip=1 pages_ok=1 ring3_entries=60 max_copy=1048576
ELYON_USER_ACCESS_READY
[PASS] K005-04 user access: sur 4 CPU, SMEP et SMAP actifs (UMIP=1, CR0.WP), lecture et exécution directes d'une page utilisateur refusées par #PF, copies validées aller-retour, 3 fautes de copie reprises et 5 plages refusées par CPU ; AC effacé à 60 entrées depuis le ring 3 (CR0=0x0000000080010033 CR4=0x0000000000340e68)
[EXCEPTION] error=0x1 rip=0x000000000020bab2 cs=0x8 rflags=0x6
[EXCEPTION] CR2=0x0000008000005000
[PASS] K005-04 SMAP: lecture directe de la page utilisateur 0x0000008000005000 refusée par #PF (error=0x1), non reprise, arrêt fail-closed (qemu rc=124)
```

Validation croisée sous VMware Workstation Pro, UEFI, 4 vCPU (GenuineIntel), même build (ISO sha256 `9b32963d0d8b8ace78bf0cad1f99612fbccca61d7d2d005d7ca35fb7212c97f7`) :

```text
[kernel] build_id=c76338e541db
[cpu] smep=1 smap=1 umip=1
[uaccess] cpu=0 cr4_ok=1 cr0_wp=1 smap=1 smep=1 copy=1 ro_refused=1 unmapped_refused=1 partial=1 faults=3 rejects=5 handler_ac=0 ac_clear=1
[uaccess] cpu=1 cr4_ok=1 cr0_wp=1 smap=1 smep=1 copy=1 ro_refused=1 unmapped_refused=1 partial=1 faults=3 rejects=5 handler_ac=0 ac_clear=1
[uaccess] cpu=2 cr4_ok=1 cr0_wp=1 smap=1 smep=1 copy=1 ro_refused=1 unmapped_refused=1 partial=1 faults=3 rejects=5 handler_ac=0 ac_clear=1
[uaccess] cpu=3 cr4_ok=1 cr0_wp=1 smap=1 smep=1 copy=1 ro_refused=1 unmapped_refused=1 partial=1 faults=3 rejects=5 handler_ac=0 ac_clear=1
[uaccess] ring3 cpu=1 user_ac=1 entries=20 ac_leaks=0
[uaccess] ring3 cpu=2 user_ac=1 entries=20 ac_leaks=0
[uaccess] ring3 cpu=3 user_ac=1 entries=20 ac_leaks=0
[uaccess] protection cpus=4 smep=1 smap=1 umip=1 pages_ok=1 ring3_entries=60 max_copy=1048576
ELYON_USER_ACCESS_READY
[PASS] VMware 4 vCPU: Elyon 0.0.5 K003-01..12, K004-01..11 et K005-01..04 validés sur build_id=c76338e541db
```

Limites :

- un CPU sans SMEP ou sans SMAP n'est plus pris en charge : le boot s'arrête (QEMU doit tourner avec `-cpu max` ou équivalent) ;
- NX n'est pas encore actif (K005-05) : les pages utilisateur restent exécutables par le ring 3 ;
- la reprise accepte toute adresse de la région utilisateur, pas seulement la plage de la copie en cours : seule l'instruction de copie peut y fauter ;
- `user_range` ne contrôle que l'adresse : la région utilisateur ne doit jamais contenir de page superviseur (invariant à tenir dès K005-05) ;
- la qualification est sautée sur un boot à un seul CPU, comme celles de K005-02 et K005-03 ;
- le ring 3 n'entre encore dans le noyau que par les IRQ et `int 0x81` : une exception venue du ring 3 avec AC levé passe par le même `clac`, mais n'est pas exercée.

**K005-04 : PASS.**

Le prochain critère est **K005-05 : pages utilisateur prises par `allocate_owned`, bit U/S strict, jamais d'accès noyau par l'identity map.**
