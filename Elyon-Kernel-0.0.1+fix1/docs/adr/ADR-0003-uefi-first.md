# ADR-0003, UEFI comme chemin de boot initial

Statut : accepte.

Elyon 0.0.x demarre d'abord sur x86-64 UEFI. Le firmware sert de mecanisme d'amorcage. Les services de boot ne font pas partie de l'environnement permanent du noyau et sont quittes explicitement.
