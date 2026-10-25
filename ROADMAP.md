# Elyon OS — Feuille de route long terme

Ce document fixe la cible du projet et les jalons qui y mènent. Chaque version a ensuite son propre `ROADMAP.md` et son `VALIDATION.md`, qui détaillent ses critères. Un critère ne passe à PASS qu'avec du code, un test automatisé et une preuve attachée à un commit, sous QEMU/OVMF puis sous VMware.

## Cible

Elyon doit exécuter **nativement** :

- les **applications Linux et Debian**, installées comme sur Debian (`apt install`, `dpkg -i paquet.deb`), avec les binaires officiels non modifiés ;
- les **applications Windows**, installées comme sous Windows (`setup.exe`, `msiexec /i paquet.msi`), avec les binaires officiels non modifiés.

Une application est compatible quand elle ne peut pas faire la différence avec son système d'origine.

## Ce que « compatible » recouvre

| Couche | Linux / Debian | Windows |
|---|---|---|
| Interface noyau | appels système Linux (~350) | appels système NT (`Nt*`, ~470) |
| Bibliothèques système | glibc, chargeur dynamique `ld.so` | `ntdll`, `kernel32`, `user32`, `gdi32`, `advapi32`… |
| Installation | `dpkg`, `apt`, paquets `.deb` | `msiexec` (Windows Installer), `setup.exe`, services, COM, registre |

L'installation découle des deux premières couches : `dpkg`, `apt` et `msiexec` sont eux-mêmes des applications. Si l'interface noyau et les bibliothèques sont fidèles, les installeurs officiels fonctionnent sans modification.

## Architecture : un noyau multi-personnalités

Elyon suit l'architecture d'origine de Windows NT (qui exécutait nativement Win32, OS/2 et POSIX), reprise par WSL 1 et par le « linuxulator » de FreeBSD :

```
     Applications Linux / Debian             Applications Windows
     (glibc, dpkg, apt officiels)            (ntdll, kernel32… implémentés par Elyon)
                │                                        │
   ┌────────────┴────────────┐            ┌──────────────┴──────────────┐
   │ Personnalité Linux       │            │ Personnalité NT             │
   │ syscalls, ELF, signaux,  │            │ syscalls NT, PE, objets et  │
   │ fork/exec, futex, /proc  │            │ handles, registre, services │
   └────────────┬────────────┘            └──────────────┬──────────────┘
                └───────────────────┬────────────────────┘
        Cœur Elyon : scheduler SMP, mémoire virtuelle par processus,
        VFS (ext4, NTFS), pilotes de stockage et réseau, graphique
```

- **Personnalité Linux** : Elyon implémente lui-même l'ABI Linux dans le noyau. Le userland Debian tourne tel quel.
- **Personnalité NT** : Elyon implémente lui-même les appels système NT, le chargeur PE, le gestionnaire d'objets, le registre et les services. Les applications Windows ne passent pas par la personnalité Linux.
- **Bibliothèques Win32** : une partie représente des millions de lignes (`user32`, `gdi32`, `comctl32`, `msi`…). ReactOS réutilise pour cela le code de Wine (LGPL, utilisable en liaison dynamique). Cette réutilisation, au-dessus d'une couche noyau NT native, sera évaluée le moment venu ; elle ne fait pas passer les applications Windows par Linux.

## Ampleur

- La compatibilité Linux est un chantier de plusieurs années, réaliste.
- La compatibilité Windows complète est du niveau de ReactOS (plus de 25 ans, toujours en alpha). Installer et exécuter des applications Windows réelles par `.msi` est atteignable ; la compatibilité avec l'ensemble des applications Windows ne l'est pas à court terme.

L'ordre des jalons découle de ces dépendances : d'abord le cœur commun, puis la personnalité Linux, puis la personnalité NT.

## Jalons

### Cœur commun

| Jalon | Objectif | Preuve attendue |
|---|---|---|
| 0.0.3 | Interruptions et SMP | **Qualifié** (ACPI, LAPIC, IOAPIC, AP, timer) |
| 0.0.4 | Scheduler et tâches noyau | **Qualifié** (préemption, FPU, per-CPU, sleep, synchronisation SMP, stress SMP) |
| 0.0.5 | Ring 3 et appels système | En cours : K005-01 à K005-12. Premier processus utilisateur isolé du noyau ; `syscall`/`sysret`, TSS RSP0, segments utilisateur ; une faute en ring 3 tue le processus sans affecter le noyau ; exigences de la section Sécurité |
| 0.0.6 | Mémoire virtuelle par processus | Un CR3 par processus, fautes de page, `mmap`/`munmap`, copie à l'écriture, libération des frames par le PMM |
| 0.0.7 | VFS et premier chargeur | Couche VFS, initramfs, chargeur ELF statique ; un binaire statique lancé depuis l'initramfs |
| 0.0.8 | Stockage | Pilotes virtio-blk et AHCI ; lecture/écriture ext4 sur disque |
| 0.0.9 | Entrées et affichage | Clavier et souris (PS/2 puis USB HID), pilote d'affichage sur le framebuffer UEFI, console graphique, compositeur simple ; un programme utilisateur ouvre une fenêtre déplaçable à la souris. Socle du futur X11/Wayland (Linux) et de `user32`/`gdi32` (Windows) |

