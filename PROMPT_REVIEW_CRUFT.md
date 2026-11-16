Tu reprends le projet Cruft (éditeur de jeu type Unity en Rust, Bevy 0.20 + egui + serveur MCP). Le code a été écrit par d'autres agents (Gemini via Antigravity pour l'UI, GPT pour le cœur et le MCP). Ton rôle : te mettre à jour sur le projet, contrôler leur travail et me faire un rapport. Économise les tokens : lis ciblé, pas de réécriture massive.

## 1. Mise à jour du contexte
- Lis `AGENTS.md` (ou `ROADMAP_CRUFT.md`) en entier : stack, structure, règles, lots A1→A4 et B1→B4.
- Survole `docs/design/cruft-mockup.dc.html` uniquement pour les couleurs, rayons et feedbacks attendus.
- `git log --oneline -50` et `git diff --stat` depuis le premier commit pour voir qui a fait quoi.
- Arborescence : `tree -L 3 -I target` (ou équivalent).

## 2. Contrôles automatiques
Lance et note le résultat de chaque commande :
1. `cargo build --workspace`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace`
4. `cargo tree -d | head -50` : doublons de versions de Bevy/egui/wgpu
5. Vérifie dans les `Cargo.toml` que chaque crate liée à Bevy (bevy_egui, egui_dock, gizmo, avian…) est bien compatible Bevy 0.20.

## 3. Contrôles d'architecture (les règles non négociables)
1. **Toute modification de scène passe par une `EditorCommand`**. Cherche les mutations directes du World hors de `cruft_core` (ex. `commands.spawn`, `world.spawn`, `despawn`, `get_mut::<Transform>` dans `cruft_ui` et `cruft_mcp`). Liste chaque violation avec fichier:ligne.
2. **Undo/redo** : chaque commande a un `undo` réel et testé ? Le drag d'un champ fusionne bien en une seule entrée d'historique ?
3. **Pont MCP** : aucun accès au World depuis tokio. Vérifie que c'est un channel requête/réponse traité par un système Bevy. Cherche les risques de deadlock (recv bloquant dans un système, Mutex partagé avec le World).
4. **Propositions de l'agent** : `propose_changes` stocke un ChangeSet non appliqué, affiché en fantôme ; `apply_proposal` = UNE commande annulable ; `reject_proposal` ne touche pas la scène.
5. **Play mode** : snapshot avant Play, restauration exacte à Stop.
6. Liste des tools MCP exposés vs ceux demandés dans le lot B4 (manquants / en trop).

## 4. Contrôles fonctionnels
1. Lance l'éditeur (`cargo run -p cruft_editor`) : il démarre sans panic ? Note les warnings au lancement.
2. Teste le serveur MCP en stdio : handshake `initialize` + `tools/list`, puis un `spawn_entity` suivi d'un `undo`. Si possible avec `npx @modelcontextprotocol/inspector`.
3. Sauvegarde puis recharge une scène : résultat identique ?

## 5. Contrôle UI (par rapport à la maquette)
Coche ou non, sans chercher la perfection au pixel :
- Couleurs Rust : bleu `#4299BF` = sélection/actif, vert `#0B7261`/`#3FBF9F` = play/appliqué, rouge `#A72145`/`#E0567A` = destructif, jaune `#FFC832` = propositions en attente.
- Panneaux séparés et arrondis (10 px), fond `#090D10`, polices Fira Sans / Fira Mono.
- Contour vert autour du viewport en play mode.
- Feedbacks : hover animés, état pressé, flash de sélection, infobulles outils, toasts avec icône, palette Ctrl K.
- Onglet Agent = journal d'actions en diff (pas de chat), fantômes jaunes dans viewport + outliner, carte Appliquer/Rejeter.

## 6. Livrable
Écris `docs/REVIEW.md` avec :
- Un tableau par lot (A1–A4, B1–B4) : Fait / Partiel / Absent, en une ligne de justification.
- Les résultats build / clippy / tests.
- Les violations d'architecture avec fichier:ligne.
- Une liste priorisée des corrections : 🔴 bloquant, 🟠 important, 🟢 confort. Pour chacune, indique quel agent doit la faire (Gemini = UI, GPT = cœur/MCP) et donne le prompt court à lui coller.

Ne corrige toi-même que les bloquants triviaux (moins de ~20 lignes, ex. compilation cassée, import manquant), dans un commit séparé `fix(review): …`. Tout le reste va dans le rapport. Termine par un résumé de 5 lignes maximum.
