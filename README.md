# Cruft
Éditeur de jeu Rust avec Bevy 0.20. Les lots B1/B2/B3 fournissent le workspace,
les commandes annulables, l'inspection reflect, la persistance RON, le mode Play,
le navigateur d'assets et la console tracing.

## Démarrer

Rust 1.97.1 ou plus récent, un environnement de compilation natif et un GPU
compatible avec Bevy sont nécessaires.

```sh
cargo run -p cruft_editor
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

La fenêtre « Cruft » affiche un sol, deux cubes, une lumière et une caméra.
Ctrl Z annule ; Ctrl Shift Z ou Ctrl Y rétablit. La scène initiale est créée
par des commandes, sans remplir l'historique utilisateur.

## Workspace

| Crate | Rôle |
| --- | --- |
| `cruft_core` | Commandes, historique, sélection, états, vues reflect, console ; sans UI |
| `cruft_scene` | Persistance RON, snapshots Play, catalogue et import d'assets |
| `cruft_ui` | Interface de l'éditeur (A1/A2) |
| `cruft_mcp` | Point d'intégration pour les outils MCP (B4) |
| `cruft_editor` | Binaire assemblant les plugins et la scène de test |

Les consignes du projet sont dans [AGENTS.md](AGENTS.md).

## API des commandes

Ajouter `CruftCorePlugin` avant d'utiliser les commandes. Toutes les mutations
de scène depuis l'UI ou MCP doivent passer par `History::execute` :

```rust,ignore
use bevy::prelude::*;
use cruft_core::{History, SetComponentField, SpawnEntity};

History::execute(world, SpawnEntity::new("Cube")
    .with_component(Transform::default()))?;
History::execute(world,
    SetComponentField::typed::<Transform>(entity, "translation.x", "2.5"))?;
world.resource_mut::<History>().finish_group(); // fin du drag
History::undo(world)?;
History::redo(world)?;
```

`SetComponentField::new` accepte l'entité, le chemin complet du type de
composant, le chemin reflect du champ et une valeur **RON typée** : `2.5`,
`"nom"`, `[1, 2, 3]`, etc. Un chemin vide remplace le composant complet.
Les commandes consécutives visant la même entité, le même composant et le
même champ fusionnent en conservant la toute première valeur pour l'undo.
L'UI appelle `finish_group()` à la fin du geste ou au changement de focus ;
MCP l'appelle entre deux actions indépendantes. Undo/redo ferme aussi le groupe.
Une modification réussie vide la pile redo ; une erreur conserve les piles.
L'historique retient au maximum 200 entrées.

`DespawnEntity` capture le sous-arbre avant toute suppression, via reflect,
et restaure ses composants, ses liens parent/enfants, l'ordre des enfants et
la sélection. Enregistrer les composants éditables avec `register_type::<T>()`
et `#[derive(Component, Reflect)] #[reflect(Component)]`. Les champs ignorés
par reflect nécessitent une implémentation de clone reflect adaptée. Les
composants explicitement marqués `clone_behavior = Ignore` sont transitoires ;
les autres composants non enregistrés font échouer la suppression sans perte.

Bevy attribue un nouvel identifiant à une entité restaurée. Les commandes
résolvent automatiquement les anciens identifiants. L'UI, MCP et les
références conservées hors du sous-arbre doivent utiliser
`resolve_entity(world, original_id)` avant une requête. Les références
`#[entities]` présentes dans les composants restaurés sont remappées.
`Reparent` conserve le transform **local**, refuse les cycles et accepte
`None` pour détacher une entité ; utiliser cette commande pour la hiérarchie,
plutôt que modifier `ChildOf` ou `Children` par reflect.

`Selection` expose une entité principale et une liste ordonnée sans doublons.
`EditorEvent::Toast(String)` est un événement Bevy immédiat :
`CruftUiPlugin` s'abonne avec un observer `On<EditorEvent>`. Les commandes signalent
leurs erreurs par cet événement et `error()` ; l'historique renvoie aussi
un `Result` et émet les confirmations apply/undo/redo.

## Inspecteur générique (B2)

`component_views(&world, entity)` renvoie des `ComponentView { name, fields }`
triées par nom complet du type. Seuls les composants reflect présents dans
`AppTypeRegistry` sont inclus. Chaque `FieldView` fournit `path`, `kind`,
`value`, `editable` et une éventuelle `error` de sérialisation.

`value` est une chaîne **RON**, pas une chaîne d'affichage brute : les guillemets
des textes sont conservés, les grands entiers ne passent pas par un flottant.
`FieldKind` distingue booléens, entiers signés/non signés, flottants, textes,
caractères, références d'entités et conteneurs. Les enums exposent leurs noms
de variantes. Les structures imbriquées, tuples, listes et tableaux exposent
des chemins tels que `translation.x`, `points[0].1` ou `values[2]`. Les maps
et sets s'éditent par leur valeur RON complète, leurs clés n'étant pas adressables
par un chemin reflect.

L'inspection est en lecture seule : pour changer une valeur, transmettre
`component.name`, `field.path` et la nouvelle valeur RON à
`History::execute(world, SetComponentField::new(...))`. Respecter `editable` :
la hiérarchie passe par `Reparent`, les types opaques non sérialisables restent
visibles, et certains conteneurs (notamment les tuples/tableaux sans type data
`ReflectFromReflect`) exposent seulement leurs sous-champs comme modifiables.

## Sauvegarder et charger une scène (B2)

```rust,ignore
use cruft_scene::{save_scene, load_scene, save_scene_file, load_scene_file};

let ron = save_scene(world, scene_entities.iter().copied())?;
save_scene_file(world, scene_entities.iter().copied(), "assets/level.scn.ron")?;
load_scene(world, &ron)?;
// Ou : load_scene_file(world, "assets/level.scn.ron")?;
```

