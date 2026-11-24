# Client Qt de Context Fabricator

Le client Qt Quick/C++20 cible Ubuntu 24.04 amd64 avec X11 et Wayland, et macOS arm64. SwiftUI reste le client natif sur macOS. Les deux interfaces utilisent le même backend Rust et la même CLI. [Galerie Qt](SCREENSHOTS-QT.md) · [Architecture](../ARCHITECTURE.md) · [Préparation de la release](RELEASE.md)

## Responsabilités

| Couche | Rôle |
| --- | --- |
| QML | Mise en page adaptative, navigation, focus et accessibilité. Le thème sombre, les contrôles et les dialogues utilisent Qt Quick Controls Basic. |
| C++ | États de présentation, pagination, récupération et annulation des requêtes. CMake génère le client à partir de l’OpenAPI canonique. |
| Session locale | Le superviseur démarre Rust et découvre son endpoint authentifié. En développement, `--endpoint-file` permet de rejoindre une session existante. |
| Rust | Collecte GitHub, stockage et recherche, modèles, inférence, vérification des citations, télémétrie et accès au coffre système. |

Les fonctionnalités regroupent les vues et les modèles C++ sous `src/features/`. La composition, la session et les composants communs se trouvent dans `src/app/`, `src/platform/` et `src/ui/`. Le banc graphique partage le module QML et le point d’entrée de production. [Arborescence](../desktop/qt/README.md#arborescence)

## Parcours disponibles

| Espace | Périmètre |
| --- | --- |
| Overview et Settings | Corpus, état du service et de l’index, trafic sortant, langues et limites de stockage. Les préférences d’interface et de réponse sont indépendantes. |
| GitHub | Connexion, choix du dépôt et de la branche, fichiers, historique, GitHub Actions et graphe Rust. Les traces de collecte donnent accès aux demandes et aux erreurs. |
| Answers | Question locale, citations, historique de session et enregistré, annulation et trace. |
| Inspect Pipeline | Sources, recherche, embeddings, classement, entrée et sortie du modèle, vérification. Le catalogue est partagé avec macOS. Qt permet l’import de fichiers et de suites JSON, et SwiftUI propose aussi un éditeur pour coller du texte. |
| Modèles | Catalogue Qwen, recherche compatible sur Hugging Face, estimation de capacité, téléchargement, pause, reprise et index sémantique. |
| Diagnostics | Requêtes, logs filtrés, métriques, traces, stockage et usage des réponses. |

## Build et qualification

Les presets CMake couvrent le développement, la qualité, la couverture et l’assemblage Linux. Le programme de validation graphique produit des captures depuis un profil jetable. Son mode `--offscreen` rejoue le parcours complet sans ouvrir de fenêtre sur le bureau. Les modes natifs du banc couvrent Cocoa, X11 et Wayland. [Guide Qt](../desktop/qt/README.md#rejouer-les-captures-de-la-fenêtre) · [Matrice CI](../.github/workflows/qt.yml)

Le paquet Ubuntu inclut le client, le backend, la CLI et llama.cpp CPU. Le test du paquet vérifie l’installation, le démarrage X11 et Wayland et la conservation du profil avec le coffre Secret Service. Le scénario optionnel avec les poids Qwen du catalogue vérifie une première réponse citée. [Paquet Ubuntu](../desktop/qt/README.md#paquet-ubuntu-2404-amd64)

Sur macOS, le guide détaille l’assemblage d’un bundle local signé ad hoc et le test Cocoa de son backend et de son runtime. La qualification de la distribution publique suit les critères de la release. [Bundle local](../desktop/qt/README.md#bundle-macos-local) · [Release](RELEASE.md)
