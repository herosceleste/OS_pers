# Politique de securite Elyon

Elyon est concu selon un principe secure-by-design. Pour 0.0.1, toute vulnerabilite affectant le bootstrap UEFI, la transition `ExitBootServices`, la gestion de la memoire, les tables GDT/IDT ou l'execution en ring 0 est consideree critique jusqu'a analyse contraire.

Ne publiez jamais de secret, cle privee, certificat de signature, jeton d'acces ou donnees personnelles dans une issue publique.

Les versions futures integreront un PSIRT, un processus CVE, des SBOM signes, une provenance de build, une politique de divulgation coordonnee et une chaine de signature des releases.
