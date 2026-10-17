# Limites connues de Elyon Kernel 0.0.1

Ce document formalise les ecarts entre la baseline d'architecture initiale et le jalon 0.0.1 effectivement prouve. Il sert de garde-fou pour la 0.0.2.

## Statut du jalon

Le tag historique `v0.0.1` pointe sur le premier bootstrap livre. Ce commit ne compilait pas en raison d'une incompatibilite de taille de `GlobalDescriptorTable` et sa CI comportait deux autres defauts bloquants. Il est conserve pour la tracabilite et ne doit pas etre utilise comme release de reference.

Le correctif du 17/10/2026 corrige la compilation GDT, le formatage `rustfmt`, le boot OVMF en `pflash` sous Ubuntu 24.04 et deux avertissements du compilateur. La validation externe fournie a atteint `ELYON_KERNEL_READY` sous QEMU/OVMF avec Rust 1.97.0 et a confirme le comportement fail-closed par injection `ud2`.

## Ecarts par rapport aux criteres d'architecture

- K003 : la carte memoire UEFI est copiee et classee, mais n'est pas encore validee pour les chevauchements, l'ordre et l'alignement.
- K005 : les exceptions 0..31 sont interceptees en fail-closed, mais sans diagnostic de vecteur, code d'erreur ou contexte CPU.
- K006 : aucun allocateur physique de pages Elyon n'est encore actif.
- K007 : Elyon utilise encore les tables de pagination heritees du firmware ; il ne possede pas encore ses propres tables de pages.
- K009 : le binaire affiche sa version mais n'embarque pas encore de build ID cryptographique ou derive du commit.
- K011 : partiellement satisfait. `Cargo.lock` est versionne et le build utilise `--locked`. La reproductibilite bit a bit de `BOOTX64.EFI` avec Rust 1.99.1 sur l'environnement de reference reste a demontrer et a consigner. Constat du 17/10/2026 avec Rust 1.97.0 : deux builds identiques lances dans deux repertoires cibles differents produisent des binaires qui different de 10 octets, vraisemblablement dans l'en-tete de debogage PE (identifiant CodeView/PDB). Piste 0.0.2 : neutraliser cet en-tete (option de l'editeur de liens ou post-traitement) et ajouter a la CI un double build avec comparaison des empreintes.

## Points techniques a traiter en 0.0.2

1. Le champ `total_memory_bytes` additionne aujourd'hui toutes les zones UEFI, y compris MMIO et zones reservees. Il ne doit pas etre presente comme de la RAM physique installee.
2. `BOOT_SERVICES_DATA` est actuellement classe `Reclaimable`. Avant toute reclamation effective, Elyon devra avoir bascule sur une pile dont il controle explicitement l'allocation et exclure les plages encore utilisees par le bootstrap.
3. L'IDT utilise le selecteur de code `0x08` en dur. La 0.0.2 devra propager le selecteur reel produit par la GDT.
4. Aucun TSS ni pile IST n'existe encore. La 0.0.2 devra au minimum proteger `#DF` par une pile IST dediee et preparer la gestion robuste des fautes de pile.
5. La CI utilise encore des actions GitHub referencees par tag ou branche, notamment `dtolnay/rust-toolchain@master`. La chaine d'approvisionnement devra etre epinglee sur des SHA complets et documentes.
6. La toolchain cible reste Rust 1.99.1. Le boot externe du correctif a ete prouve avec Rust 1.97.0 ; une validation distincte avec 1.99.1 reste requise.

## Regle de sortie de 0.0.2

Un critere n'est considere satisfait que si le code correspondant est present, le test est execute, la preuve est conservee et le resultat est rattache a un commit et a une toolchain identifies.
