# Roadmap Elyon Kernel 0.0.3

## Séquence de réalisation

### Phase 1 — ACPI utile au noyau

1. Valider la signature et le checksum du RSDP.
2. Préférer XSDT lorsqu'elle est disponible, sinon RSDT.
3. Parcourir les tables ACPI en validant longueur et checksum.
4. Parser MADT.
5. Extraire au minimum :
   - adresse du Local APIC ;
   - entrées Local APIC processeur ;
   - entrées x2APIC si présentes ;
   - IOAPIC ;
   - Interrupt Source Overrides ;
   - NMI utiles.

Critère de sortie : Elyon journalise une topologie cohérente et refuse proprement une MADT invalide.

### Phase 2 — Local APIC du BSP

1. Détecter le mode APIC/x2APIC supporté par CPUID.
2. Activer le Local APIC de façon contrôlée.
3. Lire et journaliser l'APIC ID du BSP.
4. Configurer le Spurious Interrupt Vector.
5. Préparer EOI et primitives d'accès APIC.

Critère de sortie : le BSP utilise le Local APIC Elyon sans régression du boot nominal.

### Phase 3 — IOAPIC

1. Mapper et identifier chaque IOAPIC découvert par MADT.
2. Lire la version et le nombre d'entrées de redirection.
3. Appliquer les Interrupt Source Overrides nécessaires.
4. Masquer les lignes non configurées par défaut.
5. Préparer la route d'au moins une IRQ contrôlée.

Critère de sortie : table de redirection construite à partir de la topologie ACPI, sans hypothèse matérielle codée en dur.

### Phase 4 — Timer

1. Choisir une source de calibration disponible.
2. Calibrer le timer Local APIC ou utiliser un mode approprié documenté.
3. Installer un vecteur timer dédié.
4. Compter les ticks par CPU.
5. Envoyer EOI correctement.

Critère de sortie : au moins 100 interruptions timer consécutives observées sans exception parasite ni blocage.

### Phase 5 — Structures per-CPU

Chaque CPU doit disposer au minimum de :

- APIC ID ;
- index logique Elyon ;
- pile noyau dédiée ;
- GDT dédiée ou état GDT sûr par CPU ;
- TSS dédiée ;
- pile IST double fault dédiée ;
- compteur de ticks ;
- état de démarrage ;
- emplacement futur pour le contexte scheduler.

Critère de sortie : aucune pile TSS/IST n'est partagée entre deux CPU actifs.

### Phase 6 — SMP

1. Réserver une zone trampoline sous 1 MiB explicitement gérée.
2. Préparer le trampoline AP.
3. Envoyer INIT puis SIPI selon le protocole x86.
4. Faire entrer chaque AP dans un point d'entrée Elyon 64-bit.
5. Installer son état per-CPU.
6. Journaliser `ELYON_AP_READY apic_id=<id> cpu=<n>`.

Critère de sortie : sous une VM à 2 vCPU, le BSP et au moins un AP atteignent leurs marqueurs respectifs. Sous 4 vCPU, Elyon doit démarrer les AP annoncés comme activés par MADT.

### Phase 7 — Préparation du scheduler

Le jalon ne doit pas encore sur-promettre un scheduler complet. Il doit préparer :

- structure de contexte sauvegardable ;
- idle thread par CPU ;
- primitive de changement de contexte ;
- point de préemption piloté par timer ;
- règles de verrouillage minimales pour données partagées.

Critère de sortie : le mécanisme peut être testé avec une tâche noyau de démonstration, sans déclarer encore l'espace utilisateur ou un ordonnanceur généraliste.

## Ordre obligatoire

`ACPI/MADT -> LAPIC -> IOAPIC -> timer -> per_cpu -> SMP -> scheduler foundation`

On ne démarre pas les AP avant d'avoir défini l'état per-CPU et les piles dédiées. On ne déclare pas la préemption avant d'avoir un timer validé.
