# Validation Elyon Kernel 0.0.3

## Statut

**EN COURS — K003-01 à K003-09 sont validés.**

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
| K003-06 | Timer noyau produit des interruptions répétables | **PASS** |
| K003-07 | État per-CPU avec pile noyau, TSS et IST dédiées | **PASS** |
| K003-08 | Au moins un AP est réellement démarré par INIT/SIPI | **PASS** |
| K003-09 | Chaque AP atteint `ELYON_AP_READY` avec son APIC ID | **PASS** |
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

## K003-06 — Timer noyau

Elyon utilise désormais le timer du Local APIC comme source d'interruptions périodiques du noyau. Sa fréquence effective est calibrée au démarrage à l'aide du canal 2 du PIT, utilisé uniquement comme référence temporelle en polling pendant une fenêtre d'environ 10 ms.

La séquence de qualification est volontairement stricte :

- le PIC 8259 maître et esclave est entièrement masqué avant `STI` afin d'éviter toute IRQ héritée non maîtrisée ;
- le vecteur `0xE0` est réservé au timer LAPIC dans l'IDT ;
- le timer LAPIC est d'abord lancé masqué avec un diviseur 16 pour mesurer son décrément pendant la fenêtre PIT ;
- Elyon calcule ensuite le compteur initial nécessaire pour une cadence cible de 100 Hz ;
- le LAPIC est programmé en mode périodique ;
- le noyau active les interruptions, attend 100 interruptions réelles puis les désactive ;
- chaque interruption incrémente un compteur atomique et envoie un EOI au Local APIC ;
- le test échoue si le compteur EOI signale la moindre anomalie ;
- le timer est finalement remasqué avant la poursuite du boot de qualification.

La qualification a été exécutée sur le commit `019cd3cfcc61cbb84484b3be7e1786ed85964ea2`, GitHub Actions run **#66**.

Preuves CI :

```text
[timer] source=PIT2 pit_count=11932 lapic_elapsed=629888 divide=16 effective_hz=62987849 initial_count=629878 target_hz=100
[timer] vector=0xe0 legacy_pic_masked=1 qualification_target=100
[timer] ticks=100 eoi_failures=0 periodic=1
ELYON_TIMER_READY
ELYON_KERNEL_READY_0_0_3
[PASS] K003-06 timer LAPIC périodique 100 Hz, 100 ticks et EOI validés
```

Les différentes exécutions du même run ont produit une fréquence LAPIC effective proche de 63 MHz après division par 16, avec une variation normale liée à la fenêtre de calibration. Le compteur initial est dérivé de la mesure et non codé en dur.

Le handler timer préserve les registres généraux, appelle un dispatch Rust minimal sans journalisation dans le chemin d'interruption, incrémente le compteur puis acquitte l'interruption via EOI avant le retour par `iretq`.

**K003-06 : PASS.**

## K003-07 — État per-CPU

Elyon dispose désormais d'un état per-CPU borné à 64 slots. Le BSP occupe le slot logique 0 et conserve la pile noyau dédiée fournie par le loader. Chaque AP découvert comme utilisable dans MADT reçoit avant tout démarrage SMP :

- un identifiant logique Elyon ;
- son APIC ID ;
- un état explicite `prepared` ;
- une pile noyau dédiée de 64 KiB allouée par le PMM ;
- une IST double fault dédiée de 16 KiB allouée par le PMM ;
- sa propre GDT ;
- sa propre TSS ;
- un compteur de ticks atomique.

Le BSP utilise réellement la GDT/TSS/IST du slot 0. Le registre TR est relu et comparé au sélecteur TSS attendu. Les AP ne sont pas encore démarrés : leurs slots restent `prepared`, ce qui évite de confondre provisionnement et succès SMP.

Une première version de l'implémentation réservait statiquement toutes les piles AP et IST dans le `.bss`. La qualification a montré que cette stratégie grossissait inutilement l'image noyau et pouvait empêcher le loader UEFI de réserver sa plage physique fixe. Elle a été remplacée par une allocation PMM contiguë, ce qui rend la propriété mémoire explicite et prépare directement le trampoline SMP.

La qualification finale a été exécutée sur le commit `8a4e8e2c9776bdbdae010aebf18f631ca1dd2c0f`, GitHub Actions run **#108**, avec une VM QEMU à 4 vCPU.

Preuves CI :

```text
[percpu] cpus=4 capacity=64 bsp_logical=0 bsp_apic_id=0 unique=1 owned=1 tr_loaded=1
[percpu] cpu=0 apic_id=0 state=online stack=0x000000003e29f000..0x000000003e2af000 gdt=0x0000000000309000 tss=0x000000000030a200 ist=0x000000000030d3a0..0x00000000003113a0 ticks=0
[percpu] cpu=1 apic_id=1 state=prepared stack=0x0000000000102000..0x0000000000112000 gdt=0x0000000000309048 tss=0x000000000030a268 ist=0x0000000000112000..0x0000000000116000 ticks=0
[percpu] cpu=2 apic_id=2 state=prepared stack=0x0000000000116000..0x0000000000126000 gdt=0x0000000000309090 tss=0x000000000030a2d0 ist=0x0000000000126000..0x000000000012a000 ticks=0
[percpu] cpu=3 apic_id=3 state=prepared stack=0x000000000012a000..0x000000000013a000 gdt=0x00000000003090d8 tss=0x000000000030a338 ist=0x000000000013a000..0x000000000013e000 ticks=0
ELYON_PERCPU_READY
[percpu] bsp_ticks=100 timer_ticks=100 match=1
[PASS] K003-07 4 slots per-CPU uniques, BSP online/TSS chargé, 3 AP préparés et ticks BSP validés
```

