# Revue Cruft — 2026-11-16

Relecteur : Claude. Le dépôt n'a qu'un commit (`c07d86c Initial commit`) : tout le code est non commité, donc impossible d'attribuer le travail par commit (attribution déduite des lots).

> **Mise à jour du même jour.** Après la revue, Claude a repris la main et appliqué les corrections 🔴 et la plupart des 🟠 (§7). Les tableaux ci-dessous montrent l'état **avant → après**.

## 1. Contrôles automatiques

| Commande | Revue initiale | Après corrections |
|---|---|---|
| `cargo build --workspace` | ✅ | ✅ |
| `cargo clippy --workspace --all-targets -- -D warnings` | ✅ (2 avertissements : `rmcp` et `axum` inutilisés) | ✅ sans avertissement |
| `cargo test --workspace` | ✅ 42 tests | ✅ **53 tests** (core 22, scene 14, ui 10, mcp 4, editor 3) |
| `cargo tree -d` | ✅ bevy 0.20.1, egui 0.36.2, aucun doublon | idem (ajouts : `base64`, `image` déjà présent via Bevy) |
| `cargo run -p cruft_editor` | ✅ sans panic | ✅ sans panic, serveur MCP sur `127.0.0.1:7457` |

## 2. État par lot

| Lot | Avant | Après | Justification |
|---|---|---|---|
| **B1** Commandes / undo | ✅ | ✅ | + `CompositeCommand` (N commandes = 1 entrée, rollback si échec) et `InsertComponent` (undo = retrait). |
| **A1** Thème + layout | ✅ | ✅ | Polices encore Inter / JetBrains Mono (décision en attente, §6). |
| **B2** Inspecteur + RON | ✅ | ✅ | Sauvegarde réelle débloquée : composants dérivés (`DerivedComponents`), erreurs qui nomment le composant fautif. |
| **A2** UI ↔ cœur | ✅ | ✅ | Outliner basé sur `SceneEntity` (n'affiche plus les entités moteur), renommage / visibilité via commandes. |
| **B3** Play mode + assets | ✅ | ✅ | Inchangé (déjà solide). |
| **A3** | ❓ | ❓ | Toujours non défini dans `AGENTS.md`. |
| **B4** Serveur MCP | ❌ | ✅ | rmcp 3.10, thread tokio, **stdio** (`--mcp-stdio`) + **Streamable HTTP** `127.0.0.1:7457/mcp`, 21 outils, `docs/mcp.md`. Testé de bout en bout (§4). |
| **A4** Panneau Agent + palette | 🟠 simulé | ✅ | Journal, fantômes, carte et indicateur alimentés par `cruft_core::Proposals` et `AgentActivity` (vrais outils, vraies durées, vrai nom du client). Plus aucune donnée factice. |

## 3. Architecture

### 3.1 Mutations hors `EditorCommand`

| Fichier | Avant | Après |
|---|---|---|
| `cruft_ui` renommage (`rename_entity_command`) | Insère `Name` directement | ✅ `set_or_insert` → `SetComponentField` ou `InsertComponent` |
| `cruft_ui` visibilité (outliner + inspecteur) | Insère `Visibility` directement | ✅ idem |
| `cruft_ui` `attach_scene_camera_to_viewport` | Écrasait `Camera` et ajoutait `RenderTarget` sur la caméra **de la scène** | ✅ Supprimé : une caméra éditeur `EditorViewportCamera` (`EditorOnly`) rend le viewport ; en play mode elle suit la caméra de jeu, puis reprend sa pose à Stop |
| `cruft_editor` `materialize_primitives` | — | Système qui dérive `Mesh3d` et le matériau du composant `Primitive`. Ce n'est pas une édition de scène : c'est une donnée dérivée, comme `GlobalTransform`, non sauvegardée |
| `cruft_editor` scène de démarrage | `apply()` direct, voulu | inchangé |

### 3.2 Undo / redo et multi-sélection (point de contrôle 1)
- ✅ `CompositeCommand` générique dans `cruft_core`.
- ✅ Suppr supprime **toute la sélection** en une entrée ; les descendants d'entités déjà sélectionnées sont ignorés (`delete_selection`, testé).
- 🟠 Restent sur la seule sélection principale : gizmos et glisser-déposer dans l'outliner.

### 3.3 Pont MCP (point de contrôle 2)
- ✅ Canal crossbeam borné (128), `try_send` côté tokio, système Bevy exclusif `process_requests` (`try_iter`, ≤ 32 requêtes par frame), réponse `oneshot`, timeout 10 s, requêtes abandonnées ignorées. Tokio ne touche jamais au World ; aucun Mutex partagé avec le World ; aucun appel bloquant côté Bevy.
- La capture d'écran répond de façon différée via un observer `ScreenshotCaptured` (Mutex local à l'observer, sans lien avec le World).

