# Manifeste Elyon Kernel 0.0.1

Composants source livres :

- `elyon-boot`, image UEFI x86-64 et transition firmware vers noyau.
- `elyon-kernel`, noyau Rust `no_std`.
- console serie COM1.
- representation interne de la carte memoire.
- identification CPUID.
- GDT ring 0 Elyon.
- IDT Elyon avec garde des exceptions CPU.
- scripts build, execution QEMU et smoke test.
- CI GitHub Actions.
- documentation de boot, securite, roadmap et ADR.

Critere de succes 0.0.1 : la sortie serie contient `ELYON_KERNEL_READY` apres `ExitBootServices`, chargement de la GDT et chargement de l'IDT.
