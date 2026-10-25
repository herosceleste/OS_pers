# Elyon Kernel 0.0.4 — Qualification de clôture

## Décision

**K004-01 à K004-12 : PASS**, sans régression des critères K003-01 à K003-12 hérités de la 0.0.3.

Le même gate final, `scripts/validate-vmware-final.sh`, qualifie le boot QEMU/OVMF et le boot VMware Workstation Pro, tous deux en UEFI et à 4 vCPU. Il couvre K003-01..12 et K004-01..11 ; K004-12 est ce gate lui-même, appliqué aux deux environnements sur le même commit.

## Baseline qualifiée

- branche : `elyon-0.0.4-k004-12`, PR #22 vers `elyon-0.0.4`
- commit qualifié : `e7416f1c5612`, dont le noyau est identique à celui de K004-11 (`cc6ed4a`) ; seuls les scripts de qualification ont changé
- GitHub Actions : Elyon Kernel 0.0.4 CI, run **#70**
- commit de merge construit par le workflow PR : `a181d6cac7ae`
- environnement CI : Ubuntu 24.04, QEMU/OVMF, 4 vCPU
- Rust : `rustc 1.99.1 (58cf8f9c3 2026-10-13)`

## Empreintes des artefacts CI #70

```text
a44d27968b2a5e484c3b8391ad1ecdb883659a90f0c571413e3191ae229efd00  build/esp/EFI/BOOT/BOOTX64.EFI
f214ef248a9976f1627f74f47c699d6376a1735db9d82ac30a64684174132792  build/esp/ELYON/KERNEL.ELF
```

Ces empreintes identifient les binaires du gate final de cette CI, construits sur le commit de merge. Le build_id est intégré au noyau, donc l'ISO VMware, construite sur `e7416f1`, a ses propres empreintes. La reproductibilité bit à bit entre environnements n'est pas revendiquée.

ISO VMware : `Elyon-Kernel-0.0.4-UEFI-e7416f1-k004-12.iso`, sha256 `86908992b59b587a082ea21d08be673d0b4e343e9f8cd2856279d49b3ab315db`.

## Gate final

`scripts/test-release-gate-qemu.sh` boote QEMU/OVMF à 4 vCPU, rejoue les contrôles K003 historiques, puis applique le gate final au log. Le gate final exige :

- K003-01..12 : ACPI et MADT à 4 CPU, Local APIC, IOAPIC entièrement masqué, timer périodique, état per-CPU, 3 AP prêts, SMP 4/4, stabilité SMP (10 heartbeats), changement de contexte ;
- K004-01..06 : les synthèses de modèle de tâche, de piles, de Round-Robin, de préemption, d'intégrité de contexte et d'isolation SIMD ;
- K004-07..11 : le même contrôle indépendant que chaque test QEMU, rejoué sur le log (`ELYON_CHECK_LOG`) ;
- le build_id du commit attendu, `ELYON_KERNEL_READY_0_0_4`, et aucune panique, exception ou ligne `FAILED`.

Il refuse un log dont le build_id ne correspond pas au commit, ou auquel manque un marqueur.

Marqueurs, identiques sous QEMU/OVMF et sous VMware :

```text
ELYON_ACPI_READY
ELYON_LAPIC_READY
ELYON_PERCPU_READY
ELYON_AP_READY cpu=1 apic_id=1
ELYON_AP_READY cpu=2 apic_id=2
ELYON_AP_READY cpu=3 apic_id=3
ELYON_AP_BOOT_READY
ELYON_SMP_READY
ELYON_SMP_STABLE
ELYON_CONTEXT_SWITCH_READY
ELYON_TASK_MODEL_READY
ELYON_TASK_STACKS_READY
ELYON_SCHED_RR_READY
ELYON_IOAPIC_READY
ELYON_TIMER_READY
ELYON_SCHED_PREEMPT_READY
ELYON_SCHED_CONTEXT_READY
ELYON_SCHED_FPU_READY
ELYON_SCHED_PERCPU_READY
ELYON_SCHED_IPI_READY
ELYON_SCHED_SLEEP_READY
ELYON_SCHED_SYNC_READY
ELYON_SCHED_STRESS_READY
ELYON_KERNEL_READY_0_0_4
```

