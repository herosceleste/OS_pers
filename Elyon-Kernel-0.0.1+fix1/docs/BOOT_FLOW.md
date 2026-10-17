# Elyon 0.0.1, flux de demarrage

```text
UEFI firmware
    |
    v
EFI/BOOT/BOOTX64.EFI
    |
    +-- initialise COM1
    +-- prepare la carte memoire
    +-- ExitBootServices
    |
    v
Elyon Kernel
    |
    +-- CLI
    +-- identification CPU
    +-- GDT Elyon
    +-- IDT Elyon fail-closed
    +-- lecture CR0/CR3/CR4
    |
    v
ELYON_KERNEL_READY
    |
    v
HLT
```

La frontiere de souverainete technique de ce jalon est `ExitBootServices`. A partir de ce point, Elyon ne depend plus des services de boot UEFI pour poursuivre son execution.
