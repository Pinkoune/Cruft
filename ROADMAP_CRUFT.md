# Cruft — Éditeur de jeu Rust avec IA intégrée

À mettre à la racine du repo (copie-le aussi en `AGENTS.md` pour que les agents le lisent automatiquement).

## Stack figée (ne pas en changer sans décision)

- **Runtime** : Bevy 0.20 (ECS, rendu, assets, `bevy_reflect`)
- **UI éditeur** : `bevy_egui` + `egui_dock` (panneaux ancrables), thème custom (voir Design ci-dessous)
- **Gizmos** : `transform-gizmo-bevy` (ou équivalent compatible 0.20)
- **Picking** : `bevy_picking` (intégré)
- **IA** : `rmcp` (SDK MCP officiel), transport stdio + Streamable HTTP
- Avant d'ajouter une crate, vérifier qu'elle supporte **Bevy 0.20** sur crates.io. Sinon, signaler au lieu de bricoler.

## Structure du workspace

```
cruft/
  crates/
    cruft_core/      # commandes, undo/redo, sélection, état éditeur (sans UI)
    cruft_scene/     # sérialisation scènes via reflect, load/save
    cruft_ui/        # thème egui, panneaux, widgets, animations
    cruft_mcp/       # serveur MCP, tools -> commandes cruft_core
    cruft_editor/    # binaire : assemble tout
  assets/
  AGENTS.md
```

Règle d'or : **toute modification de la scène passe par une `EditorCommand`** (UI, raccourci ou IA). C'est ce qui donne undo/redo gratuit pour les actions de l'IA.

## Design (référence visuelle)

Maquette : https://claude.ai/artifact/BVsDrqjTYUKUzyykpAej7B (privée, réservée à Jérémy : les agents n'y ont pas accès). Donner aux agents des captures d'écran + le fichier source `docs/design/cruft-mockup.dc.html` (HTML/CSS de référence : couleurs, tailles, animations).

