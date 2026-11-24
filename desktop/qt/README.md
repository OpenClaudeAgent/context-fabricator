# Context Fabricator Qt

Client Qt Quick/C++20 pour Ubuntu 24.04 amd64 et macOS arm64. L’application supervise le serveur Rust et utilise le client généré pour le corpus, les réponses, les modèles et les diagnostics. [Galerie Qt](../../docs/SCREENSHOTS-QT.md) · [Contrat OpenAPI](../../contracts/openapi.yaml) · [Périmètre du port](../../docs/qt-port.md) · [Préparation de la release](../../docs/RELEASE.md)

## Arborescence

```text
src/
  app/                 Composition, navigation, langue et apparence
  platform/            Processus Rust, découverte de session et transport local
  ui/                  Thème et composants QML partagés
  features/
    overview/          État du service, aperçu du corpus et trafic
    answers/           Composer, session de réponse, historique, modèles actifs et citations
    models/            Bibliothèque, transferts, estimation de stockage, moteur et embeddings
    settings/          Préférences et politique de l’historique
    github/            Compte, dépôts, collecte, stockage, Actions et graphe Rust
    diagnostics/       Requêtes, journaux, métriques, usage et inspecteur de traces partagé
    pipeline/          Inspection des réponses, sources de test et exécution des benchmarks
cmake/                 Génération OpenAPI et contrôles qualité
i18n/                  Génération et chaînes propres à Qt
packaging/             Assemblage des distributions
tests/                 Tests C++, Qt Quick et lancement des paquets
```

Chaque fonctionnalité regroupe ses modèles C++ et ses vues QML. L’application assemble ces fonctionnalités. Les modèles utilisent la session et le transport de `platform/` pour appeler le client généré. Un composant propre à une fonctionnalité reste dans son dossier. Le thème et les contrôles réutilisés par plusieurs fonctionnalités vivent dans `ui/`. CMake conserve l’URI QML `ContextFabricator` et les noms des ressources embarquées.

## Build

Installer Qt 6.12 avec Qt Base, Qt Declarative et Qt Tools (Linguist), Python 3, un compilateur C++20 et un JDK. La CI utilise Qt 6.12.0, Java 21, Clang 19 sur Linux et Apple Clang sur macOS. Sous Ubuntu, le SDK Qt en ligne a besoin de son archive ICU. Installer les bibliothèques système GL/EGL et XKB pour le build.

Le [fichier de versions](requirements-quality.txt) fixe CMake 4.2.3, Ninja 1.13.0 et Lizard 1.21.0 pour les deux plateformes. Installer ces outils depuis ce répertoire pour reprendre les versions de la CI.

```sh
python3 -m venv build/tools/qa
build/tools/qa/bin/pip install --only-binary=:all: -r requirements-quality.txt
export PATH="$(cd build/tools/qa/bin && pwd):$PATH"
```

Depuis ce répertoire, indiquer `Qt6_ROOT` si CMake ne trouve pas le SDK.

```sh
cmake --preset dev
cmake --build --preset dev --parallel 4
ctest --preset dev
```

CMake télécharge une fois le JAR OpenAPI Generator 7.27.0 dans `desktop/qt/build/tools` et vérifie son SHA-256. Un JAR déjà disponible peut être fourni avec `-DCTXFAB_OPENAPI_GENERATOR_JAR=/chemin/vers/le.jar`. Toute modification du contrat ou de la recette déclenche une régénération. Le client généré préserve les champs obligatoires qui acceptent `null`, les identifiants numériques des tentatives Actions et le corps du DELETE utilisé pour le retrait confirmé des poids. Les sources générées et les rapports restent sous `desktop/qt/build/`.

CMake extrait les chaînes QML et C++ avec `lupdate` et embarque les catalogues compilés par `lrelease`. Pour les textes communs, renseigner les traductions dans le [catalogue macOS](../macos/App/Localizable.xcstrings). Pour un texte propre à Qt, utiliser le [catalogue Qt](i18n/qt-only.json). Le build échoue si une langue manque ou si les paramètres d’un message divergent.

## Qualité

Le preset `quality` requiert clang-format et clang-tidy 19 sur le PATH, ainsi que Lizard 1.21.0. Le build exécute le formatage en mode vérification, le lint QML avec zéro avertissement, l’analyse C++ avec avertissements bloquants et la limite de complexité cyclomatique de 10 par fonction de production.

```sh
cmake --preset quality
cmake --build --preset quality --parallel 4
ctest --preset quality

cmake --preset sanitizers
cmake --build --preset sanitizers --parallel 4
ctest --preset sanitizers
```