Conclusion du gate sous QEMU/OVMF, CI #70 :

```text
[PASS] QEMU/OVMF 4 vCPU: Elyon 0.0.4 K003-01..12 et K004-01..11 validés sur build_id=a181d6cac7ae
[PASS] K004-12 qualification finale 0.0.4: K003-01..12 et K004-01..11 cohérents sur QEMU/OVMF 4 vCPU, boot nominal sans anomalie
```

Le code de retour de QEMU vaut 124 : le noyau s'arrête après `ELYON_KERNEL_READY_0_0_4` et `timeout` termine QEMU. Le verdict repose sur le contenu du log, pas sur ce code.

## Régressions fail-closed

```text
[PASS] #UD diagnostiqué puis arrêté en fail-closed (qemu rc=124)
[PASS] #DF pris en charge sur l'IST dédiée puis arrêt fail-closed (qemu rc=124)
```

Toute panique Rust et tout échec de qualification après le démarrage des AP arrêtent tous les CPU par NMI (`fatal::halt_system`).

## Validation croisée VMware finale 4 vCPU

**Résultat : PASS**, le 2026-10-25, sous VMware Workstation Pro, UEFI, 4 vCPU, sur l'ISO construite depuis `e7416f1` :

```text
[kernel] build_id=e7416f1c5612
[PASS] VMware 4 vCPU: Elyon 0.0.4 K003-01..12 et K004-01..11 validés sur build_id=e7416f1c5612
```

Synthèses finales relevées sous VMware :

```text
[smp] all_ready started=3 online=4 prepared=0 expected=4 trampoline=0x8000 sipi_vector=0x8
[smp] stability rounds=10 online=4 prepared=0 ipi_errors=0 min_heartbeat=10 max_heartbeat=10
[context] result phases=2 switches=4 online_before=4 online_after=4 prepared=0
[task] ids=1,2 unique=1 capacity=256 created=2 transitions=12 invalid_rejected=2 states_seen=0x1f
[task-stack] tasks=3 distinct=1 overlaps=0 owned=3 rounds=2 switches=12 args_ok=3 canaries_ok=3
[sched] policy=round-robin cooperative iterations=4 yields=9 exits=3 task_switches=11 bsp_switches=2 running_max=1 args_ok=3 rsp_ok=12 canaries_ok=3 queue_empty=1
[ioapic] legacy_routes=15 overrides_applied=1 shadowed=1 id_mismatches=1 masked=24/24 all_masked=1
[timer] ticks=100 eoi_failures=0 periodic=1
[preempt] policy=timer-preemptive quantum_ticks=1 target=30 preemptions=30 task_ticks=30 min_slices=10 counters=35813920,232501786,37612228 exits=3 running_max=1 eoi_failures=0 canaries_ok=3
[context-integrity] tasks=12,13,14 registers=15 df=1 preemptions=30 min_slices=10 busy_preemptions=10,10,10 iterations=2414525557,1077905825,31717414 corruptions=0 exits=3 running_max=1 eoi_failures=0 canaries_ok=3
[fpu-integrity] tasks=15,16,17 strategy=xsave avx=1 xmm=16 ymm_high=16 mxcsr=1 preemptions=30 min_slices=10 busy_preemptions=10,10,10 iterations=54303951,218122596,216968178 corruptions=0 exits=3 running_max=1 eoi_failures=0 canaries_ok=3
[percpu-sched] cpus=4 tasks_per_cpu=2 iterations=3 trace_entries=28 affinity_ok=1 rotation_ok=1 idle_ran=4 idle_running_aps=3 ap_fpu_ready=3 running_max=1 canaries_ok=12
[sched-ipi] cpus=4 placement_ok=1 ipi_sent=3 ipi_received=3 ipi_coalesced=0 blocks=1 wakes=1 migrations=1 contention=0 idle_running_aps=3 running_max=1 canaries_ok=3
[sched-sleep] cpus=4 hz=100 sleeps=6 timer_wakes=5 explicit_wakes=1 order_ok=1 never_early=1 max_late=0 ipi_sent=12 ipi_received=12 ipi_coalesced=0 timer_queue_empty=1 idle_running_aps=3 running_max=1 canaries_ok=7
[sched-sync] locks acquisitions=466,34994,68,120000 contention=0,388,0,222118 classes=pmm,run_queue,timer_queue,data switch_checks=1668
[sched-sync] cpus=4 ipi_sent=774 ipi_received=761 ipi_coalesced=13 idle_running_aps=3 running_max=1 canaries_ok=20
[stress] cpus=4 waves=9 tasks_per_wave=120 rounds=100 created=1080 exited=1080 reaped=1080 switches=99520
[stress] roles simd_rounds=27000 sleeps=2700 blocks=5400 wakes=5400 moves=90 corruptions=0 canary_failures=0 running_max=1
[stress] fairness min_progress=100 max_wait_switches=41 initial_wait=40 yield_wait=41 limit=64 idle_running_aps=3
[stress] reclaim frames_before=271582 frames_after=271582 slots_before=197 slots_after=197
```

