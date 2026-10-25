# Elyon Kernel 0.0.5 — Validation

**EN COURS — K005-01 est validé. K005-02 à K005-12 restent à qualifier.**

## Baseline héritée

La 0.0.5 est dérivée de la Release `v0.0.4`, dont les critères K003-01 à K003-12 et K004-01 à K004-12 sont qualifiés sous QEMU/OVMF et VMware Workstation en 4 vCPU (voir `Elyon-Kernel-0.0.4/QUALIFICATION_0.0.4.md`). La CI 0.0.5 conserve ces invariants comme régressions pendant tout le chantier ring 3 : chaque test K003 et K004 de la 0.0.4 tourne à chaque commit, et le gate de release applique le gate final K003-01..12 et K004-01..11 au boot QEMU/OVMF à 4 vCPU.

## Critères

| Critère | Description | Statut |
|---|---|---|
| K005-01 | Segments utilisateur, `RSP0` par tâche | **PASS** |
| K005-02 | `swapgs` à chaque entrée depuis le ring 3 | À faire |
| K005-03 | Entrée « paranoïaque » NMI/#DF/#MC | À faire |
| K005-04 | SMEP, SMAP et copies validées | À faire |
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