### Personnalité Linux et Debian

| Jalon | Objectif | Preuve attendue |
|---|---|---|
| L1 | Premières syscalls Linux | Binaire musl statique « hello » : `write`, `exit_group`, `brk`, `mmap` |
| L2 | Processus Linux | `fork`, `execve`, `wait4`, signaux, `clone` et futex ; BusyBox interactif |
| L3 | Liaison dynamique | `ld.so` et glibc officiels de Debian ; `bash` et coreutils Debian non modifiés |
| L4 | Gestion de paquets | `dpkg -i paquet.deb` sur un paquet officiel Debian |
| L5 | Réseau | Pile TCP/IP, virtio-net, DNS ; `apt update && apt install` depuis un miroir Debian officiel |
| L6 | Userland Debian | Démarrage d'un système Debian minimal (init, services, utilisateurs) sur Elyon |

### Personnalité NT et Windows

| Jalon | Objectif | Preuve attendue |
|---|---|---|
| W1 | Chargeur PE et premières syscalls NT | Application console Windows « hello » (`.exe` PE64) |
| W2 | Objets et fichiers NT | Gestionnaire d'objets et de handles, fichiers, sections, threads NT ; outils console Windows réels |
| W3 | Registre et NTFS | Registre persistant, lecture/écriture NTFS |
| W4 | Installeurs simples | Un `setup.exe` (NSIS ou Inno Setup) officiel installe et lance son application |
| W5 | Windows Installer | `msiexec /i paquet.msi` installe une application réelle : service Windows Installer, transactions, désinstallation |
| W6 | Graphique Win32 | `user32`/`gdi32`, fenêtres et messages ; une application graphique Windows installée par `.msi` |

### Plateforme

Accélération graphique (au-delà du framebuffer de 0.0.9), audio, USB au-delà du HID, utilisateurs, permissions et ACL NT, outillage de mise à jour. Ces chantiers seront planifiés au fur et à mesure des besoins des personnalités.

## Sécurité

La sécurité est la priorité du projet : Elyon exécutera des binaires Linux et Windows qu'il ne contrôle pas, et chaque bug noyau est donc un bug de sécurité. Chaque critère est relu, avant sa PR, par des agents spécialisés (relecture Rust du code `unsafe` et de la concurrence, relecture sécurité, recherche d'erreurs silencieuses, performance), en plus des tests, des injections de faute et de la validation QEMU et VMware.

Exigences obligatoires issues des relectures de 0.0.4, à satisfaire avant de qualifier 0.0.5 et 0.0.6 :

- **`swapgs` à chaque entrée depuis le ring 3** (appels système, IRQ, exceptions), avant toute prise de verrou ou impression : l'index du CPU et la zone per-CPU sont lus via GS. `lfence` après le `swapgs` conditionnel (variante Spectre SWAPGS). La politique de `CR4.FSGSBASE` et de l'écriture des bases FS/GS par les processus est fixée explicitement.
- **Entrée « paranoïaque » pour NMI, #DF et #MC** : le choix de faire `swapgs` ne s'y fie pas au CS sauvegardé (une NMI juste après `syscall` arrive en CPL 0 avec le GS utilisateur). Piles IST dédiées pour NMI et #MC ; `RSP0` du TSS propre à chaque tâche.
- **Une entrée utilisateur ne doit jamais pouvoir arrêter le système** : les fautes et les erreurs atteignables depuis un processus tuent ce processus ; l'arrêt global (`halt_system`, par NMI sur tous les CPU) reste réservé aux invariants du noyau.
- **Mémoire des processus** : uniquement des pages obtenues par `allocate_owned` (effacées, rendables par jeton), jamais d'accès par l'identity map du noyau à une page utilisateur ; les allocations permanentes du noyau restent non libérables.
- **Masques `SFMASK`** (IF, DF, AC effacés à l'entrée) et nettoyage des registres non retournés à la sortie des appels système.

## Principes

- **Natif** : chaque personnalité est implémentée par Elyon ; aucune ne dépend d'une autre.
- **Binaires officiels** : la preuve de compatibilité se fait avec des paquets Debian et des installeurs Windows officiels, non recompilés.
- **Qualification** : même discipline qu'aujourd'hui (TDD, QEMU/OVMF, VMware, fail-closed, preuves dans `VALIDATION.md`).