La seule ligne d'avertissement est `[ioapic] WARNING id_mismatch madt_id=4 hw_id=0 policy=diagnostic-only` : l'écart entre l'ID IOAPIC de MADT et celui du registre matériel sous VMware, journalisé comme diagnostic depuis la 0.0.3.

## Ce que la 0.0.4 qualifie

Un scheduler noyau SMP sur 4 CPU, avec :

- tâches noyau à identifiants uniques et machine d'états vérifiée ;
- pile PMM propre à chaque tâche, avec canaris ;
- Round-Robin coopératif, puis préemption par le timer LAPIC ;
- sauvegarde complète du contexte et de l'état x87/SSE/AVX par tâche ;
- files par CPU, idle task par CPU, IPI de rescheduling, affinité et migration ;
- sommeil et réveil sur file temporelle, avec un ticket d'attente par sommeil ou blocage ;
- spinlocks irq-save à ordre de classes vérifié, aucun verrou à travers un switch ;
- récupération des tâches terminées, pages et slots réutilisés ;
- arrêt fail-closed de tous les CPU sur toute violation d'invariant.

Le mode utilisateur, le ring 3, les appels système, les processus isolés, le VFS, le réseau et l'interface graphique restent hors périmètre.

## Limites et dettes reportées

- Seul le timer du BSP est programmé : les AP ne sont pas préemptés, et une tâche d'AP qui ne cède jamais son CPU n'est pas détectée par la mesure de famine.
- Une file de CPU ou la file temporelle pleine (64 entrées) est une erreur fatale ; une contre-pression est nécessaire avant le ring 3.
- `running_count_on` parcourt les 256 slots à chaque switch.
- Le ticket d'attente tient sur 32 bits dans la clé de la file temporelle, soit environ 4 milliards d'attentes avant l'arrêt `TicketOverflow`.
- Les spinlocks ne sont pas équitables ; aucune famine n'a été mesurée sous le stress de K004-11.
- Les tâches des qualifications K004-01 à K004-10 ne sont pas récupérées et gardent leurs slots et leurs pages.
- Exigences de sécurité à satisfaire avant de qualifier les 0.0.5 et 0.0.6, inscrites dans `ROADMAP.md` à la racine du dépôt, section Sécurité : `swapgs` à chaque entrée depuis le ring 3, entrée « paranoïaque » NMI/#DF/#MC, RSP0 par tâche, aucune entrée utilisateur ne doit pouvoir arrêter le système, mémoire des processus prise par `allocate_owned`, masques `SFMASK`.
