# Elyon Kernel 0.0.5 — Architecture cible

## Héritage 0.0.4

Elyon dispose d'un loader UEFI séparé, d'un PMM à pages rendables par jeton, d'une pagination initiale propre au noyau, d'ACPI/MADT, LAPIC/IOAPIC, d'un état per-CPU lu via GS et d'un bootstrap SMP INIT/SIPI. Au-dessus, un ordonnanceur noyau SMP : tâches à pile dédiée et canaris, contexte CPU et état x87/SSE/AVX par tâche, files et idle task par CPU, préemption par le timer du BSP, IPI de rescheduling, file temporelle, spinlocks irq-save à ordre de classes vérifié, récupération des tâches terminées et arrêt fail-closed de tous les CPU par NMI.

## Frontière ring 3 / ring 0

La 0.0.5 ajoute une seconde frontière de confiance, entre le noyau et un processus qu'il ne contrôle pas :

1. **Entrées** : toute entrée depuis le ring 3 (appel système, IRQ, exception) exécute `swapgs` avant de toucher à l'état per-CPU, prend la pile noyau `RSP0` de la tâche courante et n'a confiance dans aucun registre utilisateur. NMI, #DF et #MC ne se fient pas au CS sauvegardé pour décider du `swapgs`.
2. **Mémoire** : SMEP et SMAP empêchent le noyau d'exécuter ou de lire par erreur une page utilisateur ; les seuls accès passent par des copies qui valident l'adresse, la longueur et la permission.
3. **Appels système** : `syscall`/`sysret` avec `SFMASK` qui efface IF, DF et AC à l'entrée ; vérification d'un RIP canonique avant `sysret` ; registres non retournés effacés à la sortie.
4. **Fautes** : une faute atteignable depuis le ring 3 tue le processus et rend le CPU au scheduler ; `halt_system` reste réservé aux invariants du noyau.

## Hors périmètre

Un espace d'adressage par processus (CR3), les fautes de page à la demande, `mmap`, le VFS, le chargeur ELF, les drivers de stockage, le réseau et la GUI ne font pas partie du jalon 0.0.5.
