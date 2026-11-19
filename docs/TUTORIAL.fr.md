# Tutoriel vnengine — de zéro au paquet distribuable

*(This tutorial is also available in English: `TUTORIAL.md`.)*

Ce guide vous mène du kit auteur à un `.vnpkg` distribuable — sans
installer Rust. Durée estimée : ~30 minutes.

## 1. Installer le kit auteur

1. Téléchargez `vnengine-kit-<plateforme>.zip` (ou `.tar.gz`) depuis la
   page des releases : `windows-x64`, `windows-arm64` ou `linux-x64`.
2. Décompressez-le où vous voulez. À l'intérieur :

   ```text
   bin/         vnengine, vnengine-pack
   template/    projet de démarrage
   docs/        ce tutoriel + la référence du DSL
   ```

3. Ajoutez `bin/` à votre `PATH`, ou retenez son chemin absolu — chaque
   commande ci-dessous fonctionne aussi avec le chemin complet
   (`kit/bin/vnengine --project .`).

Optionnel mais recommandé : installez l'extension VS Code
(`vnengine.vsix`, même release) pour la coloration syntaxique, les
diagnostics dans l'éditeur et les commandes lancer/valider/empaqueter :

```bash
code --install-extension vnengine.vsix
```

## 2. Partir du modèle

Le répertoire `template/` est un mini visual novel complet. Copiez-le
pour garder le kit intact :

```bash
cp -r template my-novel
cd my-novel
```

Sous PowerShell, utilisez `Copy-Item template my-novel -Recurse`, puis
`Set-Location my-novel`.

Structure du projet :

```text
project.toml       config moteur : titre, résolution, dossiers, personnages
script/main.vn     votre histoire (le DSL .vn)
images/            décors et sprites
audio/             musiques et effets sonores
voice/             répliques doublées
plugins/           plugins WASM optionnels
saves/             créé à l'exécution — ne jamais le distribuer
```

## 3. Lancer le jeu

```bash
vnengine --project .
```

Contrôles : **Espace/Entrée/clic** avancer · **F5** sauvegarder ·
**F9** charger · **F11** plein écran · **A** auto · **S** skip ·
**F8** overlay debug · **F4/Q** quitter.

Lancé depuis un répertoire de projet, `vnengine` **recharge à chaud**
`script/main.vn` (~2 s) : modifiez le script, sauvegardez, le changement
apparaît.

## 4. Écrire votre première scène

Ouvrez `script/main.vn`. L'essentiel :

```vn
define e = Character("Eileen", color = "#ff4040")

label start:
    scene bg school with dissolve
    show eileen happy at center
    e "Hello, world!"
    menu:
        "Répondre bonjour":
            e "Elle sourit."
        "Rester silencieux":
            e "Elle hausse les épaules."
    end
```

Les assets sont référencés **sans extension** et relativement à leur
dossier : `scene bg school` charge `images/bg school.png` (ou
`.jpg`/`.webp`) ; `play music "theme"` charge `audio/theme.ogg`.

Tout ce que le langage permet est dans `docs/DSL_REFERENCE.md` — sprites,
transitions, variables, menus, écrans, canaux audio, succès, données
persistantes, plugins.

## 5. Valider en écrivant

```bash
vnengine-pack --project . --check-only
```

Le script est parsé puis linté : labels inconnus, assets manquants,
personnages non déclarés, transforms/screens/succès inconnus, labels
inaccessibles. Sortie exploitable par les éditeurs et la CI :

```bash
vnengine-pack --project . --check-only --json
```

Avec l'extension VS Code, ces mêmes contrôles apparaissent en souligné
pendant la frappe — propulsés par le parseur du moteur compilé en WASM,
aucun binaire requis.

## 6. Empaqueter le jeu

```bash
vnengine-pack --project . --output build/my-novel.vnpkg
```

Le `.vnpkg` est une archive déterministe : manifeste, script compilé,
assets hashés, licences. Il se distribue aux joueurs en un seul fichier.

## 7. Distribuer

Les joueurs lancent le paquet avec le binaire moteur de leur plateforme :

```bash
vnengine --package my-novel.vnpkg
```

Layout de distribution typique :

```text
my-novel-windows-x64/
├── vnengine.exe          (depuis le bin/ du kit)
└── my-novel.vnpkg
```

## Dépannage

| Problème | Solution |
|---|---|
| `vnengine` introuvable | Ajoutez `bin/` du kit au `PATH`, ou utilisez le chemin complet. |
| `warning: missing image asset` | Le nom après `scene`/`show` doit correspondre à un fichier sous `images/` (sans extension). |
| Erreur de parse ligne N | L'indentation est significative — utilisez des espaces, restez cohérent. |
| Fichier absent du paquet | Vérifiez les warnings de `--check-only` ; les assets vivent sous `images/`, `audio/`, `voice/`, `translations/`, `plugins/`. |
