//! MCP tool catalogue (names, descriptions, JSON schemas).
use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use serde_json::{Value, json};
use std::sync::Arc;

fn schema(value: Value) -> Arc<JsonObject> {
    match value {
        Value::Object(map) => Arc::new(map),
        _ => unreachable!("tool schemas are objects"),
    }
}

fn object(properties: Value, required: &[&str]) -> Arc<JsonObject> {
    schema(json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    }))
}

fn tool(
    name: &'static str,
    description: &'static str,
    input: Arc<JsonObject>,
    read_only: bool,
) -> Tool {
    let mut tool = Tool::new(name, description, input);
    let mut annotations = ToolAnnotations::new();
    annotations.read_only_hint = Some(read_only);
    annotations.destructive_hint = Some(false);
    tool.annotations = Some(annotations);
    tool
}

const ID: &str = "ID d'entité (chaîne décimale renvoyée par list_entities).";

fn change_schema() -> Value {
    json!({
        "type": "object",
        "description": "Un changement. `op` vaut spawn | set_component | set_field | despawn | reparent.",
        "properties": {
            "op": {"type": "string", "enum": ["spawn", "set_component", "set_field", "despawn", "reparent"]},
            "name": {"type": "string", "description": "spawn : nom de la nouvelle entité"},
            "parent": {"type": ["string", "null"], "description": "spawn / reparent : ID du parent (null = racine)"},
            "components": {"type": "object", "description": "spawn : {\"Transform\": {\"translation\": [x,y,z], \"scale\": [x,y,z]}, ...}. Clé = nom court ou chemin complet du type, valeur = JSON partiel fusionné sur la valeur par défaut."},
            "primitive": {"type": "string", "enum": ["cube", "sphere", "plane", "capsule", "cylinder"], "description": "spawn : ajoute un mesh visible"},
            "color": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": "spawn : couleur sRGB 0..1 de la primitive"},
            "entity": {"type": "string", "description": ID},
            "component": {"type": "string", "description": "set_component / set_field : type du composant (ex. Transform)"},
            "value": {"description": "set_component : JSON du composant ; set_field : valeur RON en chaîne (ex. \"2.5\")"},
            "path": {"type": "string", "description": "set_field : chemin reflect (ex. translation.x)"}
        },
        "required": ["op"]
    })
}

