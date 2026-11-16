# Serveur MCP de Cruft

L'éditeur embarque un serveur MCP (`crates/cruft_mcp`, SDK `rmcp` 3.10). Il démarre avec l'éditeur.

| Transport | Adresse / lancement | Quand l'utiliser |
|---|---|---|
| **Streamable HTTP** (toujours actif) | `http://127.0.0.1:7457/mcp` | **Recommandé** : l'éditeur est déjà ouvert, plusieurs clients peuvent s'y connecter. |
| **stdio** | `cruft_editor --mcp-stdio` | Le client lance lui-même l'éditeur. Les logs passent alors sur stderr ; stdout ne transporte que le protocole. |

Le serveur n'écoute que sur `127.0.0.1` (hôtes autorisés : localhost, 127.0.0.1, ::1). Les outils de fichiers (`save_scene`, `load_scene`) n'acceptent que des chemins relatifs au dossier `assets/`, sans `..`.

## Configurer les clients

### Claude Code

Avec l'éditeur déjà lancé (HTTP) :

```bash
claude mcp add --transport http cruft http://127.0.0.1:7457/mcp
```

Ou, pour que Claude Code lance l'éditeur (stdio) :

```bash
claude mcp add cruft -- C:/Users/<vous>/Projects/Cruft/target/debug/cruft_editor.exe --mcp-stdio
```

Équivalent dans `.mcp.json` à la racine du repo :

```json
{
  "mcpServers": {
    "cruft": { "type": "http", "url": "http://127.0.0.1:7457/mcp" }
  }
}
```

### Antigravity / Gemini CLI

Dans le fichier de configuration MCP (Antigravity : *Manage MCP servers → View raw config* ; Gemini CLI : `~/.gemini/settings.json`) :

```json
{
  "mcpServers": {
    "cruft": { "httpUrl": "http://127.0.0.1:7457/mcp" }
  }
}
```

Selon la version, Antigravity attend `serverUrl` au lieu de `httpUrl`. En stdio : `{"command": "…/cruft_editor.exe", "args": ["--mcp-stdio"]}`.

## Mode « proposer » (par défaut)

L'agent ne modifie pas la scène directement. `spawn_entity`, `set_component`, `set_field`, `reparent`, `despawn_entity` et `propose_changes` créent une **proposition en attente** :

- fantômes en pointillés jaunes dans le viewport et l'outliner, carte « Appliquer (A) / Rejeter » ;
- **Appliquer** exécute toute la proposition comme **une seule** commande annulable (Ctrl Z) ;
- **Rejeter** ne touche pas la scène.

Le bouton « Mode : proposer / direct » de l'onglet Assistant IA bascule en mode direct : les mêmes outils appliquent alors immédiatement (toujours en une seule entrée d'historique, annulable).

Regroupez les changements liés dans **un** `propose_changes` :

```json
{
  "title": "Ajouter trois couverts autour du centre",
  "changes": [
    {"op": "spawn", "name": "Cover_A", "primitive": "cube", "color": [1.0, 0.78, 0.2],
     "components": {"Transform": {"translation": [0, 0.5, -3], "scale": [1.8, 1, 0.4]}}},
    {"op": "set_field", "entity": "4294966944", "component": "Transform", "path": "scale.y", "value": "2.0"},
    {"op": "set_component", "entity": "4294966944", "component": "PointLight", "value": {"intensity": 800000}},
    {"op": "reparent", "entity": "4294966944", "parent": null},
    {"op": "despawn", "entity": "4294966950"}
  ]
}
```

- Les **ID d'entité** sont des chaînes décimales, renvoyées par `list_entities` et `find_entities`.
- Les **composants** s'écrivent en JSON avec leur nom court (`Transform`) ou leur chemin complet. Un objet partiel est fusionné sur la valeur par défaut. `Transform` : `translation` et `scale` en `[x, y, z]`, `rotation` en quaternion `[x, y, z, w]`.
- `set_field` prend une valeur **RON** en chaîne (`"2.5"`, `"(1.0, 2.0, 3.0)"`).
- `primitive` : `cube`, `sphere`, `plane`, `capsule`, `cylinder` (taille unitaire, à dimensionner avec `scale`). La primitive est enregistrée comme donnée (`Primitive`), donc la scène reste sauvegardable.

## Outils

| Outil | Effet |
|---|---|
| `list_entities`, `find_entities(query?, component?)`, `get_entity(id)` | Lecture de la scène (composants en JSON) |
| `spawn_entity`, `despawn_entity`, `set_component`, `set_field`, `reparent` | Écriture unitaire (proposition en mode « proposer ») |
| `propose_changes(title, changes[])` | Plusieurs changements = une proposition |
| `list_proposals`, `apply_proposal(id?)`, `reject_proposal(id?)` | Gestion des propositions (la dernière en attente si `id` est absent) |
| `list_assets`, `load_scene(path)`, `save_scene(path)` | Fichiers sous `assets/` (`load_scene` ajoute la scène, annulable) |
| `enter_play_mode`, `stop_play_mode` | Play mode (snapshot / restauration) |
| `read_logs(n?)` | Console |
| `screenshot_viewport(full_window?)` | PNG du viewport ; `full_window: true` = toute la fenêtre (fantômes et panneaux compris) |
| `undo`, `redo` | Historique |

Toute écriture passe par une `EditorCommand` dans `History` et émet un toast « Assistant : … ».

## Architecture (point de contrôle 2)

```
client MCP ──stdio / HTTP──> rmcp (thread « cruft-mcp », runtime tokio)
                                │  McpClient::call : try_send sur un canal crossbeam borné (128)
                                ▼  puis attente d'un oneshot, timeout 10 s
                     système Bevy exclusif `process_requests` (Update, ≤ 32 requêtes / frame)
                                │  seul endroit qui touche le World
                                ▼
                     cruft_core::proposals / History / cruft_scene
```

- Tokio n'accède jamais au World. Pas de Mutex partagé avec le World. Aucun `recv` bloquant côté Bevy (`try_iter`).
- Une requête dont le client a abandonné (timeout, déconnexion) est ignorée avant exécution.
- `screenshot_viewport` répond de façon différée, quand Bevy a capturé l'image (observer `ScreenshotCaptured`).
- L'activité (outil, durée, client) est enregistrée dans `cruft_core::AgentActivity`, affichée par l'onglet Assistant IA.