Sur Ubuntu avec Clang 19, installer `libclang-rt-19-dev` pour le preset sanitizers. Pour qualifier `readStatus` sur le vrai service, fournir le chemin absolu du binaire Rust déjà compilé.

```sh
CTXFAB_TEST_BACKEND_EXECUTABLE="$(cd ../.. && pwd)/backend/target/debug/ctxfab-server" ctest --preset quality
```

Le scénario démarre le backend sur un profil temporaire, couvre un conflit de propriétaire, simule un crash et vérifie le renouvellement de session après redémarrage. Il vérifie la fermeture du backend à la fin. La matrice CI compile le serveur avec la version Rust du dépôt et fournit le binaire au test. Les tests réseau avec fixture restent disponibles pour le développement Qt seul.

La suite `qt-preferences` utilise le même binaire Rust. Elle vérifie la validation des réglages, la recherche du japonais pour les réponses, la reprise d’un brouillon après un conflit de révision, la persistance de la politique d’historique et l’appel d’effacement.

La suite `qt-app-language` charge les huit langues de l’interface, vérifie le choix système et restaure la préférence au redémarrage. Qt stocke ce choix dans les préférences locales de l’application. Le réglage de langue des réponses est stocké dans le profil backend. La suite `qt-qml-components` vérifie les changements de libellés et la navigation clavier du sélecteur.

La suite `qt-github` lance aussi la fixture GitHub Python sur un port local. Elle couvre la plage des révisions du client généré, la connexion et la rotation, la découverte paginée, la configuration des quatre flux et les modifications concurrentes de dépôts. Avec l’ordonnanceur Rust actif, elle vérifie la synchronisation d’une branche, le quota, la purge d’un seul dépôt et la reprise après purge. Le scénario du graphe Rust vérifie deux racines indépendantes, les liens épinglés au commit publié et la consultation après déconnexion. Le scénario Actions vérifie le filtre, les tentatives, les liens exacts, les erreurs de collecte et la vérification des droits après rotation du jeton. Lancer `ctest --preset quality -R qt-github` avec `CTXFAB_TEST_BACKEND_EXECUTABLE` pour isoler ces parcours.

La suite `qt-github` vérifie aussi la collecte manuelle de l’historique, la reprise après purge et l’ouverture du span de collecte avec le vrai serveur. `qt-traces` couvre les pages de spans et de journaux, la sélection exacte du span, les traces liées et la reprise après une erreur de chargement. Elle vérifie aussi les statistiques d’usage, la conservation de la dernière lecture réussie et le JSON exporté avec un identifiant 64 bits.

La suite `qt-models` vérifie le catalogue et ses estimations de stockage, les états de transfert en 64 bits et la reprise après une erreur de lecture. Elle utilise le serveur Rust et la fixture Hugging Face des XCTest pour télécharger, mettre en pause, reprendre et abandonner un transfert. Le test de retrait remplace les poids dans un profil temporaire et vérifie que le serveur exige une nouvelle confirmation.

La suite `qt-inference-controls` vérifie les opérations du moteur et de sélection de profil sur le fil. Elle vérifie aussi la persistance des commandes de l’index et la recherche BM25 avec le serveur Rust redémarré.

Le preset `coverage` exécute le build strict, CTest et le scénario complet hors écran avec le backend Rust, puis exige 90 % des lignes du C++ de présentation et de session. Le processus principal, le client généré et le QML sont hors du périmètre LLVM. Les tests de composants QML restent inclus dans CTest. Installer les outils LLVM du compilateur utilisé (`llvm-19` et `libclang-rt-19-dev` sous Ubuntu avec Clang 19, outils Xcode avec Apple Clang).

```sh
cmake --preset coverage
CTXFAB_TEST_BACKEND_EXECUTABLE="$(cd ../.. && pwd)/backend/target/debug/ctxfab-server" \
  cmake --build --preset coverage --parallel 4
```

Les rapports sont dans `desktop/qt/build/coverage/` depuis la racine. Chaque exécution crée ses propres profils. Le contrôle échoue si le backend est absent, si LLVM omet un fichier C++ de production ou si les données de couverture sont incohérentes. Le rapport `coverage-diagnostics.log` garde les entrées à hash nul identifiées par LLVM.

Le test de fenêtre Linux utilise `Xvfb` et `xdotool`. Il lance le vrai client Qt, vérifie que `ctxfab status` découvre l’endpoint via XDG et ferme la fenêtre avec Ctrl+W. Depuis la racine du dépôt, fournir les trois exécutables compilés.

