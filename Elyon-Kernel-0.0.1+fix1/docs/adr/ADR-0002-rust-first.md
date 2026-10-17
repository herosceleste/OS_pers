# ADR-0002, Rust no_std comme langage principal

Statut : accepte.

Le noyau privilegie Rust `no_std`. L'assembleur est limite aux operations pour lesquelles le controle architectural direct est necessaire. Les dependances doivent rester faibles, auditees et explicites.
