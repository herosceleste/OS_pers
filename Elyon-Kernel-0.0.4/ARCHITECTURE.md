# Architecture Elyon Kernel 0.0.3

## 1. Chaîne de découverte matérielle

Le noyau part du `acpi_rsdp` transmis par le loader. La 0.0.3 ne doit pas rescanner arbitrairement la mémoire à la recherche d'un RSDP si le loader en a déjà fourni un valide.

Chaîne cible :

`BootInfo.acpi_rsdp -> RSDP -> XSDT/RSDT -> MADT -> topologie APIC`

Chaque structure ACPI lue doit être bornée par sa longueur déclarée, validée par signature lorsqu'elle est attendue, puis vérifiée par checksum avant exploitation.

## 2. Modèle APIC

Elyon doit abstraire le contrôleur d'interruptions local derrière une interface noyau qui permette de supporter d'abord xAPIC, puis x2APIC sans disséminer des accès MMIO ou MSR dans tout le code.

Responsabilités minimales :

- lire l'APIC ID courant ;
- activer le contrôleur local ;
- configurer le Spurious Interrupt Vector ;
- programmer le timer ;
- envoyer EOI ;
- envoyer INIT/SIPI pour le démarrage SMP.

Le choix xAPIC/x2APIC doit être déterminé par CPUID et par la configuration réellement disponible, puis journalisé.

## 3. IOAPIC

Les IOAPIC sont décrits par MADT. Elyon ne doit pas supposer une adresse fixe ni un seul IOAPIC.

Une structure `IoApicDescriptor` devra au minimum porter :

- identifiant ACPI ;
- base MMIO ;
- Global System Interrupt Base ;
- nombre d'entrées de redirection découvert à l'exécution.

Les Interrupt Source Overrides doivent être appliqués avant de router les IRQ héritées.

## 4. Vecteurs IDT

Les vecteurs CPU 0..31 restent réservés aux exceptions.

Une plage explicite doit être réservée aux interruptions externes et au timer. Le choix exact sera centralisé dans un module de constantes afin d'éviter les collisions lors de l'arrivée du scheduler, des IPIs et des périphériques.

Aucune IRQ matérielle ne doit être activée avant que son entrée IDT et sa politique d'EOI soient installées.

## 5. État per-CPU

Structure cible indicative :

```text
PerCpu
  logical_id
  apic_id
  online
  kernel_stack
  gdt
  tss
  double_fault_ist
  ticks
  scheduler_state
```

Les pointeurs ou références exacts dépendront du modèle mémoire retenu, mais les invariants sont obligatoires :

- une pile noyau distincte par CPU ;
- une TSS distincte par CPU ;
- une IST double fault distincte par CPU ;
- aucun AP déclaré online avant installation complète de son état local.

Le mécanisme d'accès rapide à `PerCpu` pourra évoluer vers GS-base, mais la 0.0.3 peut commencer par une table indexée tant que le chemin critique reste correct et testable.

## 6. Trampoline AP

Le démarrage x86 des AP impose un code trampoline dans une zone adressable par SIPI. Cette zone doit être explicitement réservée par le PMM au lieu de rendre à nouveau toute la mémoire basse allocatable.

Le trampoline devra :

1. partir du mode imposé par le protocole SIPI ;
2. établir l'environnement minimal nécessaire ;
3. rejoindre le mode long ;
4. charger les tables fournies par Elyon ;
5. basculer sur la pile dédiée de l'AP ;
6. appeler un point d'entrée Rust avec l'identité du CPU.

Le trampoline et ses données de bootstrap doivent être documentés comme une exception contrôlée à la politique de réservation de la mémoire basse.

## 7. Synchronisation SMP

Avant le démarrage des AP, le BSP prépare une table de bootstrap par CPU. Chaque AP passe par des états explicites, par exemple :

`Discovered -> Starting -> Online -> Failed`

Le BSP applique un timeout borné et ne doit pas attendre indéfiniment un AP absent ou défaillant.

Pour 0.0.3, les primitives de synchronisation peuvent rester minimales, mais toute donnée partagée mutable utilisée après mise online de plusieurs CPU doit avoir une stratégie explicite de verrouillage ou d'accès atomique.

## 8. Timer et temps noyau

Le compteur de ticks doit être par CPU pour la validation initiale. Un temps global ou monotone inter-CPU pourra être construit ensuite.

La calibration doit être séparée de la programmation périodique. Le journal de validation doit indiquer la source utilisée et le résultat de calibration, sans prétendre à une précision absolue tant que celle-ci n'a pas été mesurée.

## 9. Scheduler foundation

Le timer fournira le futur point de préemption, mais la 0.0.3 s'arrête à la fondation :

- contexte sauvegardable ;
- idle context par CPU ;
- primitive de switch testable ;
- hook timer vers scheduler désactivable.

Le passage à un ordonnanceur généraliste, aux processus utilisateurs et aux priorités appartient au jalon suivant.

## 10. Invariants de sécurité

Toute étape SMP doit préserver les garanties 0.0.2 :

- exceptions fail-closed ;
- #DF avec IST ;
- PMM sans allocation implicite sous 1 MiB ;
- tables de pages possédées par Elyon ;
- aucune activation d'interruption avant installation de son handler ;
- aucune structure critique partagée entre CPU sans intention documentée.

Un échec ACPI, APIC, timer ou SMP doit produire un diagnostic exploitable et revenir à un état sûr, ou arrêter le noyau lorsque continuer serait ambigu.
