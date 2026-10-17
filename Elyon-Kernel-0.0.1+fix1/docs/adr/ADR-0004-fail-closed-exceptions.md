# ADR-0004, exceptions fail-closed

Statut : accepte.

Tant qu'un gestionnaire d'exception diagnostique complet n'existe pas, une exception CPU ne doit pas reprendre une execution incertaine. Elyon 0.0.1 desactive les interruptions et bloque le processeur dans un handler minimal.