```sh
python3 desktop/qt/tests/linux_window_smoke.py desktop/qt/build/quality/ctxfab_qt \
  /chemin/vers/ctxfab-server /chemin/vers/ctxfab
```

### Rejouer les captures de la fenêtre

Le client et le banc graphique partagent le module QML statique `ContextFabricator` et le même point d’entrée. La cible de test lie Qt Test et pilote les contrôles par leurs noms d’objet. Elle utilise un profil jetable, le backend Rust Debug, un worker d’inférence déterministe et la fixture GitHub HTTP locale. Elle vérifie que l’interface démarre en sombre avec une ancienne préférence claire. Le contrôle échantillonne les pixels des captures pour détecter un fond clair ou une image vide. Le scénario couvre Overview, les réponses locales, leur historique, la trace exacte dans Diagnostics, l’annulation, l’abstention et les huit langues à 400 px. Le scénario Settings vérifie le formulaire centré à 760 px, le filtre de langue au clavier, l’adaptation à 460 px en français et l’ouverture de la bibliothèque de modèles. Le scénario GitHub vérifie l’ouverture du connecteur au clavier, la connexion, les deux modes d’ajout, la pagination et l’identité du dépôt ouvert depuis une liste filtrée.

Depuis la racine du dépôt, configurer le preset `coverage` dans `desktop/qt`, puis lancer ces commandes. Xvfb fournit l’affichage Linux. Sur macOS, lancer le scénario depuis une session graphique.

```sh
cmake --build desktop/qt/build/coverage --target ctxfab_qt_window_tests --parallel 4
python3 -B desktop/qt/tests/run_window_scenarios.py \
  desktop/qt/build/coverage/ctxfab_qt_window_tests /chemin/vers/ctxfab-server \
  desktop/qt/build/window-scenarios
```

Pour rejouer le scénario complet et produire les images de la galerie sans ouvrir de fenêtre, ajoutez `--offscreen`. Pour limiter le rejeu à **Inspect Pipeline**, utilisez `--pipeline-only`. La suite `ctest --test-dir desktop/qt/build/dev -R '^qt-pipeline$'` couvre aussi l’activation des sources, les imports, BM25, Hybrid, l’annulation et la reprise après erreur. Les questions de code et les cas Actions enregistrés viennent du même catalogue que l’application macOS.

Sous Linux, l’option `--wayland` rejoue le même scénario avec Qt connecté à Weston. Installer aussi Weston et choisir un dossier de résultats distinct. Le compositeur utilise Xvfb pour disposer d’un siège clavier et permettre de tester les raccourcis et le retour du focus. [Banc graphique](tests/run_window_scenarios.py)

