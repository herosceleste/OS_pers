# Elyon Kernel 0.0.3 — Qualification de clôture

## Décision

**K003-01 à K003-12 : PASS en qualification automatisée.**

La PR #2 reste en Draft jusqu'à la validation croisée VMware Workstation Pro 4 vCPU.

## Baseline qualifiée

- branche : `elyon-0.0.3`
- commit contenant le gate final : `0bebda3e98aa2b9fa4230e15cbfb986b1ef604c7`
- GitHub Actions : Elyon Kernel 0.0.3 CI, run **#196**
- merge commit construit par le workflow PR : `68c424e8b7dc`
- environnement : Ubuntu 24.04, QEMU/OVMF, 4 vCPU
- Rust : `rustc 1.99.1 (58cf8f9c3 2026-10-13)`

## Empreintes des artefacts CI #196

```text
e0f408309828358c4a16d578e1b97f3d08ff093d3ffbc6a6b4388fa5fd83dc8f  build/esp/EFI/BOOT/BOOTX64.EFI
e8718a012f3a196c46bca4226700a1535fbb221ea9d72c319e3afa7088b73b1e  build/esp/ELYON/KERNEL.ELF
```

Ces empreintes identifient les binaires produits dans cette qualification. La reproductibilité bit à bit entre environnements n'est pas revendiquée à ce stade.

## Gate nominal 4 vCPU

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
ELYON_IOAPIC_READY
ELYON_TIMER_READY
ELYON_KERNEL_READY_0_0_3
```

Synthèses finales :

```text
[smp] all_ready started=3 online=4 prepared=0 expected=4 trampoline=0x8000 sipi_vector=0x8
[smp] stability rounds=10 online=4 prepared=0 ipi_errors=0 min_heartbeat=10 max_heartbeat=10
[context] result phases=2 switches=4 online_before=4 online_after=4 prepared=0
[ioapic] legacy_routes=15 overrides_applied=5 shadowed=1 masked=24/24 all_masked=1
[timer] ticks=100 eoi_failures=0 periodic=1
```

Conclusion du gate :

```text
[PASS] K003-12 qualification finale 0.0.3: K003-01..11 cohérents sur 4 vCPU, boot nominal sans anomalie
```

## Régressions fail-closed

```text
[PASS] #UD diagnostiqué puis arrêté en fail-closed (qemu rc=124)
[PASS] #DF pris en charge sur l'IST dédiée puis arrêt fail-closed (qemu rc=124)
```

Le code de retour 124 est attendu : les tests utilisent `timeout` pour terminer QEMU après l'arrêt fail-closed du noyau.

## Condition restante avant fusion

Effectuer une validation croisée sous VMware Workstation Pro avec 4 vCPU. La fusion ne doit être proposée qu'après confirmation des marqueurs suivants :

```text
ELYON_AP_READY cpu=1
ELYON_AP_READY cpu=2
ELYON_AP_READY cpu=3
ELYON_SMP_READY
ELYON_SMP_STABLE
ELYON_CONTEXT_SWITCH_READY
ELYON_TIMER_READY
ELYON_KERNEL_READY_0_0_3
```

Aucune fonctionnalité de scheduler généraliste, de préemption de tâches, de sauvegarde FPU/SIMD complète ou de politique d'ordonnancement n'est revendiquée dans Elyon 0.0.3.