Le format natif est `DynamicWorld` / `WorldDeserializer` dans
`bevy::world_serialization` (nom utilisé en Bevy 0.20). Les composants enregistrés
avec `#[reflect(Component)]` sont sérialisés ; les composants non enregistrés
ou explicitement transitoires sont omis. Les ressources ne sont jamais chargées.
Le registre de destination doit connaître tous les types présents dans le fichier.

La liste d'entités est explicite pour ne pas sauvegarder les fenêtres, panneaux
et autres entités internes de l'éditeur. Elle doit inclure les parents, enfants
et toutes les cibles de champs `#[entities]` ; une référence hors de cet ensemble
est une erreur. Les identifiants sont remappés au chargement : l'identité vérifiée
par les tests porte sur les composants et les liens, pas sur les ID numériques.

`load_scene` **ajoute** les entités à la scène courante en une seule commande
`LoadScene`, annulable par `History::undo`. Il préserve les entités existantes.
Un fichier invalide, un type inconnu, une référence manquante ou une hiérarchie
cyclique sont refusés avant la création des entités. Pour accéder au remappage
lors d'un chargement manuel via le trait, utiliser `LoadScene::entity_map()`.
Les anciennes commandes restent valides après redo via `resolve_entity`.

Les assets référencés par chemin utilisent l'`AssetServer` de destination ;
les handles UUID sont conservés. Les assets générés uniquement en mémoire
(comme les meshes/matériaux de la scène de démonstration) ne sont pas embarqués
dans le RON : leur sauvegarde échoue explicitement pour éviter de les remplacer
par des handles par défaut. La persistance de leur contenu relève du pipeline
d'assets. Les champs `#[reflect(ignore)]` restent hors du contrat de persistance.

## Play / Pause / Stop (B3)

Play (ou Ctrl P) capture la scène avant d'activer `EditorState::Playing`.
Pause passe à `Paused` et suspend `Time<Virtual>` ; un second clic reprend.
Stop (ou Ctrl P) revient à `Editing`, restaure les composants, la hiérarchie,
la sélection et l'historique antérieurs, puis retire les entités créées pendant
la simulation. La restauration intervient après les hooks `OnExit` du gameplay.
Les deux cubes de démonstration tournent uniquement pendant Play.

```rust,ignore
use cruft_core::{EditorState, Gameplay};
use cruft_scene::{PlayAction, request_play_action};

app.add_systems(Update, move_player.in_set(Gameplay));
app.add_systems(FixedUpdate, simulate_physics.in_set(Gameplay));
// Équivalent pour un système isolé : .run_if(in_state(EditorState::Playing))
request_play_action(world, PlayAction::Play);
request_play_action(world, PlayAction::Pause);
request_play_action(world, PlayAction::Resume);
request_play_action(world, PlayAction::Stop);
```

Les demandes sont traitées une par frame, avant les transitions d'état Bevy.
`SceneEntity` identifie les entités de scène ; `SpawnEntity` et `LoadScene`
l'ajoutent automatiquement. Leurs descendants sont inclus. Ajouter ce marqueur
aux entités créées autrement avant Play, et `EditorOnly` aux helpers de l'éditeur.
Les commandes d'édition et undo/redo sont bloqués tant que Play est actif.

`SceneSnapshot` conserve les valeurs reflect **en mémoire** : il garde les handles
forts des assets procéduraux, sans les sérialiser en RON. Seuls les composants
enregistrés sont restaurés ; les ressources de gameplay et le contenu mutable des
assets ne font pas partie du snapshot. Enregistrer les composants de gameplay
avec `#[reflect(Component)]`. Les entités survivantes gardent leur ID ; les entités
recréées publient un remappage via `resolve_entity`, utilisé par l'historique.
Les sous-hiérarchies des modèles déjà instanciés sont restaurées avec leurs
modifications ; la copie générée ensuite par le loader est éliminée pour éviter
les doublons. Changer explicitement la source du modèle réinstancie cette source.
Les snapshots d'undo excluent aussi les caches déclarés dans `SnapshotExclusions` ;
le binaire y inscrit `PreviousGlobalTransform`, régénéré par le rendu Bevy.
Les composants applicatifs inconnus continuent de bloquer la suppression.

## Assets et console (B3)

`AssetCatalog` scanne récursivement `assets/` au démarrage et via **Actualiser**.
Il reconnaît GLB, PNG, EXR, RON, SCENE et `.scn.ron`, ignore les liens symboliques
et fournit des chemins relatifs triés ainsi que les erreurs de lecture.
Le binaire utilise le dossier `assets/` du workspace, indépendamment du dossier
depuis lequel il est lancé.

Glisser une tuile GLB dans le viewport importe `#Scene0` à l'origine via
`History::execute(SpawnEntity)`, avec Transform et `SceneRoot`. Bevy 0.20 a renommé
ce composant en `WorldAssetRoot` : `cruft_scene::SceneRoot` est son alias explicite.
L'import constitue une seule entrée undo/redo ; le chargement et ses erreurs sont
asynchrones. Les autres formats sont catalogués, sans action de dépôt pour l'instant.
Le triangle de `cruft_scene/tests/fixtures/triangle.glb` sert au test du vrai loader.

`cruft_core::console::ConsoleLog` est un ring buffer partagé entre threads, de
2 000 entrées par défaut (16 Kio maximum par message). Chaque entrée expose
timestamp, niveau, cible et message, avec les champs structurés de l'événement.
`entries()`, `dropped()` et `clear()` sont accessibles à l'UI. Le binaire installe
la capture avec `LogPlugin { custom_layer: capture_layer, ..default() }` ; la console
respecte le filtre Bevy/`RUST_LOG` et conserve la sortie terminal existante.
