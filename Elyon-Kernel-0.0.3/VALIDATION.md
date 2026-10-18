# Validation Elyon Kernel 0.0.3

## Statut

**EN COURS — K003-01, K003-02 et K003-03 sont validés.**

Baseline de départ : fusion 0.0.2 dans `main`, commit `303feeeff93b4ed1b5b3092cc16a4d8c4489f252`.

Première qualification ACPI :

- commit de code qualifié : `5d52192a85d41dd8ae694abad55f58cc14fd8115`
- GitHub Actions : run **#4**
- environnement : Ubuntu 24.04 + QEMU/OVMF
- toolchain : Rust 1.99.1
- VM de test ACPI : 2 vCPU

## Critères

| ID | Critère | État |
|---|---|---|
| K003-01 | RSDP validé par signature et checksum | **PASS** |
| K003-02 | XSDT/RSDT parcourue avec validation des tables | **PASS** |
| K003-03 | MADT parsée et topologie CPU/APIC journalisée | **PASS** |
| K003-04 | Local APIC du BSP activé et APIC ID confirmé | À faire |
| K003-05 | IOAPIC découvert et redirections initiales maîtrisées | À faire |
| K003-06 | Timer noyau produit des interruptions répétables | À faire |
| K003-07 | État per-CPU avec pile noyau, TSS et IST dédiées | À faire |
| K003-08 | Au moins un AP est réellement démarré par INIT/SIPI | À faire |
| K003-09 | Chaque AP atteint `ELYON_AP_READY` avec son APIC ID | À faire |
| K003-10 | Le BSP reste stable après activation SMP | À faire |
| K003-11 | Base de changement de contexte noyau testée | À faire |
| K003-12 | CI nominale et SMP sans régression des critères 0.0.2 | À faire |

## K003-01 — RSDP

Le noyau utilise exclusivement le pointeur RSDP transmis par le loader via `BootInfo.acpi_rsdp`. Le parseur vérifie :

- la signature `RSD PTR ` ;
- le checksum ACPI 1.0 sur les 20 premiers octets ;
- pour ACPI 2.0+, la longueur étendue et le checksum sur la structure complète ;
- que la structure reste dans la plage physique actuellement identity-mappée par Elyon.

Preuve CI :

```text
[acpi] RSDP=OK revision=2 root=XSDT address=0x000000003fb7d0e8
```

**K003-01 : PASS.**

## K003-02 — XSDT/RSDT et validation des SDT

Pour ACPI 2.0+, Elyon préfère XSDT lorsqu'une adresse XSDT est exposée. RSDT reste pris en charge comme repli lorsque XSDT n'est pas disponible.

Chaque table référencée par la table racine est bornée, sa longueur est contrôlée, son checksum est vérifié avant exploitation et toute adresse hors de la zone actuellement mappée provoque un arrêt explicite.

Preuve CI :

```text
[acpi] root=XSDT entries=6 validated=6
```

Les six SDT référencées par la XSDT de l'environnement de qualification ont été parcourues et validées.

**K003-02 : PASS.**

## K003-03 — MADT et topologie APIC

Le parseur MADT gère actuellement les entrées utiles à la suite du jalon :

- Processor Local APIC, type 0 ;
- IO APIC, type 1 ;
- Interrupt Source Override, type 2 ;
- NMI Source, type 3 ;
- Local APIC NMI, type 4 ;
- Local APIC Address Override, type 5 ;
- Processor Local x2APIC, type 9 ;
- Local x2APIC NMI, type 10.

Les entrées sont vérifiées par longueur avant lecture et stockées dans des capacités bornées. Une structure invalide ou une capacité dépassée entraîne une erreur explicite plutôt qu'une continuation ambiguë.

Preuves CI avec 2 vCPU :

```text
[acpi] MADT=OK address=0x000000003fb78000 lapic=0x00000000fee00000 flags=0x1
[acpi] cpu_count=2 usable=2 ioapics=1 isos=5 nmis=1
[acpi] cpu uid=0 apic_id=0 flags=0x1 x2apic=false enabled=true online_capable=false
[acpi] cpu uid=1 apic_id=1 flags=0x1 x2apic=false enabled=true online_capable=false
[acpi] ioapic id=0 address=0x00000000fec00000 gsi_base=0
ELYON_ACPI_READY
```

Le test automatisé conclut :

```text
[PASS] K003-01/K003-02/K003-03 RSDP, XSDT/RSDT et MADT validés
```

**K003-03 : PASS.**

## Régressions 0.0.2

La même CI a confirmé que l'introduction du parseur ACPI ne casse pas la baseline précédente :

```text
[PASS] Elyon Kernel 0.0.3 a atteint ELYON_KERNEL_READY_0_0_3
[PASS] #UD diagnostiqué puis arrêté en fail-closed
[PASS] #DF pris en charge sur l'IST dédiée puis arrêt fail-closed
```

Les protections PMM bas-mémoire, le CR3 Elyon, les diagnostics d'exception et le chemin #DF/IST restent donc actifs sur ce jalon.

## Preuves exigées

Un critère ne passe à PASS que si les trois éléments suivants existent :

1. code présent sur un commit identifié ;
2. test exécuté dans un environnement identifié ;
3. journal ou assertion CI démontrant le comportement attendu.

La simple présence de structures ACPI, APIC, TSS ou d'un trampoline dans le code ne constitue pas une preuve de fonctionnement.

## Environnements de qualification

- GitHub Actions + QEMU/OVMF pour les tests automatisables ;
- VMware Workstation Pro pour une validation croisée sur 2 puis 4 vCPU ;
- Rust 1.99.1 tant qu'une montée de toolchain n'est pas explicitement décidée et qualifiée.

## Prochaine étape

Le prochain critère est **K003-04 : activer le Local APIC du BSP, confirmer l'APIC ID courant et installer les primitives EOI / Spurious Interrupt Vector sans régression du boot nominal.**
