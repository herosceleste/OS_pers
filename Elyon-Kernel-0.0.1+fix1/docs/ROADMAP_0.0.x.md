# Roadmap Elyon Kernel 0.0.x

## 0.0.1

Boot UEFI x86-64, sortie des Boot Services, carte memoire, console serie, GDT, IDT fail-closed, identification CPU, et marqueur de boot automatise.

## 0.0.2

Separation `elyon-loader.efi` et `elyon-kernel.elf`, chargeur ELF64, allocation physique par bitmap, ownership de la pagination, exceptions diagnostiques, framebuffer GOP et ACPI RSDP.

## 0.0.3

APIC local, IOAPIC, timers, TSC calibre, SMP bootstrap des AP, piles par CPU, donnees per-CPU.

## 0.0.4

Ordonnanceur preemptif initial, threads noyau, changement de contexte, primitives de synchronisation et temporisation.

## 0.0.5

Ring 3, processus, espace d'adressage utilisateur, syscalls Elyon ABI v0, capabilities minimales et isolation des services.
