# Current-branch questions

All 22 questions address the retained main-branch source at uv commit `a58e7cd639ef2ce0f4e6717b3e06269c3e1001ad`. Each English/French pair shares its split, fact requirements and evidence. [Machine-readable questions and exact source spans](gold.json)

## git-cache

dev · prose · answerable

- **EN** What identifies a Git dependency in the cache once its reference has been resolved?
- **FR** Une fois la référence résolue, quelle valeur identifie une dépendance Git dans le cache ?

## digest-endian

dev · identifier · answerable

- **EN** In to_hex, what byte order is used to encode the u64?
- **FR** Dans to_hex, dans quel ordre les octets du u64 sont-ils encodés ?

## refresh-one

dev · prose · answerable

- **EN** Which option forces revalidation for just one dependency during synchronization?
- **FR** Quelle option force une nouvelle validation pour une seule dépendance lors de la synchronisation ?

## toolchains

heldout · cross-file · answerable

- **EN** Give the workspace minimum Rust version and the channel pinned for developers, in that order.
- **FR** Quelles sont, dans cet ordre, la version minimale de Rust pour le workspace et la version du canal épinglée pour les développeurs ?

## id-entropy

heldout · prose · answerable

- **EN** How long are the IDs produced by the fast ID crate and how many bits of entropy do they contain?
- **FR** Quelle est la longueur des identifiants du générateur rapide et de combien de bits d’entropie disposent-ils ?

## canonical-fetch

heldout · identifier · answerable

- **EN** For CanonicalUrl, why should fetching use the original URL? Give the two changes that can make the canonical URL unsuitable.
- **FR** Pour CanonicalUrl, pourquoi faut-il utiliser l’URL originale pour le téléchargement ? Citez les deux changements possibles.

## config-precedence

heldout · prose · answerable

- **EN** Two project configuration files sit side by side. Which one takes precedence and what happens to the other file’s uv table?
- **FR** Lorsque les deux fichiers de configuration du projet coexistent dans le même dossier, lequel est prioritaire et que devient la table uv de l’autre ?

## lock-timeout

heldout · prose · answerable

- **EN** For cache-modifying commands, give the default wait for other processes and the environment variable that changes it.
- **FR** Pour les commandes qui modifient le cache, quel est le délai d’attente par défaut des autres processus et quelle variable permet de le modifier ?

## bool-hashing

heldout · cross-file · answerable

- **EN** Which unsigned integer width does CacheKey use for a bool, and which hasher does cache_digest construct before finishing the hash?
- **FR** Quelle largeur d’entier non signé CacheKey utilise-t-il pour un booléen, et quel hacheur cache_digest construit-il avant de terminer le calcul ?

## local-run

heldout · unanswerable · insufficient evidence

- **EN** What exact ID did my private laptop generate on its most recent invocation of the uv fast ID generator?
- **FR** Quel identifiant exact mon ordinateur privé a-t-il généré lors de son dernier appel au générateur rapide de uv ?

## local-config

dev · unanswerable · insufficient evidence

- **EN** Which exact custom index URL is currently set in my private workstation user configuration?
- **FR** Quelle URL d’index personnalisée est actuellement définie dans la configuration utilisateur de mon poste privé ?