### 3.4 Propositions
- ✅ `cruft_core::proposals` : `propose` valide et stocke **sans appliquer** ; `apply_proposal` = une `CompositeCommand` dans `History` ; `reject_proposal` ne touche pas la scène ; `AgentMode::{Propose (défaut), Direct}`.
- ✅ Changements gérés : `spawn` (avec parent, composants JSON partiels, primitives), `set_component`, `set_field`, `despawn`, `reparent`.

### 3.5 Play mode
- ✅ Inchangé. L'écriture MCP pendant Play crée une proposition, mais l'application est refusée tant que Play tourne (`ensure_editing`).

### 3.6 Outils MCP : demandés vs exposés
Tous ceux du lot B4 sont exposés. En plus : `set_field`, `reparent`, `list_proposals`, `redo`, et `screenshot_viewport(full_window)` (capture toute la fenêtre de l'éditeur, fantômes compris).

## 4. Contrôles fonctionnels (éditeur réel)

| Test | Résultat |
|---|---|
| HTTP : `initialize` → `tools/list` (21) → `propose_changes` (2 primitives + 1 `set_field`) → `apply_proposal` → `undo` → `redo` → play / stop → `read_logs` → `reject_proposal` | ✅ |
| stdio : `cruft_editor --mcp-stdio`, `initialize` → `tools/list` → `spawn_entity` → `apply_proposal` → `undo` (client « claude-code ») | ✅ stdout = JSON-RPC uniquement |
| `save_scene` puis `load_scene` dans l'éditeur réel | ✅ (7 entités sauvées, 6 meshes recréés au chargement) |
| Capture du viewport et de la fenêtre | ✅ fantômes jaunes viewport + outliner, carte « Proposition de l'agent · 4 changements », indicateur vert « claude-code » |

Bugs trouvés par ces tests et corrigés :
- **Impossible de sauvegarder la scène de démarrage** : ses meshes étaient des assets éphémères. → Composant `Primitive` sérialisable + `DerivedComponents`.
- **Composants de rendu de la caméra non sérialisables** (`ColorGrading`, `CameraRenderGraph`…) → ignorés avec avertissement pour les entités caméra (`DerivedComponents::lenient::<Camera3d>()`).

## 5. UI vs maquette

| Point | |
|---|---|
| Couleurs Rust, fond, rayons, séparation 6 px | ✅ |
| Polices Fira Sans / Fira Mono | ❌ Inter / JetBrains Mono (décision en attente) |
| Contour vert respirant en play | ✅ |
| Hover, état pressé, flash de sélection, infobulles, toasts avec icône, palette Ctrl K | ✅ |
| Onglet Agent = journal diff réel ; fantômes jaunes ; carte Appliquer / Rejeter | ✅ (la carte s'adapte au nombre de lignes et ne passe plus sous le toast) |
| Indicateur client (point pulsant) | ✅ vert = client actif depuis moins de 2 min, jaune = serveur prêt, rouge = serveur arrêté |
| Menu Fichier | ✅ Nouvelle scène, Ouvrir…, Enregistrer (Ctrl S), Enregistrer sous… ; ● rouge « non sauvegardé » réel |

## 6. Ce qui reste

### 🟠 Important
**1. Multi-sélection pour les gizmos et le glisser-déposer** : Gemini
> Dans `cruft_ui/src/lib.rs`, les gizmos (`apply_gizmo_drag_step`) et le drop dans l'outliner n'agissent que sur la sélection principale. Pour un drag de gizmo sur N entités, applique le même delta à chaque entité sélectionnée dans une `cruft_core::CompositeCommand`, avec une fusion par geste (une seule entrée d'historique par drag). Pour le drop, une `CompositeCommand` de `Reparent`. Ajoute des tests.

**2. Champ d'instruction de l'onglet Agent** : décision produit. En V1 sans client LLM intégré, « Envoyer » **copie la consigne** (avec les contextes @entité / @scène) dans le presse-papiers pour la coller dans Claude Code. Le toast l'indique. Un vrai client intégré reste à faire (hors V1 selon A4).

**3. Menus Édition / Créer / Scène / Agent / Fenêtre** : Gemini
> Ces menus n'affichent encore qu'un toast. Branche-les comme `draw_file_menu` : Édition (Annuler / Rétablir / Supprimer), Créer (entité vide + primitives via `cruft_core::Primitive`), Scène (Play / Stop), Agent (mode proposer / direct, appliquer / rejeter), Fenêtre (réafficher un onglet fermé).

### 🟢 Confort
**4. Polices** : à toi de trancher entre Fira (tableau des tokens) et Inter (prompt A1). Puis Gemini : embarquer Fira Sans Regular/Bold + Fira Mono dans `cruft_ui/assets/fonts` et mettre à jour `theme.rs`.

**5. Coût de rendu** : la caméra de jeu rend aussi dans la fenêtre, sous l'UI (double rendu). Une optimisation possible : désactiver son rendu en édition sans modifier ses données, par exemple avec un composant d'exécution non sérialisé ou un `RenderLayers` éditeur. GPT.

**6. Caméra rechargée** : `CameraRenderGraph` n'est pas sauvegardé (non sérialisable), et Bevy avertit « Camera … doesn't have a render graph » sur une caméra chargée depuis un fichier. Sans effet dans l'éditeur (le viewport utilise la caméra éditeur), mais à corriger avant un runtime de jeu autonome. GPT.

**7. Doc** : définir A3. Commiter par lot.

## 7. Corrections appliquées par Claude

| Fichier(s) | Changement |
|---|---|
| `cruft_core/src/composite.rs` | `CompositeCommand`, `InsertComponent` |
| `cruft_core/src/proposals.rs` | `ProposedChange`, `Proposals`, `AgentMode`, `AgentActivity`, `propose` / `apply_proposal` / `reject_proposal` / `submit_changes`, aperçus de fantômes et lignes de diff |
| `cruft_core/src/json.rs` | JSON ↔ reflect (fusion sur la valeur par défaut, noms courts de composants) |
| `cruft_core/src/primitive.rs` | Composant sérialisable `Primitive` |
| `cruft_core/src/commands.rs` | `SpawnEntity::with_boxed`, `with_parent` |
| `cruft_mcp/src/{lib,bridge,server,tools}.rs` | Serveur rmcp complet (stdio + HTTP), 21 outils, capture différée, tests |
| `cruft_scene/src/persistence.rs` | `scene_entities`, `scene_roots`, `DerivedComponents` (+ `lenient`), erreurs nommées |
| `cruft_ui/src/lib.rs` | Panneau Agent branché, suppression des données factices et de `ApplyProposalCommand`, caméra éditeur dédiée, menu Fichier + Ctrl S + dialogue, suppression multi-sélection, outliner par `SceneEntity`, `set_or_insert`, carte adaptative |
| `cruft_editor/src/main.rs` | Plugin MCP, logs sur stderr en stdio, cible de capture, scène de démarrage en `Primitive`, `materialize_primitives` |
| `docs/mcp.md` | Configuration Claude Code / Antigravity / Gemini CLI, format des changements, architecture |
| `cruft_ui/src/display.rs` + menu Fenêtre | Plein écran sans bordure (F11 / Alt+Entrée) et exclusif (mode vidéo du moniteur), résolutions 720p → 4K (ultrawide compris), échelle de l'interface auto (125 % en 1440p, 150 % en 4K quand Windows est à 100 %) ou manuelle (Ctrl + / Ctrl - / Ctrl 0), plafonnée pour qu'une petite fenêtre ne déborde jamais, rendu du viewport à 50–100 %, VSync. Réglages dans `%APPDATA%\Cruft\display.ron`. |

Rien n'est commité : tout le dépôt est encore non suivi, travail des autres agents compris.
