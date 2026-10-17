# Validation de Elyon Kernel 0.0.1

## Historique

Le tag historique `v0.0.1` correspond au premier bootstrap livre. Il est conserve pour la tracabilite, mais il ne constitue pas une release validee : une erreur de type dans la GDT empechait la compilation, le controle `rustfmt` de la CI echouait et le lancement OVMF d'Ubuntu 24.04 utilisait `-bios` au lieu du montage `pflash` requis par l'image scindee CODE/VARS.

Le commit de correction du 17/10/2026 applique le patch fourni et verifie par Serge AMAN :

- `GlobalDescriptorTable::<4>::empty()` corrige l'incompatibilite de taille ;
- `cargo fmt` corrige les fichiers non conformes ;
- OVMF est monte en `pflash`, CODE en lecture seule et VARS sur une copie modifiable ;
- deux avertissements du compilateur sont supprimes.

## Validation externe executee

La revue transmise le 17/10/2026 indique qu'un clone propre du bundle, apres application du patch, a ete compile et demarre sous QEMU/OVMF avec Rust 1.97.0. Le boot a atteint `ELYON_KERNEL_READY` apres `ExitBootServices`, detection CPU, lecture de 130 regions memoire, chargement GDT et IDT.

Une instruction invalide `ud2` a egalement ete injectee. L'exception a ete capturee une fois puis le processeur s'est arrete proprement, sans triple faute, ce qui confirme le comportement fail-closed de l'IDT pour ce test.

Cette preuve est une validation externe fournie avec le correctif ; elle n'a pas ete reproduite dans l'environnement qui regenere cette archive.

## Validation encore requise

La cible officielle du depot reste Rust 1.99.1. Cette version a ete publiee le 15/10/2026 (source : annonce officielle, https://blog.rust-lang.org/2026/10/15/Rust-1.99.1/), mais le test externe du correctif a utilise Rust 1.97.0 car 1.99.1 n'etait pas telechargeable dans cet environnement. Un run CI ou local avec exactement Rust 1.99.1 reste donc obligatoire avant de declarer le jalon pleinement reproduit avec la toolchain cible.

Les ecarts fonctionnels K003, K005, K006, K007, K009 et, pour sa partie reproductibilite binaire, K011 sont documentes dans `docs/KNOWN_LIMITATIONS_0.0.1.md` et sont transferes au backlog 0.0.2.

## Correctifs de livraison du 17/10/2026 (`v0.0.1+fix1`)

- `Cargo.lock` est versionne et `scripts/build.sh` compile avec `--locked`.
- Le fichier d'empreintes de la livraison ne contient plus que des noms de fichiers relatifs ; il se verifie avec `sha256sum -c` depuis le dossier de livraison, sur n'importe quelle machine.
- Le tag de reference est `v0.0.1+fix1`. La metadonnee de build `+fix1` n'influe pas sur la precedence SemVer, contrairement au suffixe de pre-version `-corrected`.
- La date de publication de Rust 1.99.1 a ete verifiee sur l'annonce officielle.
