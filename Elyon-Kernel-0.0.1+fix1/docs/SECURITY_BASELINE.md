# Baseline securite 0.0.1

Cette baseline fixe les proprietes minimales du premier noyau Elyon.

1. Aucun noyau Linux, BSD ou Windows n'est embarque.
2. Les interruptions sont desactivees pendant la prise de controle des tables CPU.
3. Une GDT propre a Elyon est chargee avant le signal de disponibilite du noyau.
4. Une IDT propre a Elyon est chargee. Les exceptions 0 a 31 utilisent un handler fail-closed qui bloque le CPU.
5. La carte memoire utilisee est celle retournee au moment de `ExitBootServices`.
6. Les zones `LOADER_CODE` et `LOADER_DATA` ne sont pas liberees dans 0.0.1.
7. Le noyau n'alloue pas dynamiquement apres la sortie des Boot Services.
8. Les paniques sont journalisees sur la console serie puis conduisent a un arret controle.
9. Aucun reseau, telemetrie ou service externe n'est present dans 0.0.1.
10. Les dependances sont volontairement reduites au minimum et doivent etre verrouillees avant release publique.

Cette version n'est pas encore un produit securise pour production. Elle constitue une fondation de bootstrap a auditer.
