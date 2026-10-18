# Elyon Kernel 0.0.3

## Nom du jalon

**Interrupts & SMP Foundation**

Elyon Kernel 0.0.3 part de la baseline 0.0.2 validée et fusionnée dans `main`. Ce jalon introduit les fondations matérielles nécessaires au multiprocesseur et à la préemption, sans encore déclarer un scheduler complet de production.

## Objectifs

Le périmètre 0.0.3 couvre :

- parsing ACPI minimal utile au noyau, avec priorité à RSDP, XSDT/RSDT et MADT ;
- découverte fiable des processeurs logiques et de leurs APIC IDs ;
- initialisation du Local APIC du BSP ;
- découverte et préparation des IOAPIC ;
- mise en place d'un timer noyau fiable ;
- démarrage des Application Processors par séquence INIT/SIPI ;
- création de structures `per_cpu` ;
- GDT, TSS, pile noyau et pile IST propres à chaque CPU ;
- base technique du scheduler : contexte CPU, idle thread et point de changement de contexte, sans annoncer encore un ordonnanceur complet.

## Principes hérités de 0.0.2

La 0.0.3 doit conserver les propriétés déjà validées :

- loader UEFI et kernel ELF séparés ;
- sortie des UEFI Boot Services avant l'entrée noyau ;
- BootInfo versionné et validé ;
- PMM protégé contre l'allocation sous 1 MiB ;
- pagination Elyon et CR3 propre ;
- exceptions CPU diagnostiques en fail-closed ;
- TSS et IST double fault opérationnels ;
- build ID dérivé du commit ;
- validation CI obligatoire avant promotion du jalon.

## Contraintes d'architecture

Le BSP ne doit pas partager sa TSS ou ses piles d'exception avec les AP. Chaque CPU démarré doit disposer de son propre état noyau minimal. Aucun succès SMP ne sera déclaré sur la seule détection ACPI : un AP doit réellement atteindre un marqueur série propre à son APIC ID.

Le timer doit générer des interruptions observables et comptées. La simple programmation d'un registre APIC ne constitue pas une validation.

Le scheduler complet, l'espace utilisateur, les syscalls, W^X/NX final, l'IOMMU et Secure Boot restent hors périmètre de ce jalon.