pub fn all() -> Vec<Tool> {
    let spawn_props = json!({
        "name": {"type": "string"},
        "parent": {"type": "string", "description": "ID du parent (optionnel)"},
        "components": {"type": "object", "description": "Composants en JSON, ex. {\"Transform\": {\"translation\": [0, 1, 0]}}"},
        "primitive": {"type": "string", "enum": ["cube", "sphere", "plane", "capsule", "cylinder"]},
        "color": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3}
    });
    vec![
        tool(
            "list_entities",
            "Liste les entités de la scène (id, nom, parent, nombre d'enfants).",
            object(json!({}), &[]),
            true,
        ),
        tool(
            "find_entities",
            "Cherche des entités dont le nom contient `query` (insensible à la casse), optionnellement filtrées par composant.",
            object(
                json!({"query": {"type": "string"}, "component": {"type": "string", "description": "ex. PointLight"}}),
                &[],
            ),
            true,
        ),
        tool(
            "get_entity",
            "Détaille une entité : composants reflect et champs, valeurs en JSON.",
            object(
                json!({"id": {"type": "string", "description": ID}}),
                &["id"],
            ),
            true,
        ),
        tool(
            "spawn_entity",
            "Crée une entité. En mode « proposer » (défaut), crée une proposition en attente que l'utilisateur appliquera ou rejettera.",
            object(spawn_props, &["name"]),
            false,
        ),
        tool(
            "despawn_entity",
            "Supprime une entité et ses enfants (via proposition en mode « proposer »).",
            object(
                json!({"id": {"type": "string", "description": ID}}),
                &["id"],
            ),
            false,
        ),
        tool(
            "set_component",
            "Remplace ou ajoute un composant (JSON partiel fusionné sur la valeur par défaut). Via proposition en mode « proposer ».",
            object(
                json!({"id": {"type": "string", "description": ID}, "component": {"type": "string"}, "value": {}}),
                &["id", "component", "value"],
            ),
            false,
        ),
        tool(
            "set_field",
            "Modifie un champ précis (chemin reflect + valeur RON). Via proposition en mode « proposer ».",
            object(
                json!({"id": {"type": "string", "description": ID}, "component": {"type": "string"}, "path": {"type": "string"}, "value": {"type": "string", "description": "RON, ex. \"2.5\" ou \"(1.0, 2.0, 3.0)\""}}),
                &["id", "component", "path", "value"],
            ),
            false,
        ),
        tool(
            "reparent",
            "Change le parent d'une entité (null = racine). Via proposition en mode « proposer ».",
            object(
                json!({"id": {"type": "string", "description": ID}, "parent": {"type": ["string", "null"]}}),
                &["id"],
            ),
            false,
        ),
        tool(
            "propose_changes",
            "Regroupe plusieurs changements en UNE proposition (fantômes jaunes + carte Appliquer/Rejeter dans l'éditeur). Appliquée = une seule entrée d'annulation. En mode « direct », appliquée immédiatement.",
            object(
                json!({"title": {"type": "string", "description": "Intitulé court affiché dans le journal"}, "changes": {"type": "array", "items": change_schema(), "minItems": 1}}),
                &["title", "changes"],
            ),
            false,
        ),
        tool(
            "list_proposals",
            "Liste les propositions (en attente / appliquées / rejetées) et le mode de l'agent.",
            object(json!({}), &[]),
            true,
        ),
        tool(
            "apply_proposal",
            "Applique une proposition en attente (la dernière si `id` absent) comme UNE commande annulable.",
            object(json!({"id": {"type": "integer"}}), &[]),
            false,
        ),
        tool(
            "reject_proposal",
            "Rejette une proposition en attente sans toucher la scène.",
            object(json!({"id": {"type": "integer"}}), &[]),
            false,
        ),
        tool(
            "list_assets",
            "Liste les fichiers du dossier assets/ (glb, png, exr, ron, scene).",
            object(json!({}), &[]),
            true,
        ),
        tool(
            "load_scene",
            "Ajoute une scène RON (chemin relatif à assets/) à la scène courante. Annulable.",
            object(json!({"path": {"type": "string"}}), &["path"]),
            false,
        ),
        tool(
            "save_scene",
            "Enregistre la scène courante en RON (chemin relatif à assets/, ex. scenes/niveau1.scn.ron).",
            object(json!({"path": {"type": "string"}}), &["path"]),
            false,
        ),
        tool(
            "enter_play_mode",
            "Lance le play mode (snapshot de la scène).",
            object(json!({}), &[]),
            false,
        ),
        tool(
            "stop_play_mode",
            "Arrête le play mode et restaure la scène d'avant Play.",
            object(json!({}), &[]),
            false,
        ),
        tool(
            "read_logs",
            "Renvoie les `n` derniers logs de la console (défaut 100).",
            object(
                json!({"n": {"type": "integer", "minimum": 1, "maximum": 2000}}),
                &[],
            ),
            true,
        ),
        tool(
            "screenshot_viewport",
            "Capture le viewport 3D (PNG). `full_window: true` capture toute la fenêtre de l'éditeur, avec les fantômes des propositions et les panneaux.",
            object(json!({"full_window": {"type": "boolean"}}), &[]),
            true,
        ),
        tool(
            "undo",
            "Annule la dernière commande de l'historique.",
            object(json!({}), &[]),
            false,
        ),
        tool(
            "redo",
            "Rétablit la dernière commande annulée.",
            object(json!({}), &[]),
            false,
        ),
    ]
}
