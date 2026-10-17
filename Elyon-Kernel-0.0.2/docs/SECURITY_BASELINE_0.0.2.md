# Baseline sécurité Elyon 0.0.2

1. Le loader et le kernel sont des binaires distincts.
2. Le loader refuse un ELF invalide ou non x86-64.
3. Les allocations critiques de bootstrap sont contraintes sous 4 GiB pour rester accessibles après l'installation de la pagination initiale Elyon.
4. La carte mémoire finale est récupérée à la frontière `ExitBootServices`, puis triée et validée.
5. Les zones non `CONVENTIONAL` ne deviennent pas automatiquement libres dans le PMM.
6. Le frame 0, le kernel, sa pile et le `BootInfo` sont réservés explicitement.
7. Le noyau charge sa propre GDT, son propre TSS, son IST de double fault et sa propre IDT avant de déclarer le système prêt.
8. Les exceptions CPU sont fail-closed et produisent un diagnostic série.
9. Elyon charge son propre CR3 avant le marqueur de disponibilité.
10. Aucun réseau, téléchargement, télémétrie ou service externe n'existe dans le noyau 0.0.2.
11. Les dépendances Rust sont verrouillées dans `Cargo.lock`.
12. Le build ne doit pas être qualifié de reproductible bit à bit tant que cette propriété n'a pas été mesurée.

Cette baseline reste un jalon de bootstrap, pas une certification de sécurité ni un état prêt pour la production.
