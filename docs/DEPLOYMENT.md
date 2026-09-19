# Déploiement multi-cible

## Objectif

Produire un paquet de VN identique pour Windows, macOS, Linux et WebAssembly,
puis fournir un runtime compilé pour chaque plateforme. Le script source reste
un format de développement ; la distribution utilise un paquet `.vnpkg`.

```text
.vn -> parse/validation -> .vnb -> .vnpkg -> runtime natif ou WASM
```

Le premier `.vnb` contient l'AST Serde validé. Il s'agit d'un format
intermédiaire versionné, volontairement simple. Il pourra être remplacé par un
bytecode compact sans changer le manifeste ni le packaging.

## Architecture cible

- `vnengine-core` : modèle et contrats partagés.
- `vnengine-script` : langage, AST, VM et formats de script compilé.
- `vnengine-runtime` : état du jeu et orchestration sans accès direct à l'OS.
- `vnengine-platform` : assets, sauvegardes, audio et événements par cible.
- `vnengine-render` : rendu `wgpu` commun.
- `vnengine-app` : boucle desktop et intégration `winit`.
- `vnengine-pack` : validation et création des `.vnpkg`.

## Format `.vnpkg`

```text
game.vnpkg
├── manifest.json
├── script/main.vnb
└── assets/...
```

Le manifeste contient au minimum la version du format, la version du moteur,
le titre, la résolution et le point d'entrée. Les prochaines versions devront
ajouter les hashes des assets et éventuellement une signature.

## Phases

1. **Packaging desktop** : créer et valider `.vnpkg` sans casser le mode projet.
2. **Abstraction plateforme** : extraire `AssetStore`, `SaveStore` et
   `AudioBackend` ; isoler `std::fs`, Kira et Gilrs derrière des features.
3. **Runtime paqueté** : démarrer une VN depuis `.vnpkg` sur Windows, macOS et
   Linux.
4. **WebAssembly** : canvas `wgpu`, chargement asynchrone des assets, WebAudio
   et IndexedDB/localStorage.
5. **Distribution** : archives natives, page Web, checksums et CI multi-cible.

## Commandes de la première étape

```text
cargo run -p vnengine-pack -- --project demo --output build/demo.vnpkg
cargo run -p vnengine-pack -- --project demo --output build/demo.vnpkg --check-only
```

Le paquet ne doit pas contenir de sauvegardes ni de fichiers cachés. Les
extensions d'assets sont contrôlées et les chemins sont conservés relatifs au
projet.