Les tests vérifient également que les quatre piles noyau, GDT, TSS et IST sont distinctes. Les GDT/TSS restent possédées par l'image noyau ; les piles et IST des AP sont possédées par le PMM et restent sous la limite d'identity mapping actuelle. Le compteur de ticks per-CPU du BSP a été corrélé au timer LAPIC : 100 ticks per-CPU pour 100 interruptions timer.

**K003-07 : PASS.**

## K003-08 — Démarrage réel d’un AP par INIT/SIPI

Elyon peut désormais démarrer réellement un Application Processor x86 par la séquence architecturale `INIT + SIPI`.

Le BSP copie un trampoline minimal dans une page basse fixe à `0x8000`. Cette page est d'abord vérifiée comme mémoire UEFI utilisable avant toute écriture. Le trampoline :

- démarre dans le mode réel imposé par SIPI ;
- charge une GDT temporaire basse ;
- entre en mode protégé 32 bits ;
- active PAE ;
- charge le CR3 d'Elyon ;
- active `EFER.LME` puis la pagination ;
- effectue le saut vers le code 64 bits ;
- charge la pile noyau du slot per-CPU cible ;
- appelle le point d'entrée Rust de l'AP.

Le BSP envoie un INIT, attend 10 ms via le canal 2 du PIT en polling, envoie un premier SIPI puis un second si nécessaire. Le vecteur SIPI utilisé est `0x08`, correspondant à la page physique `0x8000`.

Une fois en Rust, l'AP charge sa propre GDT, sa propre TSS et donc sa propre IST double fault, recharge l'IDT commune, réactive son Local APIC, lit son APIC ID matériel et vérifie qu'il correspond au slot préparé. Le slot ne passe de `prepared` à `online` qu'après ces vérifications.

La qualification finale a été exécutée sur le commit `27bc6746c9b16efaf588d0d634edb6065b68bf2a`, GitHub Actions run **#132**, avec une VM QEMU à 2 vCPU pour isoler la preuve K003-08.

Preuves CI :

```text
[smp] AP online cpu=1 apic_id=1 selector=0x8 tr=0x18
ELYON_AP_STARTED cpu=1 apic_id=1
[smp] trampoline=0x8000 sipi_vector=0x8 cpu=1 apic_id=1 online=2 prepared=0
ELYON_AP_BOOT_READY
ELYON_KERNEL_READY_0_0_3
[PASS] K003-08 un AP a réellement démarré par INIT/SIPI et atteint Elyon en mode long
```

Le sélecteur de code de l'AP est `0x08` et son registre `TR` vaut `0x18`, ce qui confirme que la GDT/TSS per-CPU a été chargée avant le passage du slot à `online`.

Le même run a également validé la configuration 4 vCPU utilisée par K003-07. Dans ce scénario, Elyon démarre volontairement un seul AP à ce stade ; les autres restent `prepared`. Leur démarrage généralisé appartient à K003-09.

**K003-08 : PASS.**

## K003-09 — Tous les AP atteignent Elyon

Le bootstrap SMP n'est plus limité à un seul Application Processor. Le BSP parcourt désormais tous les slots per-CPU encore `prepared`, réutilise séquentiellement le trampoline bas à `0x8000` et applique pour chacun la même séquence INIT/SIPI déjà qualifiée en K003-08.

Avant cette généralisation, la sortie COM1 a été sérialisée par un verrou atomique SMP. Chaque appel de journalisation désactive localement les interruptions, acquiert le verrou, écrit une ligne complète puis restaure l'état IF initial. Cette protection supprime l'entrelacement de lignes observé pendant les premières qualifications 4 vCPU.

Chaque AP doit satisfaire les mêmes contrôles qu'en K003-08 : passage en mode long, pile per-CPU, GDT/TSS/IST dédiée, Local APIC secondaire activé, APIC ID matériel conforme au slot, puis transition atomique `prepared -> online`. Un marqueur `ELYON_AP_READY cpu=<n> apic_id=<id>` est émis uniquement après cette transition.

La qualification finale a été exécutée sur le commit `7ec0a63dfa3c829063a562e60eb88abb483c5212`, GitHub Actions run **#146**, avec une VM QEMU à 4 vCPU.

Preuves CI :

```text
ELYON_AP_READY cpu=1 apic_id=1
ELYON_AP_READY cpu=2 apic_id=2
ELYON_AP_READY cpu=3 apic_id=3
[smp] all_ready started=3 online=4 prepared=0 expected=4 trampoline=0x8000 sipi_vector=0x8
ELYON_SMP_READY
ELYON_KERNEL_READY_0_0_3
[PASS] K003-09 tous les AP MADT ont atteint ELYON_AP_READY et online=4 prepared=0
```

Le test ne se contente pas de compter trois marqueurs. Il extrait les trois slots initialement `prepared` depuis le journal per-CPU, récupère leurs couples `cpu/apic_id`, puis exige un marqueur `ELYON_AP_READY` correspondant pour chacun. Il vérifie ensuite exactement trois AP ready, `online=4`, `prepared=0`, l'absence de `[smp] AP FAILED` et la survie du BSP jusqu'à `ELYON_KERNEL_READY_0_0_3`.

Le même run a également confirmé les régressions K003-01 à K003-08, le timer LAPIC, l'IOAPIC, ainsi que les tests fail-closed #UD et #DF/IST.

**K003-09 : PASS.**

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

Le prochain critère est **K003-10 : maintenir le BSP stable après activation SMP, conserver tous les CPU online pendant une fenêtre de qualification, vérifier qu'aucun AP ne régresse ou ne redémarre et préparer un véritable état idle SMP avant la fondation du changement de contexte.**
