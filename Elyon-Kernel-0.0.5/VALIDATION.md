# Elyon Kernel 0.0.5 — Validation

**EN COURS — aucun critère K005 n'est encore qualifié.**

## Baseline héritée

La 0.0.5 est dérivée de la Release `v0.0.4`, dont les critères K003-01 à K003-12 et K004-01 à K004-12 sont qualifiés sous QEMU/OVMF et VMware Workstation en 4 vCPU (voir `Elyon-Kernel-0.0.4/QUALIFICATION_0.0.4.md`). La CI 0.0.5 conserve ces invariants comme régressions pendant tout le chantier ring 3 : chaque test K003 et K004 de la 0.0.4 tourne à chaque commit, et le gate de release applique le gate final K003-01..12 et K004-01..11 au boot QEMU/OVMF à 4 vCPU.

## Critères

| Critère | Description | Statut |
|---|---|---|
| K005-01 | Segments utilisateur, `RSP0` par tâche | À faire |
| K005-02 | `swapgs` à chaque entrée depuis le ring 3 | À faire |
| K005-03 | Entrée « paranoïaque » NMI/#DF/#MC | À faire |
| K005-04 | SMEP, SMAP et copies validées | À faire |
| K005-05 | Pages utilisateur isolées et rendables | À faire |
| K005-06 | `syscall`/`sysret` sans fuite de registres | À faire |
| K005-07 | Premier processus utilisateur | À faire |
| K005-08 | Une faute en ring 3 tue le processus, jamais le système | À faire |
| K005-09 | Timer LAPIC et préemption sur chaque AP | À faire |
| K005-10 | Contre-pression sur file pleine | À faire |
| K005-11 | Stress SMP ring 3 / ring 0 | À faire |
| K005-12 | Qualification finale 0.0.5 | À faire |

## Politique de preuve

Chaque PASS doit être lié à un commit qualifié et à un test exécuté. Les marqueurs série seuls ne suffisent pas si le test ne valide pas également l'invariant concerné. Une entrée depuis le ring 3 étant hors du contrôle du noyau, chaque critère de frontière est aussi prouvé par des injections de faute : un processus qui tente d'enfreindre l'invariant doit être tué, et le système doit continuer.

La première étape fonctionnelle sera **K005-01**.
