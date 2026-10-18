# Validation Elyon Kernel 0.0.3

## Statut

**EN COURS — K003-01 à K003-05 sont validés.**

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
| K003-04 | Local APIC du BSP activé et APIC ID confirmé | **PASS** |
| K003-05 | IOAPIC découvert et redirections initiales maîtrisées | **PASS** |
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

## K003-04 — Local APIC du BSP

Le noyau détecte les capacités APIC via CPUID, lit `IA32_APIC_BASE`, conserve le mode xAPIC lorsque x2APIC n'est pas actif, vérifie que l'adresse MMIO correspond à celle annoncée par MADT, puis marque la page d'identité contenant le Local APIC comme non cacheable avant tout accès MMIO.

Le vecteur de spurious interrupt `0xFF` est installé dans l'IDT, le Software Enable du Local APIC est activé dans le SVR, le TPR est abaissé à zéro et la primitive EOI est installée.

La qualification a été exécutée sur le commit `916cda12807e74bd13063a02afb7900cce85f42b`, GitHub Actions run **#25**.

Preuves CI :

```text
[apic] cpuid apic=1 x2apic=0
[apic] IA32_APIC_BASE=0x00000000fee00000 madt_base=0x00000000fee00000 mode=xapic bsp=1
[apic] BSP apic_id=0 version=0x50014 madt_match=1
[apic] SVR vector=0xff software_enable=1
[apic] EOI primitive=OK
ELYON_LAPIC_READY
[PASS] K003-04 Local APIC BSP activé, APIC ID corrélé à MADT, SVR et EOI validés
```

L'APIC ID lu directement depuis le Local APIC est `0` et correspond à un processeur utilisable découvert dans MADT. Le processeur courant est bien identifié comme BSP.

**K003-04 : PASS.**

## K003-05 — IOAPIC

Elyon prend désormais le contrôle initial de l'IOAPIC décrit par MADT. Le noyau marque sa zone MMIO comme non cacheable, lit les registres d'identité, de version et d'arbitrage, calcule le nombre réel d'entrées de redirection, vérifie la cohérence de l'identifiant matériel avec MADT et contrôle les plages GSI.

Avant toute activation d'IRQ, toutes les entrées de redirection sont explicitement masquées. Elyon construit ensuite les routes IRQ héritées vers le BSP en conservant le bit de masque actif. Les `Interrupt Source Overrides` ACPI sont appliqués pour la GSI, la polarité et le mode edge/level. Lorsqu'un override réutilise une GSI qui correspondrait à une route ISA par défaut, la route par défaut conflictuelle est volontairement écartée au lieu d'écraser la route ACPI.

La qualification a été exécutée sur le commit `e84a119cb43807ec0730e7fad45f3beaf9db07bc`, GitHub Actions run **#43**.

Preuves CI :

```text
[ioapic] controllers=1 bsp_apic_id=0
[ioapic] madt_id=0 hw_id=0 version=0x20 arbitration_id=0 address=0x00000000fec00000 gsi=0..23 redirs=24 id_match=1
[ioapic] route irq=0 gsi=2 vector=0x20 polarity=high trigger=edge masked=1 override=1
[ioapic] route irq=5 gsi=5 vector=0x25 polarity=high trigger=level masked=1 override=1
[ioapic] route irq=9 gsi=9 vector=0x29 polarity=high trigger=level masked=1 override=1
[ioapic] route irq=10 gsi=10 vector=0x2a polarity=high trigger=level masked=1 override=1
[ioapic] route irq=11 gsi=11 vector=0x2b polarity=high trigger=level masked=1 override=1
[ioapic] legacy_routes=15 overrides_applied=5 shadowed=1 masked=24/24 all_masked=1
ELYON_IOAPIC_READY
[PASS] K003-05 IOAPIC lu, redirections masquées et overrides MADT appliqués
```

L'IOAPIC de qualification expose 24 entrées couvrant les GSI 0 à 23. Les 24 entrées ont été relues comme masquées après programmation. Cinq overrides MADT ont été appliqués et la route ISA 2 a été écartée parce que la GSI 2 est affectée à l'IRQ 0 par ACPI.

Aucune IRQ externe n'est encore démasquée dans ce jalon. Cette politique est volontaire : le noyau ne rendra une ligne active qu'après installation de son handler et de sa stratégie EOI.

**K003-05 : PASS.**

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

Le prochain critère est **K003-06 : mettre en place un timer noyau fiable, installer un vecteur d'interruption dédié, compter des ticks répétables, envoyer les EOI correctement et prouver au moins 100 interruptions consécutives sans exception parasite.**
