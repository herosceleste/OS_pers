# Elyon Kernel 0.0.3 — Qualification de clôture

## Décision

**K003-01 à K003-12 : PASS en qualification automatisée.**

**Validation croisée VMware Workstation Pro 4 vCPU : PASS** sur la branche `elyon-0.0.3-stabilize`, build_id `b1ee379af28b` (voir [Validation croisée VMware finale](#validation-croisée-vmware-finale-4-vcpu)).

La PR #2 a été fusionnée dans `main` le 2026-10-19 (`ca57e56`) ; la condition VMware décrite plus bas est désormais remplie sur la branche de stabilisation.

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


## Correctif de portabilité IOAPIC après premier boot VMware

Le premier boot croisé VMware 4 vCPU a validé le chemin SMP jusqu'à `ELYON_CONTEXT_SWITCH_READY`, puis a révélé une divergence entre l'ID IOAPIC fourni par MADT et celui lu dans le registre matériel. L'ancienne implémentation arrêtait le noyau sur cette seule divergence.

La politique a été corrigée : l'écart d'ID est désormais journalisé et compté comme diagnostic. Elyon continue de rester fail-closed sur les invariants fonctionnels : adresse MMIO invalide ou dupliquée, ID MADT dupliqué, plages GSI chevauchantes, nombre de redirections invalide, route hors IOAPIC ou read-back incohérent.

La CI GitHub Actions **#209** valide cette correction sous QEMU, où l'ID MADT et l'ID matériel restent identiques. Une nouvelle ISO VMware doit être construite depuis la branche courante et la validation croisée 4 vCPU doit être rejouée avant toute sortie de Draft ou fusion.

## Validation croisée VMware finale 4 vCPU

**Résultat : PASS.** La validation croisée a été rejouée le 2026-10-23 sur la branche `elyon-0.0.3-stabilize`.

### Baseline

- commit : `b1ee379af28b` (`ci: run host unit tests and deny clippy warnings for Elyon 0.0.3`)
- Rust : `rustc 1.99.1 (58cf8f9c3 2026-10-13)`
- ISO : `build/Elyon-Kernel-0.0.3-UEFI.iso`, produite par `scripts/build-vmware-iso.sh`
- hyperviseur : VMware Workstation Pro, firmware UEFI, 4 vCPU, journal série vers fichier

Empreintes des binaires embarqués dans l'ISO, identiques à ceux qualifiés sous QEMU dans la même session :

```text
4a1a46ce2295990a116c44bf108e188b7eda729b9ffa1f82683aafbbf03ef03b  build/esp/EFI/BOOT/BOOTX64.EFI
e39348f954d19106f01bcaa3fd06e2544795f6a31da03efb5f33daed7ca616f4  build/esp/ELYON/KERNEL.ELF
37acf1e68f1bf83c398c317abc333d60d7a129e942b98bd0596952a60caf9496  Elyon-Kernel-0.0.3-UEFI.iso
```

### Requalification QEMU préalable

Sur le même commit, en local (Ubuntu 24.04.1 sous WSL2, QEMU 8.2.2, OVMF `OVMF_CODE_4M.fd`, TCG) :

- `cargo fmt --check`, 91 tests host `elyon-core`/`elyon-abi` et clippy `-D warnings` sur les quatre crates : PASS ;
- `smoke-test.sh` et `test-release-gate-qemu.sh` 4 vCPU : PASS ;
- les onze scripts QEMU de la CI (ACPI, LAPIC, per-CPU, SMP 1 AP, SMP tous AP, stabilité SMP, changement de contexte, IOAPIC, timer, #UD, #DF) : PASS.

### Journal VMware

`scripts/validate-vmware-final.sh` conclut :

```text
[PASS] VMware 4 vCPU: Elyon 0.0.3 K003-01..12 validés sur build_id=b1ee379af28b
```

Marqueurs et synthèses observés :

```text
[acpi] cpu_count=4 usable=4 ioapics=1 isos=1 nmis=4
ELYON_AP_READY cpu=1 apic_id=1
ELYON_AP_READY cpu=2 apic_id=2
ELYON_AP_READY cpu=3 apic_id=3
[smp] all_ready started=3 online=4 prepared=0 expected=4 trampoline=0x8000 sipi_vector=0x8
[smp] stability rounds=10 online=4 prepared=0 ipi_errors=0 min_heartbeat=10 max_heartbeat=10
ELYON_SMP_READY
ELYON_SMP_STABLE
[context] result phases=2 switches=4 online_before=4 online_after=4 prepared=0
ELYON_CONTEXT_SWITCH_READY
ELYON_IOAPIC_READY
[timer] ticks=100 eoi_failures=0 periodic=1
ELYON_TIMER_READY
ELYON_KERNEL_READY_0_0_3
```

### Divergence d'ID IOAPIC

Le cas qui avait arrêté le premier boot VMware est reproduit et désormais traité en diagnostic :

```text
[ioapic] madt_id=4 hw_id=0 version=0x20 arbitration_id=0 address=0x00000000fec00000 gsi=0..23 redirs=24 id_match=0
[ioapic] WARNING id_mismatch madt_id=4 hw_id=0 policy=diagnostic-only
[ioapic] legacy_routes=15 overrides_applied=1 shadowed=1 id_mismatches=1 masked=24/24 all_masked=1
```

L'écart d'ID est journalisé, les 24 redirections restent masquées et le boot atteint `ELYON_KERNEL_READY_0_0_3` sans panic ni exception.
