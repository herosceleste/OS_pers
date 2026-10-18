# Limites connues de Elyon Kernel 0.0.2

Cette implémentation est volontairement minimale.

- La pagination initiale mappe les 4 premiers GiB en identité avec des huge pages de 2 MiB. Elle est largement RWX et devra évoluer vers W^X, NX et des permissions par section.
- Le PMM utilise un bitmap statique plafonné à 32 GiB. La prise en charge de machines plus grandes viendra après validation de l'algorithme.
- Les frames allouées par l'auto-test PMM ne sont pas rendues au pool.
- Le loader impose que le noyau réside sous 4 GiB.
- Le framebuffer n'est utilisé que si toute sa plage est accessible dans le mapping initial de 4 GiB.
- L'ACPI RSDP est découvert mais les tables ACPI ne sont pas encore parsées.
- Aucun APIC, IOAPIC, timer, SMP ou scheduler n'est encore actif.
- L'IDT diagnostique traite les exceptions comme fatales. La reprise après exception n'est pas autorisée.
- Secure Boot n'est pas encore activé : les binaires Elyon ne disposent pas encore d'une chaîne de signature de confiance.
- La CI dépend encore des paquets Ubuntu et du canal de distribution officiel Rust ; la supply chain n'est donc pas encore hermétique.
- La reproductibilité bit à bit des artefacts n'est pas encore prouvée.

Toute affirmation de performance ou de sécurité doit rester limitée aux propriétés effectivement mesurées.