La matrice collecte 31 PNG en thème sombre et les journaux pour les scénarios hors écran sur macOS et Linux, ainsi que pour le rejeu Wayland. Le test de lancement Linux couvre aussi X11. [Module QML statique](https://doc.qt.io/qt-6.12/qt-add-qml-module.html)

## Lancer le client Qt

Depuis la racine du dépôt, lancer `desktop/qt/build/dev/ctxfab_qt` sur Linux ou `desktop/qt/build/dev/ctxfab_qt.app/Contents/MacOS/ctxfab_qt` sur macOS. Le client cherche `ctxfab-server` auprès de son exécutable. En développement, passer `--backend /chemin/vers/ctxfab-server` et `--data-directory /chemin/vers/un-profil-prive` pour isoler l’essai du profil habituel.

La CLI et Qt utilisent `CTXFAB_DATA_DIRECTORY` quand il est défini. Le chemin par défaut est `~/Library/Application Support/ContextFabricator` sur macOS et `$XDG_DATA_HOME/context-fabricator` sur Linux. Linux utilise `~/.local/share/context-fabricator` si `XDG_DATA_HOME` est absent, vide ou relatif. Le serveur verrouille le profil et termine sa session quand le client ferme son entrée standard. Sur macOS, la dernière fenêtre peut se fermer en gardant le service disponible jusqu’à la sortie de l’app.

`--endpoint-file` permet de consulter une session existante en développement. Il attend un fichier de session régulier, détenu par l’utilisateur et accessible à lui seul dans un dossier privé. Qt affiche la disponibilité, le corpus, l’index d’embedding et le trafic sortant dans Overview.

## Bundle macOS local

Utiliser le SDK Qt 6.12.0 macOS avec Qt Base, Qt Declarative, Qt Tools et QtSvg. Le script déploie Cocoa et les imports QML, copie le serveur Rust et la CLI, puis embarque le llama.cpp épinglé avec la recette partagée par SwiftUI. Il vérifie la signature ad hoc de l’ensemble avant de publier le bundle.

Depuis la racine du dépôt, après le build du preset `dev` avec ce SDK, fournir `Qt6_ROOT` et choisir une destination neuve.

```sh
cargo build --locked --manifest-path backend/Cargo.toml -p ctxfab-server
cargo build --locked --manifest-path cli/Cargo.toml
app="desktop/qt/build/package/Context Fabricator Qt.app"
python3 -B desktop/qt/packaging/stage_macos.py --qt "$Qt6_ROOT" \
  --app desktop/qt/build/dev/ctxfab_qt.app --backend backend/target/debug/ctxfab-server \
  --cli cli/target/debug/ctxfab "$app"
python3 -B desktop/qt/tests/macos_bundle_smoke.py "$app" desktop/qt/build/package/mac-window.log
```

Le test ouvre une fenêtre Cocoa dans une session graphique, crée un profil temporaire et vérifie le statut authentifié depuis la CLI incluse. Il contrôle aussi la disponibilité du moteur embarqué et l’arrêt du serveur avec l’application. La livraison publique requiert encore la signature Developer ID, la notarisation et la qualification sur machine propre.

Ajouter `--loopback-only` au test pour limiter les connexions IP de l’application et de ses processus enfants à la boucle locale.

## Coffre GitHub sous Linux

Le backend utilise le service `org.freedesktop.secrets` de la session D-Bus. Les jetons transitent dans une session chiffrée et les nouveaux identifiants sont enregistrés dans la collection par défaut du coffre. Une session absente ou un coffre verrouillé produit le code API `github_keychain_unavailable`. Si le coffre est verrouillé à la déconnexion, le backend détache immédiatement le compte et conserve une demande de suppression pour le prochain accès au coffre. [Spécification Secret Service](https://specifications.freedesktop.org/secret-service/latest/)

Sur Ubuntu, le test utilise `dbus-daemon`, `gnome-keyring`, `libsecret-tools` et `libglib2.0-bin` dans une session isolée. Depuis la racine du dépôt, fournir le serveur Rust compilé en mode Debug.

```sh
python3 -B quality/github/secret_service_check.py backend/target/debug/ctxfab-server
```

## Runtime CPU Linux

Sur Ubuntu 24.04 x86‑64, installer un compilateur C et C++, Binutils et la toolchain CMake/Ninja ci-dessus. Utiliser l’archive du commit llama.cpp épinglé dont la recette vérifie le SHA-256. Depuis la racine du dépôt, choisir un répertoire runtime neuf.

```sh
work=desktop/qt/build/linux-runtime
mkdir -p "$work"
commit=$(PYTHONPATH=quality/release python3 -B -c 'import runtime_source; print(runtime_source.COMMIT)')
curl --fail --location "https://codeload.github.com/ggml-org/llama.cpp/tar.gz/$commit" \
  --output "$work/source.tar.gz"
python3 -B quality/release/linux_runtime.py "$work/source.tar.gz" "$work/runtime"
pin=$(sha256sum "$work/runtime/manifest.json")
CTXFAB_LINUX_RUNTIME_MANIFEST_SHA256=${pin%% *} cargo build --locked \
  --manifest-path backend/Cargo.toml --target-dir "$work/rust" -p ctxfab-server
python3 -B quality/release/linux_runtime_check.py "$work/rust/debug/ctxfab-server" "$work/runtime" --build 12359
```

En distribution, placer le répertoire `runtime/` auprès du backend construit avec cette empreinte. Le test crée un profil temporaire et exerce l’API avec le backend épinglé. L’option `--weights` ajoute un essai de génération CPU à partir des poids locaux Qwen3 Q4_K_M du catalogue.

## Arborescence Qt Linux

Le preset `linux-release` demande à Qt de déployer les bibliothèques, les plugins X11 et Wayland et les imports QML dans un préfixe distinct. Depuis `desktop/qt`, utiliser le SDK avec les modules Qt de la matrice, dont `qtwayland`, et installer `patchelf` pour le déploiement ELF.

```sh
cmake --preset linux-release
cmake --build --preset linux-release --parallel 4
cmake --install build/linux-release --prefix "$PWD/build/linux-stage" --strip
```

Placer `ctxfab-server`, `ctxfab` et le répertoire `runtime/` auprès de `ctxfab_qt` dans `bin/`. Le test `linux_window_smoke.py` avec `--bundled-runtime-build 12359` ouvre l’application sans option `--backend`, vérifie l’origine des bibliothèques Qt chargées et lit le statut du moteur via la session authentifiée. [Déploiement Qt sous Linux](https://doc.qt.io/qt-6/linux-deployment.html)

## Paquet Ubuntu 24.04 amd64

Sur Ubuntu 24.04 amd64, utiliser `dpkg-dev`, `desktop-file-utils` et les bibliothèques de compilation de la matrice Qt. Préparer le runtime épinglé et déployer Qt avec les commandes ci-dessus. Depuis la racine du dépôt, construire le backend et la CLI en Release pour assembler le paquet.

```sh
work="$PWD/desktop/qt/build/linux-runtime"
pin=$(sha256sum "$work/runtime/manifest.json")
export CARGO_PROFILE_RELEASE_STRIP=symbols
CTXFAB_LINUX_RUNTIME_MANIFEST_SHA256=${pin%% *} cargo build --locked --release \
  --manifest-path backend/Cargo.toml --target-dir "$work/rust" -p ctxfab-server
cargo build --locked --release --manifest-path cli/Cargo.toml --target-dir "$work/cli"
curl --fail --location 'https://raw.githubusercontent.com/unicode-org/icu/release-73-2/icu4c/LICENSE' \
  --output "$work/ICU-73.2.txt"
SOURCE_DATE_EPOCH=$(git show -s --format=%ct HEAD) python3 -B desktop/qt/packaging/stage_linux.py \
  --qt-tree "$PWD/desktop/qt/build/linux-stage" --qt-sdk "$Qt6_ROOT" \
  --backend "$work/rust/release/ctxfab-server" --cli "$work/cli/release/ctxfab" \
  --runtime "$work/runtime" --icu-license "$work/ICU-73.2.txt" --version 0.1.0~local1 "$work/package"
sudo apt install "$work/package/context-fabricator-qt_0.1.0~local1_amd64.deb"
```

APT résout les dépendances système calculées à partir des ELF. Le paquet fournit l’entrée de menu **Context Fabricator**, le lanceur `context-fabricator-qt`, `ctxfab` et le runtime CPU. Le profil utilise `$XDG_DATA_HOME/context-fabricator` ou `~/.local/share/context-fabricator`. `dpkg --verify context-fabricator-qt` vérifie les fichiers installés. Le job Linux teste l’installation dans un conteneur sans SDK et les lancements X11 et Wayland hors ligne. [Workflow Qt](../../.github/workflows/qt.yml)

### Première réponse citée sur Linux natif

Le déclenchement manuel du workflow Qt propose l’entrée `native_inference`. Le runner Linux x86‑64 télécharge et vérifie les poids Qwen3 Q4_K_M du catalogue avant d’isoler le conteneur. Le test lance le paquet sous X11, vérifie les poids et pose une question sur le corpus synthétique via la session Rust ouverte par Qt. Il exige la citation exacte, l’entrée d’historique, les spans de génération et de validation et l’absence de trafic sortant.

Avec le paquet installé, les poids déjà disponibles et seulement la boucle locale active, lancer le même test sous un utilisateur non privilégié.

```sh
python3 -B quality/release/qt_linux_cited_answer_check.py /chemin/vers/Qwen3-4B-Instruct-2507-Q4_K_M.gguf --build 12359
```

Le job conserve le journal `cited-answer.log` avec la durée et les données de la trace synthétique. [Test de qualification](../../quality/release/qt_linux_cited_answer_check.py) · [Workflow Qt](../../.github/workflows/qt.yml)

### Conservation du profil pendant la mise à jour

Le test de profil utilise deux paquets distincts dans un conteneur Ubuntu 24.04 amd64. Préinstaller les dépendances du paquet, Python, Weston, D-Bus, GNOME Keyring, `secret-tool` et `gdbus`. Démarrer ensuite le conteneur avec `--network none` et monter les sources du test en lecture seule. Installer le paquet de référence et exécuter la phase `seed` sous un utilisateur non privilégié, dans un répertoire vide qui lui appartient.

```sh
python3 -B quality/release/qt_linux_upgrade_check.py seed "$profile_root" --build 12359
```

Installer le paquet candidat avec APT depuis un fichier local et lancer `verify` sous le même utilisateur. Répéter la vérification après désinstallation et réinstallation du candidat.

```sh
python3 -B quality/release/qt_linux_upgrade_check.py verify "$profile_root" --build 12359
```

Le test lance le paquet sous Wayland, vérifie la découverte du profil par la CLI et cherche le même document en BM25 après chaque installation. Il vérifie aussi la rétention de l’historique et une clé synthétique dans un coffre privé, persistant dans le répertoire de test. [Test de mise à jour](../../quality/release/qt_linux_upgrade_check.py)