Tokens (palette officielle de rust-lang.org : gris #2a3439, rouge #a72145, vert #0b7261, jaune #ffc832, bleu lien #4299bf ; éclaircis pour le texte sur fond sombre) :
| Rôle | Valeur |
|---|---|
| Fond app | `#090D10` (quasi-noir bleuté), panneaux séparés de 6 px |
| Panneaux / barres d'onglets | `#141B1F` / `#0F1518`, bordure `#243035`, **rayon 10 px**, ombre portée + liseré clair 1 px en haut pour le relief |
| Champs / boutons | `#0B1013`, rayon 6 px |
| Texte / secondaire | `#E8ECEE` / `#A3B0B6` |
| **Bleu** `#4299BF` | interaction : sélection (fond `#1E3A48`), onglet actif, outil actif, focus, sliders |
| **Vert** `#0B7261` (trait `#3FBF9F`) | état sain / en cours : mode jeu (contour vert du viewport + bouton Play), « appliqué », axe Y |
| **Rouge** `#A72145` (texte `#E0567A`) | destructif / alerte : Rejeter, non sauvegardé, erreurs, tête de lecture, axe X |
| **Jaune** `#FFC832` | en attente : propositions de l'agent (fantômes en pointillés), avertissements, diff `~` |
| Axes X / Y / Z | rouge / vert / bleu ci-dessus |
| Polices | Fira Sans (UI), Fira Mono (valeurs, console), Alfa Slab One (logo uniquement) — celles du site Rust |

Feedbacks obligatoires : hover sur tout ce qui est cliquable (~120 ms via `ctx.animate_bool`), onglet actif = liseré bleu en bas, champs numériques modifiables en glissant (curseur ↔), sliders avec remplissage, toasts clairs en bas du viewport (Annuler + Ctrl Z), **contour vert 2 px autour de tout le panneau viewport en play mode, avec un halo qui « respire »** (2,4 s), palette Ctrl K.
Autres feedbacks : infobulles sur les outils (nom + raccourci), flash bleu à la sélection d'une ligne, état pressé sur champs / tuiles / en-têtes de composants, anneau de focus clavier bleu, toasts avec icône selon le type (vert = ok/play, jaune = agent en attente, rouge = rejet/erreur).
Ciel du viewport : dégradé bleu nuit → horizon rouille, léger halo bleu au centre.

**Concept clé de l'IA** : l'agent ne modifie pas directement la scène, il **propose**. Les entités proposées apparaissent en fantôme (pointillés jaunes) dans le viewport et l'outliner, avec une carte « Appliquer (A) / Rejeter ». L'onglet Agent est un journal d'actions (diff `+ / ~ / -` et outils MCP appelés), pas un chat. Un mode « appliquer directement » reste possible.

---

## Répartition

- **Gemini (Antigravity)** : tout ce qui est visuel / UI (lots A).
- **GPT Astra** : cœur moteur, scène, MCP (lots B).
- **Claude** : seulement revue d'architecture et arbitrage quand un lot bloque.

Ordre : B1 → A1 en parallèle, puis B2 + A2, puis B3 + A3, puis B4, puis A4.

---

## Lot B1 — Squelette + commandes (GPT Astra)

```
Crée un workspace Cargo "cruft" avec les crates cruft_core, cruft_scene, cruft_ui, cruft_mcp, cruft_editor (voir AGENTS.md).
Dans cruft_core :
- Un trait EditorCommand { fn apply(&mut self, world: &mut World); fn undo(&mut self, world: &mut World); fn label(&self) -> String; }
- Une ressource History (pile undo/redo, limite 200) + fusion des commandes consécutives sur le même champ (drag d'une valeur = 1 seule entrée).
- Commandes : SpawnEntity, DespawnEntity (sauvegarde les composants via reflect pour l'undo), SetComponentField (path reflect + valeur sérialisée), Reparent.
- Ressource Selection (entité principale + multi-sélection).
- Événement EditorEvent::Toast(String) que l'UI affichera.
cruft_editor lance une fenêtre Bevy 0.20 avec une scène de test (sol, 2 cubes, lumière, caméra).
Tests unitaires pour apply/undo de chaque commande. cargo clippy sans warning.
```

## Lot A1 — Thème + layout (Gemini)

```
Dans cruft_ui, avec bevy_egui et egui_dock, crée le layout de l'éditeur fidèle aux captures jointes et aux tokens de AGENTS.md :
barre du haut (logo, menus, Play/Pause centrés, champ "Rechercher ou demander… Ctrl K", bouton Assistant), hiérarchie à gauche, viewport au centre, panneau Assets/Console en bas, colonne droite à onglets Inspecteur / Assistant IA, barre d'état.
Le viewport affiche la caméra Bevy rendue dans une texture (render target) dans un panneau egui ; la taille de la texture suit le panneau.
Fais un module theme.rs qui applique tous les tokens (Visuals, Style, polices Inter et JetBrains Mono embarquées), et un module widgets.rs avec : bouton (hover/pressed animés via ctx.animate_bool), ligne de hiérarchie avec badge coloré, champ vec3 avec étiquettes X/Y/Z colorées, toast animé.
Utilise des données factices pour l'instant. Pas de logique de scène ici.
```

## Lot B2 — Inspecteur générique + sérialisation (GPT Astra)

```
Dans cruft_scene : save/load de scène en RON via bevy_reflect (DynamicScene ou BSN si stable en 0.20), uniquement les composants enregistrés dans le TypeRegistry.
Dans cruft_core : une fonction qui, pour une entité, liste ses composants reflect et leurs champs sous une forme neutre (ComponentView { name, fields: Vec<FieldView { path, kind, value }> }) que l'UI pourra afficher sans connaître les types.
Toute édition passe par SetComponentField. Tests : save -> load -> scène identique.
```

## Lot A2 — Brancher l'UI sur le cœur (Gemini)

```
Remplace les données factices de cruft_ui par les vraies : hiérarchie depuis le World (parents/enfants, renommage double-clic, drag & drop = commande Reparent), inspecteur depuis ComponentView (édition par drag sur les champs numériques, chaque édition = SetComponentField), sélection synchronisée entre hiérarchie et viewport (picking), gizmos transform W/E/R, Ctrl Z / Ctrl Shift Z, toasts sur EditorEvent::Toast.
```

## Lot B3 — Play mode + assets (GPT Astra)

```
Play mode : à l'appui sur Play, snapshot de la scène (cruft_scene), activation des systèmes de gameplay via un State Bevy EditorState { Editing, Playing, Paused } ; à Stop, restauration du snapshot.
Navigateur d'assets : scan du dossier assets/, types reconnus (glb, png, exr, ron, scene), drag d'un .glb dans le viewport = SpawnEntity avec SceneRoot.
Console : capture des logs tracing dans un ring buffer lisible par l'UI.
```

## Lot B4 — Serveur MCP (GPT Astra)

```
Dans cruft_mcp, avec rmcp : un serveur MCP lancé dans un thread tokio à côté de Bevy, qui communique avec le World via un channel (crossbeam) de requêtes/réponses traité par un système Bevy chaque frame. Jamais d'accès direct au World depuis tokio.
Ajoute un tool propose_changes(changes[]) : stocke un ChangeSet en attente (non appliqué) que l'UI affiche en fantôme ; apply_proposal / reject_proposal l'appliquent comme UNE commande annulable. En mode "proposer" (défaut), les tools d'écriture passent par propose_changes.
Tools : list_entities, get_entity(id), spawn_entity(name, parent?, components json), despawn_entity, set_component(id, component, json), find_entities(query), list_assets, load_scene, save_scene, enter_play_mode, stop_play_mode, read_logs(n), screenshot_viewport (PNG base64), undo.
Chaque tool qui modifie la scène crée une EditorCommand (donc annulable) et émet un Toast "Assistant : …".
Transports : stdio (pour Claude Code / Gemini CLI) et Streamable HTTP sur 127.0.0.1:7457.
Fournis un exemple de config MCP pour Claude Code et pour Antigravity dans docs/mcp.md.
```

## Lot A4 — Panneau Assistant + palette (Gemini)

```
Onglet Agent dans la colonne droite : PAS de bulles de chat. Un journal d'actions : heure, intitulé, statut (en attente / appliqué / rejeté), bloc diff en mono (+ ajout, ~ modif, - suppression), ligne des outils MCP appelés avec durée. Champ d'instruction en bas avec puces de contexte (@entité sélectionnée, @scène).
Propositions en attente : entités fantômes (contour pointillé jaune animé) dans le viewport et l'outliner + carte flottante en bas à droite du viewport "Appliquer (A) / Rejeter / Détails".
Indicateur "claude-code" avec point pulsant dans la barre de titre.
Palette de commandes Ctrl K : recherche floue sur les commandes et entités ; si le texte ne correspond à rien, proposer "Demander à l'assistant".
Pour la V1, le panneau affiche l'activité MCP des clients externes ; un client LLM intégré viendra plus tard.
```

---

## Points de contrôle (à faire relire par Claude si besoin)

1. Après B1 : le design des commandes / undo tient-il pour le multi-sélection ?
2. Après B4 : le pont tokio ↔ Bevy est-il sans deadlock et sans accès concurrent au World ?
