//! UI integration for the Cruft editor (`bevy_egui` + `egui_dock`).
//!
//! Connects the Cruft layout to the real ECS `World` and `cruft_core`:
//! - Outliner hierarchy built from `World` (`ChildOf`/`Children`, double-click rename via `SetComponentField`, drag & drop via `Reparent`)
//! - Generic Inspector driven by `cruft_core::component_views` (`ComponentView` / `FieldView`), where dragging numeric fields executes `SetComponentField` commands merged per drag gesture
//! - Synchronized `Selection` between Outliner and 3D Viewport (3D ray-cast picking via `bevy_picking`)
//! - Interactive 3D Transform Gizmos (`W` Move, `E` Rotate, `R` Scale) routing mutations through `SetComponentField`
//! - `Ctrl+Z` / `Ctrl+Shift+Z` (`Ctrl+Y`) undo/redo and animated toasts driven by `EditorEvent::Toast`

pub mod display;
pub mod feedback;
mod shortcuts;
pub mod theme;
pub mod widgets;

use bevy::{
    camera::RenderTarget,
    ecs::entity::EntityHashSet,
    math::Affine3A,
    mesh::{Indices, PrimitiveTopology, VertexAttributeValues},
    picking::mesh_picking::ray_cast::{Backfaces, RayMeshHit, ray_mesh_intersection},
    prelude::*,
    reflect::serde::TypedReflectSerializer,
    render::render_resource::{
        Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
    },
};
use bevy_egui::{
    EguiContext, EguiGlobalSettings, EguiPlugin, EguiPrimaryContextPass, EguiTextureHandle,
    EguiUserTextures, PrimaryEguiContext,
    egui::{
        self, Align, Align2, Color32, CornerRadius, CursorIcon, Frame, Id, Key, Layout, Margin,
        Pos2, Rect, RichText, ScrollArea, Sense, Shadow, Stroke, StrokeKind, Ui, Vec2, WidgetText,
    },
};
use cruft_core::{
    ComponentView, DespawnEntity, EditorEvent, FieldKind, FieldView, History, Reparent, Selection,
    SetComponentField, SpawnEntity, component_views, resolve_entity,
};
use cruft_core::{EditorState, console::ConsoleLog};
use cruft_scene::{AssetCatalog, AssetKind, PlayAction, import_glb, request_play_action};
pub use display::{DisplayMode, DisplaySettings};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
pub use feedback::ToastQueue;
use std::collections::{HashMap, HashSet};
use theme::{
    ACCENT_BLUE, ACCENT_BLUE_LIGHT, ACCENT_GREEN, ACCENT_GREEN_FILL, ACCENT_RED, ACCENT_YELLOW,
    AXIS_X, AXIS_Y, AXIS_Z, BG_AGENT_BADGE, BG_APP, BG_BUTTON_HOVER, BG_FIELD, BG_FIELD_HOVER,
    BG_MENU_HOVER, BG_PANEL, BG_SELECTION, BG_TAB_BAR, BORDER_AGENT, BORDER_CONTROL, BORDER_HOVER,
    BORDER_KBD, BORDER_PANEL, LOGO_RED, RADIUS_CARD, RADIUS_PANEL, RADIUS_WIDGET,
    RELIEF_TOP_HIGHLIGHT, TEXT_BLUE_CHIP, TEXT_BODY, TEXT_CONTROL, TEXT_MUTED, TEXT_PRIMARY,
    TEXT_SECONDARY, TEXT_YELLOW, TEXT_YELLOW_LIGHT, font_mono, font_sans, font_sans_bold,
    lerp_color,
};
use widgets::{
    BadgeKind, ButtonVariant, CruftButton, EntityKind, HierarchyRowProps, ToastData, ToastKind,
    animated_toast, draggable_scalar_row, hierarchy_rename_row, hierarchy_row, kbd_badge,
    paint_kbd_in_rect, toggle_switch, vec3_field_detailed,
};

/// Plugin registering the Cruft editor UI, theme, dock state, and viewport render target.
pub struct CruftUiPlugin;

impl Plugin for CruftUiPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<EguiPlugin>() {
            app.add_plugins(EguiPlugin::default());
        }
        if !app.world().contains_resource::<DisplaySettings>() {
            app.insert_resource(DisplaySettings::load());
        }

        app.insert_resource(EguiGlobalSettings {
            auto_create_primary_context: false,
            ..default()
        })
        .init_resource::<ToastQueue>()
        .init_resource::<EditorUiState>()
        .init_resource::<shortcuts::Shortcuts>()
        .add_systems(PreUpdate, shortcuts::collect)
        .init_resource::<AppTypeRegistry>()
        .register_type::<Visibility>()
        .register_type::<PointLight>()
        .register_type::<DirectionalLight>()
        .register_type::<SpotLight>()
        .register_type::<Camera3d>()
        .add_observer(feedback::collect_toasts)
        .add_systems(Startup, setup_ui_and_viewport_target)
        .init_resource::<EditorCameraState>()
        .insert_resource(display::PersistDisplaySettings)
        .add_systems(Update, display::apply_display_settings)
        .add_systems(PreUpdate, drive_editor_camera)
        .add_systems(EguiPrimaryContextPass, editor_ui_system);
    }
}

/// Marker for the dedicated 2D camera driving the primary Egui context.
#[derive(Component)]
pub struct EditorUiCamera;

/// Editor-only 3D camera rendering the viewport texture. Scene cameras are never
/// modified by the editor: in play mode this camera follows the game camera.
#[derive(Component)]
#[component(clone_behavior = Ignore)]
pub struct EditorViewportCamera;

/// Editor camera pose kept aside while it follows the game camera in play mode.
#[derive(Resource, Default)]
pub struct EditorCameraState {
    framed: bool,
    saved: Option<(Transform, Projection)>,
}

/// Resource holding the offscreen texture rendered by the Bevy 3D camera and displayed inside the egui Viewport panel.
#[derive(Resource)]
pub struct ViewportRenderTarget {
    pub image: Handle<Image>,
    pub size: UVec2,
}

fn setup_ui_and_viewport_target(
    mut commands: Commands,
    mut images: Option<ResMut<Assets<Image>>>,
    mut egui_user_textures: Option<ResMut<EguiUserTextures>>,
) {
    if let (Some(images), Some(egui_user_textures)) = (images.as_mut(), egui_user_textures.as_mut())
    {
        let initial_size = UVec2::new(900, 520);
        let extent = Extent3d {
            width: initial_size.x,
            height: initial_size.y,
            depth_or_array_layers: 1,
        };

        let mut image = Image {
            texture_descriptor: TextureDescriptor {
                label: Some("cruft_viewport_render_target"),
                size: extent,
                dimension: TextureDimension::D2,
                format: TextureFormat::Bgra8UnormSrgb,
                mip_level_count: 1,
                sample_count: 1,
                usage: TextureUsages::TEXTURE_BINDING
                    | TextureUsages::COPY_DST
                    | TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            },
            ..default()
        };
        image.resize(extent);

        let image_handle = images.add(image);
        egui_user_textures.add_image(EguiTextureHandle::Strong(image_handle.clone()));

        commands.spawn((
            EditorViewportCamera,
            cruft_core::EditorOnly,
            Camera3d::default(),
            Camera {
                order: -1,
                clear_color: ClearColorConfig::Custom(Color::srgb_u8(0x0C, 0x1A, 0x22)),
                ..default()
            },
            RenderTarget::Image(image_handle.clone().into()),
            Transform::from_xyz(6.0, 5.0, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
        ));
        commands.insert_resource(ViewportRenderTarget {
            image: image_handle,
            size: initial_size,
        });
    }

    commands.spawn((
        EditorUiCamera,
        cruft_core::EditorOnly,
        PrimaryEguiContext,
        Camera2d,
        Camera {
            order: 10,
            clear_color: ClearColorConfig::Custom(Color::srgb_u8(0x09, 0x0D, 0x10)),
            ..default()
        },
    ));
}

/// Frames the editor camera on the first scene camera at startup, and makes it
/// follow the game camera during play (restoring its pose on Stop).
#[allow(clippy::type_complexity)]
fn drive_editor_camera(
    mut state: ResMut<EditorCameraState>,
    editor_state: Option<Res<State<EditorState>>>,
    mut editor_camera: Query<(&mut Transform, &mut Projection), With<EditorViewportCamera>>,
    scene_cameras: Query<
        (&GlobalTransform, &Projection),
        (
            With<Camera3d>,
            Without<EditorViewportCamera>,
            Without<EditorUiCamera>,
        ),
    >,
) {
    let Ok((mut transform, mut projection)) = editor_camera.single_mut() else {
        return;
    };
    let playing = editor_state.is_some_and(|s| *s.get() != EditorState::Editing);
    let game = scene_cameras.iter().next();
    if playing {
        if state.saved.is_none() {
            state.saved = Some((*transform, projection.clone()));
        }
        if let Some((global, game_projection)) = game {
            *transform = global.compute_transform();
            *projection = game_projection.clone();
        }
    } else if let Some((saved_transform, saved_projection)) = state.saved.take() {
        *transform = saved_transform;
        *projection = saved_projection;
    } else if !state.framed
        && let Some((global, _)) = game
    {
        *transform = global.compute_transform();
        state.framed = true;
    }
}

// ---------------------------------------------------------------------------
// Dock Tabs & Editor UI State
// ---------------------------------------------------------------------------

/// Dockable tabs in the Cruft workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EditorTab {
    Outliner,
    Layers,
    Viewport,
    Assets,
    Console,
    Timeline,
    Inspector,
    Assistant,
}

/// Active transform gizmo mode (`Q` Select, `W` Move, `E` Rotate, `R` Scale).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformTool {
    Select,
    Move,
    Rotate,
    Scale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadingMode {
    Wireframe,
    Solid,
    Lit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordSpace {
    Local,
    Global,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposalState {
    Pending,
    Applied,
    Rejected,
}

/// Execution mode for incoming agent / MCP mutations (owned by `cruft_core`).
pub use cruft_core::AgentMode;

/// Diff operation symbol in the Agent action journal (`+`, `~`, `-`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffOp {
    Add,
    Modify,
    Remove,
}

impl DiffOp {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Modify => "~",
            Self::Remove => "-",
        }
    }

    pub fn from_core(op: cruft_core::DiffOp) -> Self {
        match op {
            cruft_core::DiffOp::Add => Self::Add,
            cruft_core::DiffOp::Modify => Self::Modify,
            cruft_core::DiffOp::Remove => Self::Remove,
        }
    }

    pub fn color(self) -> Color32 {
        match self {
            Self::Add => ACCENT_GREEN,
            Self::Modify => ACCENT_YELLOW,
            Self::Remove => ACCENT_RED,
        }
    }
}

/// Single monospace diff line inside an [`AgentLogEntry`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentDiffLine {
    pub op: DiffOp,
    pub text: String,
}

/// Entry in the right-hand Agent action journal (no chat bubbles).
#[derive(Clone, Debug)]
pub struct AgentLogEntry {
    pub time: String,
    pub title: String,
    pub status: ProposalState,
    pub diff_lines: Vec<AgentDiffLine>,
    pub tools: String,
}

/// Proposed entity displayed as an animated yellow dashed ghost in the Viewport and Outliner.
#[derive(Clone, Debug)]
pub struct GhostEntity {
    pub name: String,
    pub position: Vec3,
    pub scale: Vec3,
    pub op: DiffOp,
}

/// Active 3D gizmo handle drag state.
#[derive(Clone, Copy, Debug)]
pub struct ActiveGizmoDrag {
    pub entity: Entity,
    pub tool: TransformTool,
    pub axis: usize,
}

#[derive(Clone, Debug)]
struct AssetTile<'a> {
    name: &'a str,
    kind: &'static str,
    c1: Color32,
    c2: Color32,
    c3: Color32,
}

#[derive(Clone)]
struct AssetDrag {
    path: String,
}

/// Snapshot of a single entity in the Outliner hierarchy constructed from the `World`.
#[derive(Clone, Debug)]
pub struct HierarchyItem {
    pub entity: Entity,
    pub name: String,
    pub kind: EntityKind,
    pub depth: usize,
    pub tag: String,
    pub badge_kind: BadgeKind,
    pub hidden: bool,
    pub has_children: bool,
}

#[derive(Resource)]
pub struct EditorUiState {
    pub theme_initialized: bool,
    pub dock_state: DockState<EditorTab>,
    pub outliner_filter: String,
    pub palette_open: bool,
    pub palette_query: String,
    pub playing: bool,
    pub paused: bool,
    pub tool: TransformTool,
    pub shading: ShadingMode,
    pub space: CoordSpace,
    pub proposal: ProposalState,
    pub agent_mode: AgentMode,
    pub agent_prompt: String,
    pub context_include_selected: bool,
    pub context_include_scene: bool,
    pub agent_log: Vec<AgentLogEntry>,
    pub pending_ghosts: Vec<GhostEntity>,
    pub palette_selected_idx: usize,
    pub spawn_counter: usize,
    /// ID of the core proposal shown by the floating card.
    pub pending_proposal_id: Option<u64>,
    /// Header / top-bar status of the MCP connection.
    pub agent_status: AgentStatusView,
    /// Text to put on the clipboard on the next frame (needs the egui context).
    pub clipboard_request: Option<String>,
    /// Current scene file, relative to `assets/`.
    pub scene_path: Option<String>,
    /// `History::undo_len` at the last save/open (dirty marker).
    pub saved_undo_len: Option<usize>,
    pub file_dialog: Option<FileDialog>,
    /// "1920×1080 · UI 150 %" for the status bar.
    pub display_label: String,
    pub collapsed_entities: EntityHashSet,
    pub renaming_entity: Option<Entity>,
    pub rename_buffer: String,
    pub dragged_hierarchy_entity: Option<Entity>,
    pub active_gizmo_drag: Option<ActiveGizmoDrag>,
    pub hovered_gizmo_axis: Option<usize>,
    pub last_viewport_rect: Option<Rect>,
    dragged_asset: Option<AssetDrag>,
    pub field_edit_buffers: HashMap<(Entity, String, String), String>,
    pub collapsed_components: HashSet<String>,
    pub active_toast: Option<(ToastData, f64)>,
    pub last_toast_cache: Option<ToastData>,
    pub current_time: f64,
}

impl Default for EditorUiState {
    fn default() -> Self {
        let mut dock_state = DockState::new(vec![EditorTab::Viewport]);
        let surface = dock_state.main_surface_mut();

        let [center_node, _right_node] = surface.split_right(
            NodeIndex::root(),
            0.75,
            vec![EditorTab::Inspector, EditorTab::Assistant],
        );

        let [center_area, _left_node] = surface.split_left(
            center_node,
            0.23,
            vec![EditorTab::Outliner, EditorTab::Layers],
        );

        let [_viewport_node, _bottom_node] = surface.split_below(
            center_area,
            0.72,
            vec![EditorTab::Assets, EditorTab::Console, EditorTab::Timeline],
        );

        Self {
            theme_initialized: false,
            dock_state,
            outliner_filter: String::new(),
            palette_open: false,
            palette_query: String::new(),
            playing: false,
            paused: false,
            tool: TransformTool::Move,
            shading: ShadingMode::Lit,
            space: CoordSpace::Local,
            proposal: ProposalState::Applied,
            agent_mode: AgentMode::Propose,
            agent_prompt: String::new(),
            context_include_selected: true,
            context_include_scene: true,
            agent_log: Vec::new(),
            pending_ghosts: Vec::new(),
            palette_selected_idx: 0,
            spawn_counter: 0,
            pending_proposal_id: None,
            agent_status: AgentStatusView::default(),
            clipboard_request: None,
            scene_path: None,
            saved_undo_len: Some(0),
            file_dialog: None,
            display_label: String::new(),
            collapsed_entities: EntityHashSet::default(),
            renaming_entity: None,
            rename_buffer: String::new(),
            dragged_hierarchy_entity: None,
            active_gizmo_drag: None,
            hovered_gizmo_axis: None,
            last_viewport_rect: None,
            dragged_asset: None,
            field_edit_buffers: HashMap::new(),
            collapsed_components: HashSet::new(),
            active_toast: None,
            last_toast_cache: None,
            current_time: 0.0,
        }
    }
}

impl EditorUiState {
    pub fn show_toast(&mut self, message: impl Into<String>, kind: ToastKind) {
        self.active_toast = Some((
            ToastData {
                message: message.into(),
                kind,
            },
            self.current_time + 2.8,
        ));
    }

    pub fn focus_tab(&mut self, target: EditorTab) {
        if let Some(tab_path) = self.dock_state.find_tab(&target) {
            let _ = self.dock_state.set_active_tab(tab_path);
            self.dock_state
                .set_focused_node_and_surface(tab_path.node_path());
        }
    }
}

// ---------------------------------------------------------------------------
// World Hierarchy & Scene Helpers
// ---------------------------------------------------------------------------

/// Determines whether `entity` is a user-visible scene entity (excluding UI cameras, windows, observers).
pub fn is_scene_entity(world: &World, entity: Entity) -> bool {
    // Scene entities carry `SceneEntity` (set by SpawnEntity / LoadScene), or
    // descend from one (glTF sub-scenes). Engine and editor entities do not.
    let mut current = Some(entity);
    while let Some(id) = current {
        let Ok(entity_ref) = world.get_entity(id) else {
            return false;
        };
        if entity_ref.contains::<cruft_core::EditorOnly>() {
            return false;
        }
        if entity_ref.contains::<cruft_core::SceneEntity>() {
            return true;
        }
        current = entity_ref.get::<ChildOf>().map(ChildOf::parent);
    }
    false
}

/// Builds the ordered depth-first hierarchy list of scene entities from the `World`.
pub fn collect_hierarchy(
    world: &World,
    collapsed: &EntityHashSet,
    filter_lower: &str,
) -> (Vec<HierarchyItem>, usize) {
    let mut all_scene_entities: Vec<Entity> = world
        .iter_entities()
        .map(|e| e.id())
        .filter(|&e| is_scene_entity(world, e))
        .collect();
    all_scene_entities.sort_by_key(|e| e.index());

    let scene_set: EntityHashSet = all_scene_entities.iter().copied().collect();
    let total_count = all_scene_entities.len();

    let mut roots: Vec<Entity> = all_scene_entities
        .into_iter()
        .filter(|&e| {
            world
                .get::<ChildOf>(e)
                .map(ChildOf::parent)
                .is_none_or(|p| !scene_set.contains(&p))
        })
        .collect();
    roots.sort_by_key(|e| e.index());

    let mut items = Vec::with_capacity(total_count);
    for root in roots {
        collect_subtree(
            world,
            root,
            0,
            &scene_set,
            collapsed,
            filter_lower,
            &mut items,
        );
    }

    (items, total_count)
}

fn collect_subtree(
    world: &World,
    entity: Entity,
    depth: usize,
    scene_set: &EntityHashSet,
    collapsed: &EntityHashSet,
    filter_lower: &str,
    out: &mut Vec<HierarchyItem>,
) {
    let Ok(entity_ref) = world.get_entity(entity) else {
        return;
    };

    let name = entity_ref
        .get::<Name>()
        .map(|n| n.as_str().to_owned())
        .unwrap_or_else(|| format!("Entity #{}", entity.index()));

    let kind = if entity_ref.contains::<PointLight>()
        || entity_ref.contains::<DirectionalLight>()
        || entity_ref.contains::<SpotLight>()
    {
        EntityKind::Light
    } else if entity_ref.contains::<Camera3d>() {
        EntityKind::Camera
    } else if entity_ref.contains::<Mesh3d>() {
        EntityKind::Mesh
    } else {
        EntityKind::Entity
    };

    let children_vec: Vec<Entity> = entity_ref
        .get::<Children>()
        .map(|c| c.iter().filter(|child| scene_set.contains(child)).collect())
        .unwrap_or_default();
    let has_children = !children_vec.is_empty();

    let (tag, badge_kind) = if has_children {
        (children_vec.len().to_string(), BadgeKind::Neutral)
    } else {
        match kind {
            EntityKind::Camera => ("cam".into(), BadgeKind::Prefab),
            EntityKind::Light => ("light".into(), BadgeKind::Agent),
            EntityKind::Mesh => ("mesh".into(), BadgeKind::Neutral),
            _ => (String::new(), BadgeKind::Neutral),
        }
    };

    let hidden = matches!(entity_ref.get::<Visibility>(), Some(Visibility::Hidden));

    if filter_lower.is_empty() || name.to_lowercase().contains(filter_lower) {
        out.push(HierarchyItem {
            entity,
            name,
            kind,
            depth,
            tag,
            badge_kind,
            hidden,
            has_children,
        });
    }

    if has_children && (!collapsed.contains(&entity) || !filter_lower.is_empty()) {
        for child in children_vec {
            collect_subtree(
                world,
                child,
                depth + 1,
                scene_set,
                collapsed,
                filter_lower,
                out,
            );
        }
    }
}

/// Computes the world-space [`Transform`] of `entity` by walking its `ChildOf` chain.
/// Works even before Bevy's `TransformPropagate` system runs (e.g. on the same frame as an edit or in headless tests).
pub fn compute_world_transform(world: &World, entity: Entity) -> Transform {
    let mut chain = Vec::new();
    let mut current = Some(entity);
    let mut visited = EntityHashSet::default();
    while let Some(e) = current {
        if !visited.insert(e) {
            break;
        }
        if let Some(tr) = world.get::<Transform>(e) {
            chain.push(*tr);
        }
        current = world.get::<ChildOf>(e).map(ChildOf::parent);
    }
    let mut acc = Transform::IDENTITY;
    for tr in chain.into_iter().rev() {
        acc = acc * tr;
    }
    acc
}

/// Renames `entity` via a `SetComponentField` command on `Name` (or inserts `Name` if absent).
pub fn rename_entity_command(world: &mut World, entity: Entity, new_name: &str) {
    let entity = resolve_entity(world, entity);
    let trimmed = new_name.trim();
    if trimmed.is_empty() {
        return;
    }
    if let Some(existing_name) = world.get::<Name>(entity)
        && existing_name.as_str() == trimmed
    {
        return;
    }
    if let Ok(ron_str) = ron::to_string(trimmed) {
        set_or_insert(world, entity, Name::new(trimmed.to_owned()), ron_str);
    }
}

/// Sets a whole component through `SetComponentField`, or adds it with an
/// undoable `InsertComponent` when the entity does not have it yet.
fn set_or_insert<T: Component + Reflect + TypePath>(
    world: &mut World,
    entity: Entity,
    value: T,
    ron_value: impl Into<String>,
) {
    let result = if world.get::<T>(entity).is_some() {
        History::execute(world, SetComponentField::typed::<T>(entity, "", ron_value))
    } else {
        History::execute(world, cruft_core::InsertComponent::typed(entity, value))
    };
    if result.is_ok() {
        world.resource_mut::<History>().finish_group();
    }
}

/// Drains `ToastQueue` (`feedback::ToastQueue`) into `EditorUiState::active_toast`.
pub fn drain_toast_queue(world: &mut World, state: &mut EditorUiState) {
    let Some(mut queue) = world.get_resource_mut::<ToastQueue>() else {
        return;
    };
    for msg in queue.drain() {
        let kind = classify_toast_kind(&msg);
        state.show_toast(msg, kind);
    }
}

/// MCP connection status shown in the top bar and the Agent header.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentStatusView {
    /// Client name ("claude-code", …) or "MCP" when no client is known.
    pub label: String,
    /// "http://127.0.0.1:7457/mcp · 21 outils", or why the server is down.
    pub detail: String,
    pub color: Color32,
    pub connected: bool,
}

impl Default for AgentStatusView {
    fn default() -> Self {
        Self {
            label: "MCP".into(),
            detail: "serveur MCP désactivé".into(),
            color: ACCENT_RED,
            connected: false,
        }
    }
}

fn format_age(age: std::time::Duration) -> String {
    let secs = age.as_secs();
    match secs {
        0..=4 => "maint.".into(),
        5..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min", secs / 60),
        _ => format!("{} h", secs / 3600),
    }
}

/// Rebuilds the Agent view models (journal, ghosts, card, status) from the core
/// `Proposals` and the live MCP `AgentActivity`. Called once per UI frame.
pub fn sync_agent_state(world: &mut World, state: &mut EditorUiState) {
    world.init_resource::<cruft_core::Proposals>();
    world.init_resource::<cruft_core::AgentActivity>();
    world.init_resource::<AgentMode>();
    state.agent_mode = *world.resource::<AgentMode>();

    let proposals = world.resource::<cruft_core::Proposals>();
    let activity = world.resource::<cruft_core::AgentActivity>();

    // Journal: one entry per proposal, with the MCP calls made since the previous one.
    let mut log = Vec::with_capacity(proposals.items().len());
    let mut previous: Option<std::time::Instant> = None;
    for proposal in proposals.items() {
        let calls: Vec<_> = activity
            .calls
            .iter()
            .filter(|c| c.at <= proposal.created && previous.is_none_or(|p| c.at > p))
            .collect();
        let mut names: Vec<&str> = Vec::new();
        for call in &calls {
            if !names.contains(&call.tool.as_str()) {
                names.push(&call.tool);
            }
        }
        let total: std::time::Duration = calls.iter().map(|c| c.duration).sum();
        let tools = if names.is_empty() {
            proposal.source.clone()
        } else {
            format!("{} · {} ms", names.join(" · "), total.as_millis().max(1))
        };
        log.push(AgentLogEntry {
            time: format_age(proposal.created.elapsed()),
            title: proposal.title.clone(),
            status: match proposal.status {
                cruft_core::ProposalStatus::Pending => ProposalState::Pending,
                cruft_core::ProposalStatus::Applied => ProposalState::Applied,
                cruft_core::ProposalStatus::Rejected => ProposalState::Rejected,
            },
            diff_lines: proposal
                .changes
                .iter()
                .map(|change| {
                    let (op, text) = change.diff(world);
                    AgentDiffLine {
                        op: DiffOp::from_core(op),
                        text,
                    }
                })
                .collect(),
            tools,
        });
        previous = Some(proposal.created);
    }
    state.agent_log = log;

    // Ghosts + card for the most recent pending proposal.
    match proposals.pending() {
        Some(pending) => {
            state.proposal = ProposalState::Pending;
            state.pending_proposal_id = Some(pending.id);
            state.pending_ghosts = pending
                .changes
                .iter()
                .filter_map(|change| change.ghost(world))
                .map(|ghost| GhostEntity {
                    name: ghost.label,
                    position: ghost.transform.translation,
                    scale: ghost.transform.scale,
                    op: DiffOp::from_core(ghost.op),
                })
                .collect();
        }
        None => {
            state.proposal = proposals
                .items()
                .last()
                .map_or(ProposalState::Applied, |p| match p.status {
                    cruft_core::ProposalStatus::Rejected => ProposalState::Rejected,
                    _ => ProposalState::Applied,
                });
            state.pending_proposal_id = None;
            state.pending_ghosts.clear();
        }
    }

    // Connection status.
    let connected = activity.connected();
    state.agent_status = AgentStatusView {
        label: activity.client.clone().unwrap_or_else(|| "MCP".into()),
        detail: match (&activity.endpoint, activity.server_running) {
            (Some(endpoint), true) => format!("{endpoint} · {} outils", activity.tool_count),
            (Some(_), false) => "serveur en démarrage ou en échec (voir Console)".into(),
            (None, _) => "serveur MCP désactivé".into(),
        },
        color: if connected {
            ACCENT_GREEN
        } else if activity.server_running {
            ACCENT_YELLOW
        } else {
            ACCENT_RED
        },
        connected,
    };
}

/// Applies the pending proposal as ONE undoable command (`cruft_core::proposals`).
pub fn apply_pending_proposal(world: &mut World, state: &mut EditorUiState) {
    if let Err(error) = cruft_core::proposals::apply_proposal(world, state.pending_proposal_id) {
        world.trigger(EditorEvent::Toast(error.to_string()));
    }
    sync_agent_state(world, state);
}

/// Rejects the pending proposal; the scene is not touched.
pub fn reject_pending_proposal(world: &mut World, state: &mut EditorUiState) {
    if let Err(error) = cruft_core::proposals::reject_proposal(world, state.pending_proposal_id) {
        world.trigger(EditorEvent::Toast(error.to_string()));
    }
    sync_agent_state(world, state);
}

/// V1 has no embedded LLM client: the instruction (with its @context) is copied
/// to the clipboard so it can be pasted into the connected MCP client.
pub fn submit_agent_instruction(world: &mut World, state: &mut EditorUiState, raw_prompt: &str) {
    let prompt = raw_prompt.trim();
    if prompt.is_empty() {
        return;
    }
    let mut text = format!("Dans l'éditeur Cruft (outils MCP « cruft ») : {prompt}");
    if state.context_include_selected
        && let Some(entity) = world.resource::<Selection>().primary()
    {
        let entity = resolve_entity(world, entity);
        let name = world
            .get::<Name>(entity)
            .map_or_else(|| "sans nom".to_owned(), |n| n.as_str().to_owned());
        text.push_str(&format!(
            "\nContexte @entité : « {name} » (id {}).",
            cruft_core::json::entity_id(entity)
        ));
    }
    if state.context_include_scene {
        text.push_str("\nContexte @scène : commence par list_entities.");
    }
    if state.agent_mode == AgentMode::Propose {
        text.push_str(
            "\nPropose les changements avec propose_changes (je validerai dans l'éditeur).",
        );
    }
    state.clipboard_request = Some(text);
    let target = if state.agent_status.connected {
        state.agent_status.label.clone()
    } else {
        "ton client MCP".to_owned()
    };
    world.trigger(EditorEvent::Toast(format!(
        "Consigne copiée : colle-la dans {target} (pas d'assistant intégré en V1)"
    )));
}

/// Computes a case-insensitive fuzzy match score for `query` against `candidate`.
/// Returns `None` if the characters of `query` do not appear in order in `candidate`.
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    let c = candidate.to_lowercase();
    if c == q {
        return Some(1000);
    }
    if c.starts_with(&q) {
        return Some(500 - (c.len() as i32 - q.len() as i32));
    }
    if let Some(pos) = c.find(&q) {
        return Some(300 - pos as i32);
    }

    let c_chars: Vec<char> = c.chars().collect();
    let mut score = 0i32;
    let mut c_idx = 0usize;
    let mut prev_match_idx: Option<usize> = None;

    for q_ch in q.chars() {
        if q_ch.is_whitespace() {
            continue;
        }
        let mut found = None;
        while c_idx < c_chars.len() {
            if c_chars[c_idx] == q_ch {
                found = Some(c_idx);
                c_idx += 1;
                break;
            }
            c_idx += 1;
        }
        let match_idx = found?;
        score += 10;
        if let Some(prev) = prev_match_idx {
            let gap = match_idx - prev - 1;
            if gap > 4 {
                return None;
            }
            if gap == 0 {
                score += 15;
            } else {
                score -= (gap as i32) * 3;
            }
        } else if match_idx == 0 {
            score += 25;
        }
        if match_idx == 0 || matches!(c_chars[match_idx - 1], ' ' | '_' | '-' | '.' | ':' | '/') {
            score += 20;
        }
        prev_match_idx = Some(match_idx);
    }

    (score > 0).then_some(score)
}

fn classify_toast_kind(msg: &str) -> ToastKind {
    let lower = msg.to_lowercase();
    if lower.contains("erreur")
        || lower.contains("impossible")
        || lower.contains("absent")
        || lower.contains("cycle")
        || lower.contains("rejet")
        || lower.contains("invalide")
    {
        ToastKind::Error
    } else if lower.contains("agent") || lower.contains("attente") || lower.contains("proposition")
    {
        ToastKind::Warn
    } else if lower.contains("mode jeu lancé") {
        ToastKind::Play
    } else {
        ToastKind::Ok
    }
}

// ---------------------------------------------------------------------------
// 3D Viewport Camera Math, Picking & Transform Gizmos
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct ViewportCameraView {
    eye: Vec3,
    view_from_world: Mat4,
    clip_from_view: Mat4,
    world_from_clip: Mat4,
}

fn active_scene_camera_view(world: &mut World, canvas_rect: Rect) -> Option<ViewportCameraView> {
    let mut query = world
        .query_filtered::<(Entity, &Transform, Option<&Projection>), With<EditorViewportCamera>>();
    let (cam_entity, _, proj_opt) = query.iter(world).next()?;
    let cam_world = compute_world_transform(world, cam_entity);
    let view_from_world = cam_world.to_matrix().inverse();

    let aspect = (canvas_rect.width().max(1.0)) / (canvas_rect.height().max(1.0));
    let clip_from_view = match proj_opt {
        Some(Projection::Perspective(p)) => {
            perspective_rh_matrix(p.fov, aspect, p.near.max(0.01), p.far.max(100.0))
        }
        Some(Projection::Orthographic(o)) => {
            let hw = canvas_rect.width() * 0.5 * o.scale;
            let hh = canvas_rect.height() * 0.5 * o.scale;
            orthographic_rh_matrix(-hw, hw, -hh, hh, o.near, o.far)
        }
        _ => perspective_rh_matrix(std::f32::consts::FRAC_PI_4, aspect, 0.1, 1000.0),
    };

    let clip_from_world = clip_from_view * view_from_world;
    let world_from_clip = clip_from_world.inverse();

    Some(ViewportCameraView {
        eye: cam_world.translation,
        view_from_world,
        clip_from_view,
        world_from_clip,
    })
}

impl ViewportCameraView {
    fn world_to_canvas(&self, canvas_rect: Rect, world_pos: Vec3) -> Option<Pos2> {
        let view_pos = self.view_from_world.transform_point3(world_pos);
        if view_pos.z >= -0.02 {
            return None;
        }
        let clip = self.clip_from_view * view_pos.extend(1.0);
        if clip.w.abs() <= 1e-5 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        let x = canvas_rect.left() + (ndc.x * 0.5 + 0.5) * canvas_rect.width();
        let y = canvas_rect.top() + (0.5 - ndc.y * 0.5) * canvas_rect.height();
        Some(Pos2::new(x, y))
    }

    fn canvas_to_ray(&self, canvas_rect: Rect, screen_pos: Pos2) -> Option<Ray3d> {
        if canvas_rect.width() <= 1.0 || canvas_rect.height() <= 1.0 {
            return None;
        }
        let u = (screen_pos.x - canvas_rect.left()) / canvas_rect.width();
        let v = (screen_pos.y - canvas_rect.top()) / canvas_rect.height();
        let ndc_x = u * 2.0 - 1.0;
        let ndc_y = 1.0 - v * 2.0;

        let near = self
            .world_from_clip
            .project_point3(Vec3::new(ndc_x, ndc_y, -1.0));
        let far = self
            .world_from_clip
            .project_point3(Vec3::new(ndc_x, ndc_y, 0.5));
        let dir = Dir3::new(far - near).ok()?;
        Some(Ray3d::new(near, dir))
    }
}

/// Casts a 3D picking ray from `screen_pos` inside `canvas_rect` against all scene entities in `world`.
/// Uses `bevy_picking`'s `ray_mesh_intersection` for `Mesh3d` entities and sphere intersection for lights/nodes.
pub fn pick_entity_in_viewport(
    world: &mut World,
    canvas_rect: Rect,
    screen_pos: Pos2,
) -> Option<Entity> {
    let cam_view = active_scene_camera_view(world, canvas_rect)?;
    let ray = cam_view.canvas_to_ray(canvas_rect, screen_pos)?;

    let candidates: Vec<(Entity, Option<Handle<Mesh>>)> = world
        .iter_entities()
        .filter(|e| {
            let id = e.id();
            is_scene_entity(world, id)
                && !e.contains::<Camera3d>()
                && !matches!(e.get::<Visibility>(), Some(Visibility::Hidden))
        })
        .map(|e| (e.id(), e.get::<Mesh3d>().map(|m| m.0.clone())))
        .collect();

    let mut closest: Option<(Entity, f32)> = None;

    for (entity, mesh_handle) in candidates {
        let world_tr = compute_world_transform(world, entity);
        let world_mat = world_tr.to_matrix();
        let world_affine = Affine3A::from_mat4(world_mat);

        let hit_dist = if let Some(handle) = mesh_handle {
            let mesh_hit = world
                .get_resource::<Assets<Mesh>>()
                .and_then(|meshes| meshes.get(&handle))
                .and_then(|mesh| {
                    ray_intersection_over_mesh(mesh, &world_affine, ray, Backfaces::Include)
                })
                .map(|hit| hit.distance);

            mesh_hit.or_else(|| ray_unit_box_distance(ray, &world_mat, 0.5))
        } else {
            ray_sphere_distance(ray, world_tr.translation, 0.45)
        };

        if let Some(dist) = hit_dist
            && dist >= 0.0
            && closest.is_none_or(|(_, best)| dist < best)
        {
            closest = Some((entity, dist));
        }
    }

    closest.map(|(entity, _)| entity)
}

fn perspective_rh_matrix(fov_y_radians: f32, aspect_ratio: f32, z_near: f32, z_far: f32) -> Mat4 {
    let f = 1.0 / (0.5 * fov_y_radians).tan();
    let nf = 1.0 / (z_near - z_far);
    Mat4::from_cols(
        Vec4::new(f / aspect_ratio, 0.0, 0.0, 0.0),
        Vec4::new(0.0, f, 0.0, 0.0),
        Vec4::new(0.0, 0.0, z_far * nf, -1.0),
        Vec4::new(0.0, 0.0, z_near * z_far * nf, 0.0),
    )
}

fn orthographic_rh_matrix(
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
    near: f32,
    far: f32,
) -> Mat4 {
    let rcp_width = 1.0 / (right - left);
    let rcp_height = 1.0 / (top - bottom);
    let r = 1.0 / (near - far);
    Mat4::from_cols(
        Vec4::new(2.0 * rcp_width, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 * rcp_height, 0.0, 0.0),
        Vec4::new(0.0, 0.0, r, 0.0),
        Vec4::new(
            -(left + right) * rcp_width,
            -(top + bottom) * rcp_height,
            near * r,
            1.0,
        ),
    )
}

fn ray_intersection_over_mesh(
    mesh: &Mesh,
    transform: &Affine3A,
    ray: Ray3d,
    cull: Backfaces,
) -> Option<RayMeshHit> {
    if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
        return None;
    }
    let positions = mesh
        .try_attribute(Mesh::ATTRIBUTE_POSITION)
        .ok()?
        .as_float3()?;
    let normals = mesh
        .try_attribute(Mesh::ATTRIBUTE_NORMAL)
        .ok()
        .and_then(|v| v.as_float3());
    let uvs = mesh
        .try_attribute(Mesh::ATTRIBUTE_UV_0)
        .ok()
        .and_then(|uvs| match uvs {
            VertexAttributeValues::Float32x2(uvs) => Some(uvs.as_slice()),
            _ => None,
        });

    match mesh.try_indices().ok() {
        Some(Indices::U16(indices)) => {
            ray_mesh_intersection(ray, transform, positions, normals, Some(indices), uvs, cull)
        }
        Some(Indices::U32(indices)) => {
            ray_mesh_intersection(ray, transform, positions, normals, Some(indices), uvs, cull)
        }
        None => ray_mesh_intersection::<u32>(ray, transform, positions, normals, None, uvs, cull),
    }
}

fn ray_sphere_distance(ray: Ray3d, center: Vec3, radius: f32) -> Option<f32> {
    let oc = ray.origin - center;
    let b = oc.dot(*ray.direction);
    let c = oc.length_squared() - radius * radius;
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let sqrt_d = disc.sqrt();
    let t1 = -b - sqrt_d;
    let t2 = -b + sqrt_d;
    if t1 >= 0.0 {
        Some(t1)
    } else if t2 >= 0.0 {
        Some(t2)
    } else {
        None
    }
}

fn ray_unit_box_distance(ray: Ray3d, world_from_local: &Mat4, half_ext: f32) -> Option<f32> {
    let local_from_world = world_from_local.inverse();
    let local_origin = local_from_world.transform_point3(ray.origin);
    let local_dir = local_from_world.transform_vector3(*ray.direction);
    let inv_dir = Vec3::new(1.0 / local_dir.x, 1.0 / local_dir.y, 1.0 / local_dir.z);
    let min = Vec3::splat(-half_ext);
    let max = Vec3::splat(half_ext);
    let t1 = (min - local_origin) * inv_dir;
    let t2 = (max - local_origin) * inv_dir;
    let tmin = t1.min(t2).max_element();
    let tmax = t1.max(t2).min_element();
    if tmax >= tmin.max(0.0) {
        Some(tmin.max(0.0))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Main Exclusive Egui Pass System
// ---------------------------------------------------------------------------

fn editor_ui_system(world: &mut World) {
    let Ok(mut egui_ctx_comp) = world
        .query_filtered::<&mut EguiContext, With<PrimaryEguiContext>>()
        .single_mut(world)
    else {
        return;
    };
    let ctx = egui_ctx_comp.get_mut().clone();

    world.init_resource::<EditorUiState>();
    world.init_resource::<ToastQueue>();
    world.init_resource::<Selection>();
    world.init_resource::<History>();

    let elapsed = world
        .get_resource::<Time<Real>>()
        .map_or(0.0, |t| t.elapsed_secs_f64());

    world.resource_scope(|world, mut state: Mut<EditorUiState>| {
        state.current_time = elapsed;
        let mode = world
            .get_resource::<State<EditorState>>()
            .map_or(EditorState::Editing, |s| *s.get());
        state.playing = mode != EditorState::Editing;
        state.paused = mode == EditorState::Paused;

        sync_agent_state(world, &mut state);
        if let Some(text) = state.clipboard_request.take() {
            ctx.copy_text(text);
        }

        // Drain any newly triggered EditorEvent::Toast events from ToastQueue
        drain_toast_queue(world, &mut state);

        // Expire active toast after 2.8s
        if let Some((_, expires_at)) = &state.active_toast
            && state.current_time >= *expires_at
        {
            state.active_toast = None;
            ctx.request_repaint();
        }

        sync_ui_zoom(&ctx, world, &mut state);

        if !state.theme_initialized {
            theme::apply_theme(&ctx);
            // Ctrl +/- / 0 are handled by `handle_shortcuts` (persisted setting).
            ctx.options_mut(|options| options.zoom_with_keyboard = false);
            state.theme_initialized = true;
            // egui installs newly supplied font families on the next pass.
            // Drawing with Inter-Bold in this pass would panic on first launch.
            ctx.request_repaint();
            return;
        }

        if state.playing || state.proposal == ProposalState::Pending || state.active_toast.is_some()
        {
            ctx.request_repaint();
        }

        // Global shortcuts (Ctrl+K, Ctrl+P, Ctrl+Z, Ctrl+Shift+Z / Ctrl+Y, W/E/R, Delete)
        handle_shortcuts(&ctx, world, &mut state);
        drain_toast_queue(world, &mut state);

        let viewport_texture_id = world
            .get_resource::<ViewportRenderTarget>()
            .zip(world.get_resource::<EguiUserTextures>())
            .and_then(|(vt, ut)| ut.image_id(&vt.image));

        let mut root_ui = Ui::new(
            ctx.clone(),
            "cruft_root_viewport".into(),
            egui::UiBuilder::new()
                .layer_id(egui::LayerId::background())
                .max_rect(ctx.viewport_rect()),
        );

        // 1. Top Bar (38px height)
        egui::Panel::top("cruft_top_bar")
            .exact_size(38.0)
            .show_separator_line(false)
            .frame(
                Frame::NONE
                    .fill(BG_TAB_BAR)
                    .inner_margin(Margin::symmetric(8, 0))
                    .stroke(Stroke::new(1.0, BORDER_PANEL)),
            )
            .show(&mut root_ui, |ui| {
                draw_top_bar(ui, world, &mut state);
            });

        // 2. Bottom Status Bar (24px height)
        egui::Panel::bottom("cruft_status_bar")
            .exact_size(24.0)
            .show_separator_line(false)
            .frame(
                Frame::NONE
                    .fill(BG_TAB_BAR)
                    .inner_margin(Margin::symmetric(10, 0))
                    .stroke(Stroke::new(1.0, BORDER_PANEL)),
            )
            .show(&mut root_ui, |ui| {
                draw_status_bar(ui, world, &state);
            });

        // 3. Central DockArea
        state.last_viewport_rect = None;
        let mut desired_viewport_pixels: Option<UVec2> = None;
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(BG_APP))
            .show(&mut root_ui, |ui| {
                let dock_style = theme::build_dock_style(ui.style());
                let mut dock_state =
                    std::mem::replace(&mut state.dock_state, DockState::new(Vec::new()));

                let mut viewer = CruftTabViewer {
                    world,
                    state: &mut state,
                    viewport_texture_id,
                    desired_viewport_pixels: &mut desired_viewport_pixels,
                };

                DockArea::new(&mut dock_state)
                    .id(Id::new("cruft_main_dock"))
                    .style(dock_style)
                    .show_close_buttons(false)
                    .show_add_buttons(false)
                    .show_leaf_close_all_buttons(false)
                    .show_leaf_collapse_buttons(false)
                    .tab_context_menus(false)
                    .show_inside(ui, &mut viewer);

                state.dock_state = dock_state;
            });

        // Resolve after ALL tabs: a press and release can share one input
        // frame, and egui_dock may draw the viewport before the asset source.
        finish_asset_drag(&ctx, world, &mut state);

        // Drain any toasts generated by UI interactions during the DockArea pass
        drain_toast_queue(world, &mut state);

        // Resize the Bevy RenderTarget texture whenever the Viewport panel size changes
        if let Some(new_px) = desired_viewport_pixels {
            let render_scale = world
                .get_resource::<DisplaySettings>()
                .map_or(1.0, |d| d.viewport_render_scale);
            let scaled = (new_px.as_vec2() * render_scale).round().as_uvec2();
            let clamped = UVec2::new(scaled.x.clamp(64, 8192), scaled.y.clamp(64, 8192));
            let current_size = world
                .get_resource::<ViewportRenderTarget>()
                .map(|vt| (vt.image.clone(), vt.size));
            if let Some((handle, old_size)) = current_size
                && old_size != clamped
            {
                if let Some(mut images) = world.get_resource_mut::<Assets<Image>>()
                    && let Some(mut image) = images.get_mut(&handle)
                {
                    image.resize(Extent3d {
                        width: clamped.x,
                        height: clamped.y,
                        depth_or_array_layers: 1,
                    });
                }
                if let Some(mut vt) = world.get_resource_mut::<ViewportRenderTarget>() {
                    vt.size = clamped;
                }
            }
        }

        // 4. Command Palette Modal Overlay (Ctrl+K)
        if state.palette_open {
            draw_command_palette(&ctx, world, &mut state);
            drain_toast_queue(world, &mut state);
        }
        if state.file_dialog.is_some() {
            draw_file_dialog(&ctx, world, &mut state);
            drain_toast_queue(world, &mut state);
        }
    });
}

fn handle_shortcuts(ctx: &egui::Context, world: &mut World, state: &mut EditorUiState) {
    let wants_kb = ctx.egui_wants_keyboard_input() || state.renaming_entity.is_some();
    let buffered = world
        .get_resource_mut::<shortcuts::Shortcuts>()
        .map(|mut shortcuts| std::mem::take(&mut *shortcuts))
        .unwrap_or_default();

    let mut do_undo = false;
    let mut do_redo = false;
    let mut do_delete = false;
    let mut do_apply_proposal = false;
    let mut do_save = false;
    let mut toggle_fullscreen = false;
    let mut zoom_step = 0;
    let mut zoom_reset = false;

    ctx.input_mut(|input| {
        if input.consume_key(egui::Modifiers::NONE, Key::F11)
            || input.consume_key(egui::Modifiers::ALT, Key::Enter)
        {
            toggle_fullscreen = true;
        }
        if input.consume_key(egui::Modifiers::CTRL, Key::Plus)
            || input.consume_key(egui::Modifiers::CTRL, Key::Equals)
            || input.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::Equals)
        {
            zoom_step = 1;
        }
        if input.consume_key(egui::Modifiers::CTRL, Key::Minus) {
            zoom_step = -1;
        }
        if input.consume_key(egui::Modifiers::CTRL, Key::Num0) {
            zoom_reset = true;
        }
        if input.consume_key(egui::Modifiers::CTRL, Key::K) || buffered.palette {
            state.palette_open = !state.palette_open;
            state.palette_selected_idx = 0;
        }
        if input.key_pressed(Key::Escape) && state.palette_open {
            state.palette_open = false;
        }
        if input.consume_key(egui::Modifiers::CTRL, Key::P) || buffered.play {
            toggle_play_mode(world, state);
        }
        if input.consume_key(egui::Modifiers::CTRL, Key::S) || buffered.save {
            do_save = true;
        }

        if !wants_kb {
            // Match event modifiers, not the final keyboard state: Control
            // can be released in the same frame as the shortcut key.
            if input.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::Z)
                || input.consume_key(egui::Modifiers::CTRL, Key::Y)
                || buffered.redo
            {
                do_redo = true;
            } else if input.consume_key(egui::Modifiers::CTRL, Key::Z) || buffered.undo {
                do_undo = true;
            }
        }

        if !wants_kb && !state.palette_open && !input.modifiers.any() {
            if input.key_pressed(Key::Q) {
                state.tool = TransformTool::Select;
            } else if input.key_pressed(Key::W) {
                state.tool = TransformTool::Move;
            } else if input.key_pressed(Key::E) {
                state.tool = TransformTool::Rotate;
            } else if input.key_pressed(Key::R) {
                state.tool = TransformTool::Scale;
            } else if input.key_pressed(Key::A) && state.proposal == ProposalState::Pending {
                do_apply_proposal = true;
            } else if input.key_pressed(Key::Delete) || input.key_pressed(Key::Backspace) {
                do_delete = true;
            }
        }
    });

    if toggle_fullscreen || zoom_step != 0 || zoom_reset {
        let current = ctx.zoom_factor();
        let mut settings = world.resource_mut::<DisplaySettings>();
        if toggle_fullscreen {
            settings.toggle_fullscreen();
        }
        if zoom_step != 0 {
            settings.step_ui_scale(current, zoom_step);
        }
        if zoom_reset {
            settings.ui_scale = None;
        }
    }
    if do_save {
        save_scene_action(world, state, None);
    }
    if do_apply_proposal {
        apply_pending_proposal(world, state);
    } else if do_undo {
        let _ = History::undo(world);
    } else if do_redo {
        let _ = History::redo(world);
    } else if do_delete {
        delete_selection(world);
    }
}

/// Deletes every selected entity as ONE history entry. Entities whose ancestor
/// is also selected are skipped (deleting the ancestor removes them).
pub fn delete_selection(world: &mut World) {
    let selected: Vec<Entity> = world
        .resource::<Selection>()
        .entities()
        .iter()
        .map(|&e| resolve_entity(world, e))
        .filter(|&e| world.get_entity(e).is_ok())
        .collect();
    let roots: Vec<Entity> = selected
        .iter()
        .copied()
        .filter(|&entity| {
            let mut ancestor = world.get::<ChildOf>(entity).map(ChildOf::parent);
            while let Some(current) = ancestor {
                if selected.contains(&current) {
                    return false;
                }
                ancestor = world.get::<ChildOf>(current).map(ChildOf::parent);
            }
            true
        })
        .collect();
    let result = match roots.as_slice() {
        [] => return,
        [single] => History::execute(world, DespawnEntity::new(*single)),
        many => {
            let mut command =
                cruft_core::CompositeCommand::new(format!("Supprimer {} entités", many.len()));
            for &entity in many {
                command.push(DespawnEntity::new(entity));
            }
            History::execute(world, command)
        }
    };
    if result.is_ok() {
        world.resource_mut::<History>().finish_group();
    }
}

fn toggle_play_mode(world: &mut World, state: &mut EditorUiState) {
    request_play_action(
        world,
        if state.playing {
            PlayAction::Stop
        } else {
            PlayAction::Play
        },
    );
}

// ---------------------------------------------------------------------------
// Top Bar & Status Bar
// ---------------------------------------------------------------------------

fn draw_top_bar(ui: &mut Ui, world: &mut World, state: &mut EditorUiState) {
    let bar_rect = ui.max_rect();

    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;

        let (logo_rect, _) = ui.allocate_exact_size(Vec2::new(78.0, 26.0), Sense::hover());
        paint_cruft_logo(ui.painter(), logo_rect);

        let div_x = ui.cursor().left();
        ui.painter().line_segment(
            [
                Pos2::new(div_x, bar_rect.center().y - 12.0),
                Pos2::new(div_x, bar_rect.center().y + 12.0),
            ],
            Stroke::new(1.0, BORDER_PANEL),
        );
        ui.add_space(4.0);

        ui.spacing_mut().item_spacing.x = 1.0;
        let file_resp = CruftButton::new("Fichier", "Fichier")
            .variant(ButtonVariant::Menu)
            .show(ui);
        draw_file_menu(&file_resp, world, state);
        for menu in ["Édition", "Créer", "Scène", "Agent"] {
            if CruftButton::new(menu, menu)
                .variant(ButtonVariant::Menu)
                .show(ui)
                .clicked()
            {
                world.trigger(EditorEvent::Toast(format!("Menu {menu}")));
            }
        }

        let window_resp = CruftButton::new("Fenêtre", "Fenêtre")
            .variant(ButtonVariant::Menu)
            .show(ui);
        draw_window_menu(&window_resp, world);

        ui.add_space(6.0);
        let is_dirty = state.saved_undo_len != Some(world.resource::<History>().undo_len());
        let scene_label = state
            .scene_path
            .clone()
            .unwrap_or_else(|| "sans titre".to_owned());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.label(
                RichText::new(scene_label)
                    .font(font_mono(11.0))
                    .color(TEXT_SECONDARY),
            );
            if is_dirty {
                ui.label(RichText::new("●").font(font_mono(11.0)).color(ACCENT_RED));
            }
        });

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;

            if draw_assistant_top_button(ui, state.current_time, &state.agent_status).clicked() {
                state.focus_tab(EditorTab::Assistant);
            }

            if draw_top_search_button(ui).clicked() {
                state.palette_open = true;
            }
        });
    });

    // Centered Play / Pause / Step segmented control
    let seg_size = Vec2::new(98.0, 28.0);
    let seg_rect = Rect::from_center_size(bar_rect.center(), seg_size);
    ui.scope_builder(egui::UiBuilder::new().max_rect(seg_rect), |ui| {
        Frame::NONE
            .fill(BG_FIELD)
            .stroke(Stroke::new(1.0, BORDER_PANEL))
            .corner_radius(CornerRadius::same(7))
            .inner_margin(Margin::same(2))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 1.0;

                    let play_label = if state.playing { "■" } else { "▶" };
                    if CruftButton::new("transport_play", play_label)
                        .variant(ButtonVariant::Play {
                            active: state.playing,
                        })
                        .tooltip("Play / Stop  Ctrl P")
                        .show(ui)
                        .clicked()
                    {
                        toggle_play_mode(world, state);
                    }

                    if CruftButton::new("transport_pause", "⏸")
                        .variant(ButtonVariant::Icon {
                            active: state.paused,
                        })
                        .min_size(Vec2::new(30.0, 24.0))
                        .tooltip("Pause")
                        .show(ui)
                        .clicked()
                    {
                        request_play_action(
                            world,
                            if state.paused {
                                PlayAction::Resume
                            } else {
                                PlayAction::Pause
                            },
                        );
                    }

                    ui.add_enabled_ui(false, |ui| {
                        CruftButton::new("transport_step", "⏭")
                            .variant(ButtonVariant::Play { active: false })
                            .tooltip("Pas à pas · à venir")
                            .show(ui);
                    });
                });
            });
    });
}

fn paint_cruft_logo(painter: &egui::Painter, rect: Rect) {
    let icon_rect = Rect::from_min_size(
        Pos2::new(rect.left(), rect.center().y - 9.0),
        Vec2::new(18.0, 18.0),
    );
    painter.rect_filled(
        Rect::from_min_size(
            Pos2::new(icon_rect.left() + 5.5, icon_rect.top() + 2.0),
            Vec2::new(6.0, 2.5),
        ),
        CornerRadius::same(1),
        TEXT_PRIMARY,
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(icon_rect.left() + 1.5, icon_rect.top() + 6.0),
            Pos2::new(icon_rect.right() - 1.0, icon_rect.top() + 6.0),
            Pos2::new(icon_rect.right() - 3.5, icon_rect.top() + 9.5),
            Pos2::new(icon_rect.left() + 5.5, icon_rect.top() + 9.5),
            Pos2::new(icon_rect.left() + 4.5, icon_rect.bottom() - 2.0),
            Pos2::new(icon_rect.left() + 2.5, icon_rect.bottom() - 2.0),
        ],
        LOGO_RED,
        Stroke::NONE,
    ));
    painter.rect_filled(
        Rect::from_min_size(
            Pos2::new(icon_rect.left() + 3.5, icon_rect.bottom() - 4.5),
            Vec2::new(8.5, 2.5),
        ),
        CornerRadius::same(1),
        LOGO_RED,
    );

    painter.text(
        Pos2::new(icon_rect.right() + 6.0, rect.center().y),
        Align2::LEFT_CENTER,
        "CRUFT",
        font_sans_bold(14.5),
        TEXT_PRIMARY,
    );
}

fn draw_top_search_button(ui: &mut Ui) -> egui::Response {
    let id = ui.make_persistent_id("top_search_ctrl_k");
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(260.0, 26.0), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let hover_t = ui
        .ctx()
        .animate_bool_with_time(id, resp.hovered(), theme::HOVER_ANIM_SECS);

    let bg = lerp_color(BG_APP, BG_FIELD_HOVER, hover_t);
    let border = lerp_color(BORDER_PANEL, BORDER_HOVER, hover_t);
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(RADIUS_WIDGET),
        bg,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );

    let lens_c = Pos2::new(rect.left() + 13.0, rect.center().y - 1.0);
    painter.circle_stroke(lens_c, 4.2, Stroke::new(1.5, TEXT_SECONDARY));
    painter.line_segment(
        [
            Pos2::new(lens_c.x + 3.0, lens_c.y + 3.0),
            Pos2::new(lens_c.x + 6.0, lens_c.y + 6.0),
        ],
        Stroke::new(1.5, TEXT_SECONDARY),
    );

    painter.text(
        Pos2::new(rect.left() + 26.0, rect.center().y),
        Align2::LEFT_CENTER,
        "Rechercher ou demander…",
        font_sans(11.5),
        TEXT_SECONDARY,
    );

    let kbd_rect = Rect::from_min_max(
        Pos2::new(rect.right() - 46.0, rect.center().y - 8.0),
        Pos2::new(rect.right() - 6.0, rect.center().y + 8.0),
    );
    paint_kbd_in_rect(painter, kbd_rect, "Ctrl K", false);

    resp
}

fn draw_assistant_top_button(
    ui: &mut Ui,
    time_secs: f64,
    status: &AgentStatusView,
) -> egui::Response {
    let id = ui.make_persistent_id("top_assistant_btn");
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(112.0, 26.0), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let hover_t = ui
        .ctx()
        .animate_bool_with_time(id, resp.hovered(), theme::HOVER_ANIM_SECS);

    let bg = lerp_color(BG_AGENT_BADGE, Color32::from_rgb(0x2E, 0x28, 0x16), hover_t);
    let border = lerp_color(BORDER_AGENT, ACCENT_YELLOW, hover_t * 0.6);

    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(RADIUS_WIDGET),
        bg,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );

    let pulse = ((time_secs * std::f64::consts::TAU / 1.4).sin() * 0.5 + 0.5) as f32;
    let dot_alpha = (0.35 + 0.65 * pulse).clamp(0.0, 1.0);
    painter.circle_filled(
        Pos2::new(rect.left() + 12.0, rect.center().y),
        3.0,
        status.color.gamma_multiply(dot_alpha),
    );

    painter.text(
        Pos2::new(rect.left() + 22.0, rect.center().y),
        Align2::LEFT_CENTER,
        status.label.as_str(),
        font_mono(11.0),
        TEXT_YELLOW,
    );

    let state_text = if status.connected {
        "connecté"
    } else {
        status.detail.as_str()
    };
    resp.on_hover_text(format!(
        "Ouvrir le panneau Assistant IA — {} ({state_text})",
        status.label
    ))
}

fn draw_status_bar(ui: &mut Ui, world: &World, state: &EditorUiState) {
    let scene_count = world
        .iter_entities()
        .filter(|e| is_scene_entity(world, e.id()))
        .count();
    let history = world.resource::<History>();

    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;

        let (status_txt, status_col) = if state.playing {
            ("● EN JEU", ACCENT_GREEN)
        } else {
            ("● PRÊT", TEXT_SECONDARY)
        };
        ui.label(
            RichText::new(status_txt)
                .font(font_mono(10.5))
                .color(status_col),
        );

        ui.label(
            RichText::new("bevy 0.20")
                .font(font_mono(10.5))
                .color(TEXT_SECONDARY),
        );
        ui.label(
            RichText::new(&state.display_label)
                .font(font_mono(10.5))
                .color(TEXT_SECONDARY),
        )
        .on_hover_text("Fenêtre › Affichage · F11 plein écran · Ctrl +/- échelle de l'interface");

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (spark_rect, _) = ui.allocate_exact_size(Vec2::new(56.0, 12.0), Sense::hover());
            let samples: [f32; 11] = [
                0.65, 0.55, 0.65, 0.48, 0.56, 0.22, 0.56, 0.48, 0.56, 0.48, 0.56,
            ];
            let pts: Vec<Pos2> = samples
                .iter()
                .enumerate()
                .map(|(i, &v)| {
                    Pos2::new(
                        spark_rect.left() + (i as f32 / 10.0) * spark_rect.width(),
                        spark_rect.top() + v * spark_rect.height(),
                    )
                })
                .collect();
            ui.painter()
                .add(egui::Shape::line(pts, Stroke::new(1.2, ACCENT_GREEN)));

            ui.label(
                RichText::new("6.9 ms · 144 fps")
                    .font(font_mono(10.5))
                    .color(TEXT_SECONDARY),
            );
        });

        ui.label(
            RichText::new(format!(
                "{scene_count} entités · undo: {} / redo: {}",
                history.undo_len(),
                history.redo_len()
            ))
            .font(font_mono(10.5))
            .color(TEXT_SECONDARY),
        );

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            // Hints are dropped when the bar is too narrow (small window / big UI).
            for hint in [
                "Ctrl Z / Ctrl Shift Z historique",
                "W E R gizmos · double-clic renommer",
            ] {
                let galley =
                    ui.painter()
                        .layout_no_wrap(hint.to_owned(), font_mono(10.5), TEXT_SECONDARY);
                if ui.available_width() < galley.size().x + 24.0 {
                    break;
                }
                ui.label(
                    RichText::new(hint)
                        .font(font_mono(10.5))
                        .color(TEXT_SECONDARY),
                );
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Dock TabViewer Implementation
// ---------------------------------------------------------------------------

struct CruftTabViewer<'a> {
    world: &'a mut World,
    state: &'a mut EditorUiState,
    viewport_texture_id: Option<egui::TextureId>,
    desired_viewport_pixels: &'a mut Option<UVec2>,
}

impl TabViewer for CruftTabViewer<'_> {
    type Tab = EditorTab;

    fn id(&mut self, tab: &mut Self::Tab) -> Id {
        Id::new(*tab)
    }

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        let label = match tab {
            EditorTab::Outliner => "Outliner",
            EditorTab::Layers => "Calques",
            EditorTab::Viewport => "Viewport",
            EditorTab::Assets => "Assets",
            EditorTab::Console => "Console",
            EditorTab::Timeline => "Timeline",
            EditorTab::Inspector => "Inspecteur",
            EditorTab::Assistant => {
                if self.state.proposal == ProposalState::Pending {
                    "Assistant IA · 1"
                } else {
                    "Assistant IA"
                }
            }
        };
        RichText::new(label).font(font_sans(11.5)).into()
    }

    fn scroll_bars(&self, _tab: &Self::Tab) -> [bool; 2] {
        [false, false]
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        let panel_rect = ui.max_rect();
        ui.painter().line_segment(
            [
                Pos2::new(panel_rect.left() + 1.0, panel_rect.top()),
                Pos2::new(panel_rect.right() - 1.0, panel_rect.top()),
            ],
            Stroke::new(1.0, RELIEF_TOP_HIGHLIGHT),
        );

        match tab {
            EditorTab::Outliner => draw_outliner_tab(ui, self.world, self.state),
            EditorTab::Layers => draw_layers_tab(ui, self.world),
            EditorTab::Viewport => draw_viewport_tab(
                ui,
                self.world,
                self.state,
                self.viewport_texture_id,
                self.desired_viewport_pixels,
            ),
            EditorTab::Assets => draw_assets_tab(ui, self.world, self.state),
            EditorTab::Console => draw_console_tab(ui, self.world),
            EditorTab::Timeline => draw_timeline_tab(ui),
            EditorTab::Inspector => draw_inspector_tab(ui, self.world, self.state),
            EditorTab::Assistant => draw_assistant_tab(ui, self.world, self.state),
        }
    }
}

// ---------------------------------------------------------------------------
// Left Panel: Outliner from World (Hierarchy, Double-Click Rename, Drag & Drop Reparent)
// ---------------------------------------------------------------------------

fn draw_outliner_tab(ui: &mut Ui, world: &mut World, state: &mut EditorUiState) {
    let full_rect = ui.available_rect_before_wrap();
    let footer_h = 26.0;
    let header_h = 36.0;

    // 1. Filter bar + "+" Add entity button (spawns via SpawnEntity EditorCommand)
    let header_rect = Rect::from_min_size(full_rect.min, Vec2::new(full_rect.width(), header_h));
    let mut spawn_requested = false;
    ui.scope_builder(egui::UiBuilder::new().max_rect(header_rect), |ui| {
        Frame::NONE.inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let add_btn_w = 24.0;
                let filter_w = (ui.available_width() - add_btn_w - 4.0).max(60.0);

                Frame::NONE
                    .fill(BG_FIELD)
                    .stroke(Stroke::new(1.0, BORDER_PANEL))
                    .corner_radius(CornerRadius::same(RADIUS_WIDGET))
                    .inner_margin(Margin::symmetric(7, 3))
                    .show(ui, |ui| {
                        ui.set_width(filter_w - 14.0);
                        ui.horizontal_centered(|ui| {
                            ui.label(
                                RichText::new("⌕")
                                    .font(font_mono(11.0))
                                    .color(TEXT_SECONDARY),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut state.outliner_filter)
                                    .frame(Frame::NONE)
                                    .font(font_sans(11.5))
                                    .hint_text(
                                        RichText::new("Filtrer…  t:Mesh")
                                            .font(font_sans(11.5))
                                            .color(TEXT_SECONDARY),
                                    ),
                            );
                        });
                    });

                if CruftButton::new("outliner_add_ent", "+")
                    .variant(ButtonVariant::Icon { active: false })
                    .tooltip("Nouvelle entité (SpawnEntity)")
                    .show(ui)
                    .clicked()
                {
                    spawn_requested = true;
                }
            });
        });
    });

    if spawn_requested {
        state.spawn_counter += 1;
        let name = format!("Entité.{}", state.spawn_counter);
        let cmd = SpawnEntity::new(&name).with_component(Transform::default());
        if History::execute(world, cmd).is_ok() {
            world.resource_mut::<History>().finish_group();
            // Select the newly spawned entity
            let newest = world
                .iter_entities()
                .filter(|e| e.get::<Name>().is_some_and(|n| n.as_str() == name))
                .map(|e| e.id())
                .max_by_key(|e| e.index());
            if let Some(ent) = newest {
                world.resource_mut::<Selection>().select_only(ent);
            }
        }
    }

    // 2. Collect real scene hierarchy from World
    let filter_lower = state.outliner_filter.trim().to_lowercase();
    let (items, total_scene_entities) =
        collect_hierarchy(world, &state.collapsed_entities, &filter_lower);

    let selected_entities: Vec<Entity> = world
        .resource::<Selection>()
        .entities()
        .iter()
        .map(|&e| resolve_entity(world, e))
        .collect();
    let primary_selected = world
        .resource::<Selection>()
        .primary()
        .map(|e| resolve_entity(world, e));

    let list_rect = Rect::from_min_max(
        Pos2::new(full_rect.left(), header_rect.bottom()),
        Pos2::new(full_rect.right(), full_rect.bottom() - footer_h),
    );

    let mut clicked_entity: Option<(Entity, bool)> = None;
    let mut double_clicked_entity: Option<(Entity, String)> = None;
    let mut rename_commit: Option<(Entity, String)> = None;
    let mut cancel_rename = false;
    let mut toggle_vis_entity: Option<(Entity, bool)> = None;
    let mut toggle_collapse_entity: Option<Entity> = None;
    let mut hovered_drop_entity: Option<Entity> = None;
    let mut hovered_detach_root = false;

    let ctrl_or_shift = ui.input(|i| i.modifiers.ctrl || i.modifiers.shift || i.modifiers.command);
    let pointer_released = ui.input(|i| i.pointer.any_released());
    let pointer_down = ui.input(|i| i.pointer.primary_down());

    ui.scope_builder(egui::UiBuilder::new().max_rect(list_rect), |ui| {
        ScrollArea::vertical()
            .id_salt("outliner_scroll")
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;

                // If currently dragging an entity, show a root detach drop target at the top
                if let Some(dragged) = state.dragged_hierarchy_entity {
                    let (root_drop_rect, _) = ui
                        .allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
                    let root_hovered = ui.rect_contains_pointer(root_drop_rect);
                    if root_hovered {
                        hovered_detach_root = true;
                    }
                    ui.painter().rect(
                        root_drop_rect.shrink2(Vec2::new(6.0, 2.0)),
                        CornerRadius::same(4),
                        if root_hovered {
                            BG_SELECTION
                        } else {
                            BG_TAB_BAR
                        },
                        Stroke::new(
                            1.0,
                            if root_hovered {
                                ACCENT_BLUE
                            } else {
                                BORDER_CONTROL
                            },
                        ),
                        StrokeKind::Inside,
                    );
                    let dragged_name = world.get::<Name>(dragged).map_or("Entité", |n| n.as_str());
                    ui.painter().text(
                        root_drop_rect.center(),
                        Align2::CENTER_CENTER,
                        format!("↑ Détacher « {dragged_name} » à la racine"),
                        font_mono(10.0),
                        if root_hovered {
                            Color32::WHITE
                        } else {
                            TEXT_SECONDARY
                        },
                    );
                }

                for item in &items {
                    let is_selected = selected_entities.contains(&item.entity)
                        || primary_selected == Some(item.entity);

                    if state.renaming_entity == Some(item.entity) {
                        if let Some(rename_res) = hierarchy_rename_row(
                            ui,
                            item.depth,
                            item.kind,
                            &mut state.rename_buffer,
                        ) {
                            match rename_res {
                                Some(new_name) => rename_commit = Some((item.entity, new_name)),
                                None => cancel_rename = true,
                            }
                        }
                        continue;
                    }

                    let row_id_str =
                        format!("ent_{}v{}", item.entity.index(), item.entity.generation());
                    let is_drop_target = state
                        .dragged_hierarchy_entity
                        .is_some_and(|d| d != item.entity)
                        && ui.rect_contains_pointer(Rect::from_min_size(
                            ui.cursor().min,
                            Vec2::new(ui.available_width(), 24.0),
                        ));

                    let out = hierarchy_row(
                        ui,
                        HierarchyRowProps {
                            id: &row_id_str,
                            name: &item.name,
                            kind: item.kind,
                            depth: item.depth,
                            tag: &item.tag,
                            badge_kind: item.badge_kind,
                            selected: is_selected,
                            hidden: item.hidden,
                            has_children: item.has_children,
                            collapsed: state.collapsed_entities.contains(&item.entity),
                            is_drop_target,
                        },
                    );

                    if out.drag_started {
                        state.dragged_hierarchy_entity = Some(item.entity);
                    }
                    if out.pointer_hovered
                        && state
                            .dragged_hierarchy_entity
                            .is_some_and(|d| d != item.entity)
                    {
                        hovered_drop_entity = Some(item.entity);
                    }
                    if out.double_clicked {
                        double_clicked_entity = Some((item.entity, item.name.clone()));
                    } else if out.clicked {
                        clicked_entity = Some((item.entity, ctrl_or_shift));
                    }
                    if out.toggled_visibility {
                        toggle_vis_entity = Some((item.entity, item.hidden));
                    }
                    if out.toggled_collapse {
                        toggle_collapse_entity = Some(item.entity);
                    }
                }

                // Pending ghost entities from the AI assistant proposal (dashed yellow contour)
                if state.proposal == ProposalState::Pending {
                    let dash_phase = (state.current_time * 14.0) as f32;
                    let mut focus_assistant = false;
                    for (ghost_idx, ghost) in state.pending_ghosts.iter().enumerate() {
                        if ghost.op != DiffOp::Add {
                            continue;
                        }
                        if !filter_lower.is_empty()
                            && !ghost.name.to_lowercase().contains(&filter_lower)
                        {
                            continue;
                        }
                        let row_top = ui.cursor().min;
                        let row_w = ui.available_width();
                        let gid = format!("ghost_prop_{ghost_idx}_{}", ghost.name);
                        let out = hierarchy_row(
                            ui,
                            HierarchyRowProps {
                                id: &gid,
                                name: &ghost.name,
                                kind: EntityKind::Ghost,
                                depth: 1,
                                tag: "proposé",
                                badge_kind: BadgeKind::Agent,
                                selected: false,
                                hidden: false,
                                has_children: false,
                                collapsed: false,
                                is_drop_target: false,
                            },
                        );
                        let ghost_rect =
                            Rect::from_min_size(row_top, Vec2::new(row_w, 24.0)).shrink(1.0);
                        paint_dashed_rect(
                            ui.painter(),
                            ghost_rect,
                            Stroke::new(1.2, ACCENT_YELLOW),
                            5.0,
                            4.0,
                            dash_phase,
                        );
                        if out.clicked {
                            focus_assistant = true;
                        }
                    }
                    if focus_assistant {
                        state.focus_tab(EditorTab::Assistant);
                    }
                }
            });
    });

    // Apply Outliner interactions to World & Selection
    if cancel_rename {
        state.renaming_entity = None;
    }
    if let Some((entity, new_name)) = rename_commit {
        state.renaming_entity = None;
        rename_entity_command(world, entity, &new_name);
    }
    if let Some((entity, current_name)) = double_clicked_entity {
        state.renaming_entity = Some(entity);
        state.rename_buffer = current_name;
        world.resource_mut::<Selection>().select_only(entity);
    } else if let Some((entity, multi)) = clicked_entity {
        let mut selection = world.resource_mut::<Selection>();
        if multi {
            if selection.entities().contains(&entity) {
                selection.remove(entity);
            } else {
                selection.add(entity);
            }
        } else {
            selection.select_only(entity);
        }
        state.focus_tab(EditorTab::Inspector);
    }

    if let Some(entity) = toggle_collapse_entity
        && !state.collapsed_entities.remove(&entity)
    {
        state.collapsed_entities.insert(entity);
    }

    if let Some((entity, currently_hidden)) = toggle_vis_entity {
        let (value, variant) = if currently_hidden {
            (Visibility::Inherited, "Inherited")
        } else {
            (Visibility::Hidden, "Hidden")
        };
        set_or_insert(world, entity, value, variant);
    }

    // Complete Drag & Drop -> Reparent command
    if let Some(dragged) = state.dragged_hierarchy_entity {
        if pointer_released || !pointer_down {
            if let Some(new_parent) = hovered_drop_entity {
                if new_parent != dragged {
                    let _ = History::execute(world, Reparent::new(dragged, Some(new_parent)));
                    world.resource_mut::<History>().finish_group();
                    state.collapsed_entities.remove(&new_parent);
                }
            } else if hovered_detach_root {
                let _ = History::execute(world, Reparent::new(dragged, None));
                world.resource_mut::<History>().finish_group();
            }
            state.dragged_hierarchy_entity = None;
        } else if let Some(pos) = ui.ctx().pointer_interact_pos() {
            // Draw floating drag badge near cursor
            let dragged_name = world.get::<Name>(dragged).map_or("Entité", |n| n.as_str());
            let tip_rect = Rect::from_min_size(pos + Vec2::new(12.0, 12.0), Vec2::new(140.0, 22.0));
            ui.painter().rect(
                tip_rect,
                CornerRadius::same(5),
                BG_PANEL,
                Stroke::new(1.0, ACCENT_BLUE),
                StrokeKind::Inside,
            );
            ui.painter().text(
                tip_rect.center(),
                Align2::CENTER_CENTER,
                format!("⇄ {dragged_name}"),
                font_sans(11.0),
                TEXT_PRIMARY,
            );
        }
    }

    // 3. Footer ("{N} entités" | "{M} sélectionnée(s)")
    let sel_count = world.resource::<Selection>().entities().len();
    let footer_rect = Rect::from_min_max(
        Pos2::new(full_rect.left(), full_rect.bottom() - footer_h),
        full_rect.max,
    );
    ui.painter().line_segment(
        [footer_rect.left_top(), footer_rect.right_top()],
        Stroke::new(1.0, BORDER_PANEL),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(footer_rect), |ui| {
        Frame::NONE
            .inner_margin(Margin::symmetric(8, 5))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(
                        RichText::new(format!("{total_scene_entities} entités"))
                            .font(font_mono(10.5))
                            .color(TEXT_MUTED),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let sel_txt = if sel_count > 1 {
                            format!("{sel_count} sélectionnées")
                        } else {
                            format!("{sel_count} sélectionnée")
                        };
                        ui.label(
                            RichText::new(sel_txt)
                                .font(font_mono(10.5))
                                .color(TEXT_MUTED),
                        );
                    });
                });
            });
    });
}

fn draw_layers_tab(ui: &mut Ui, world: &World) {
    let mesh_count = world
        .iter_entities()
        .filter(|e| is_scene_entity(world, e.id()) && e.contains::<Mesh3d>())
        .count();
    let light_count = world
        .iter_entities()
        .filter(|e| {
            is_scene_entity(world, e.id())
                && (e.contains::<PointLight>()
                    || e.contains::<DirectionalLight>()
                    || e.contains::<SpotLight>())
        })
        .count();
    let cam_count = world
        .iter_entities()
        .filter(|e| is_scene_entity(world, e.id()) && e.contains::<Camera3d>())
        .count();

    Frame::NONE.inner_margin(Margin::same(10)).show(ui, |ui| {
        for (layer, count, col) in [
            ("0 · Meshes", format!("{mesh_count} entités"), ACCENT_BLUE),
            (
                "1 · Lumières",
                format!("{light_count} entités"),
                ACCENT_YELLOW,
            ),
            ("2 · Caméras", format!("{cam_count} entités"), ACCENT_GREEN),
        ] {
            ui.horizontal(|ui| {
                let (dot_rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                ui.painter().circle_filled(dot_rect.center(), 3.5, col);
                ui.label(RichText::new(layer).font(font_sans(12.0)).color(TEXT_BODY));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(count).font(font_mono(10.5)).color(TEXT_MUTED));
                });
            });
            ui.add_space(4.0);
        }
    });
}

// ---------------------------------------------------------------------------
// Center Panel: Viewport (RenderTarget Texture + Picking + Transform Gizmos W/E/R)
// ---------------------------------------------------------------------------

fn draw_viewport_tab(
    ui: &mut Ui,
    world: &mut World,
    state: &mut EditorUiState,
    viewport_texture_id: Option<egui::TextureId>,
    desired_viewport_pixels: &mut Option<UVec2>,
) {
    let full_rect = ui.available_rect_before_wrap();
    let toolbar_h = 36.0;

    // 1. Viewport Toolbar
    let toolbar_rect = Rect::from_min_size(full_rect.min, Vec2::new(full_rect.width(), toolbar_h));
    ui.painter()
        .rect_filled(toolbar_rect, CornerRadius::ZERO, BG_TAB_BAR);
    ui.painter().line_segment(
        [toolbar_rect.left_bottom(), toolbar_rect.right_bottom()],
        Stroke::new(1.0, BORDER_PANEL),
    );

    ui.scope_builder(egui::UiBuilder::new().max_rect(toolbar_rect), |ui| {
        Frame::NONE
            .inner_margin(Margin::symmetric(6, 4))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;

                    draw_segmented_group(ui, |ui| {
                        for (tool, key, tip) in [
                            (TransformTool::Select, "Q", "Sélection  Q"),
                            (TransformTool::Move, "W", "Déplacer  W"),
                            (TransformTool::Rotate, "E", "Pivoter  E"),
                            (TransformTool::Scale, "R", "Échelle  R"),
                        ] {
                            if CruftButton::new(tip, key)
                                .variant(ButtonVariant::Icon {
                                    active: state.tool == tool,
                                })
                                .monospace(true)
                                .tooltip(tip)
                                .show(ui)
                                .clicked()
                            {
                                state.tool = tool;
                            }
                        }
                    });

                    draw_segmented_group(ui, |ui| {
                        for (sp, label) in
                            [(CoordSpace::Local, "Local"), (CoordSpace::Global, "Global")]
                        {
                            if CruftButton::new(label, label)
                                .variant(ButtonVariant::Icon {
                                    active: state.space == sp,
                                })
                                .show(ui)
                                .clicked()
                            {
                                state.space = sp;
                            }
                        }
                    });

                    let _ = CruftButton::new("vp_snap_btn", "⊞ 0.25 m")
                        .variant(ButtonVariant::Icon { active: false })
                        .monospace(true)
                        .bordered(true)
                        .tooltip("Pas de grille (G)")
                        .show(ui);

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;

                        let _ = CruftButton::new("vp_persp_btn", "Persp ▾")
                            .variant(ButtonVariant::Icon { active: false })
                            .bordered(true)
                            .show(ui);

                        draw_segmented_group(ui, |ui| {
                            for (sh, label) in [
                                (ShadingMode::Wireframe, "Fil"),
                                (ShadingMode::Solid, "Solide"),
                                (ShadingMode::Lit, "Éclairé"),
                            ] {
                                if CruftButton::new(label, label)
                                    .variant(ButtonVariant::Icon {
                                        active: state.shading == sh,
                                    })
                                    .show(ui)
                                    .clicked()
                                {
                                    state.shading = sh;
                                }
                            }
                        });
                    });
                });
            });
    });

    // 2. Viewport 3D Canvas Area
    let canvas_rect = Rect::from_min_max(
        Pos2::new(full_rect.left(), toolbar_rect.bottom()),
        full_rect.max,
    );
    state.last_viewport_rect = Some(canvas_rect);

    let ppp = ui.ctx().pixels_per_point();
    let px_w = (canvas_rect.width() * ppp).round().max(64.0) as u32;
    let px_h = (canvas_rect.height() * ppp).round().max(64.0) as u32;
    *desired_viewport_pixels = Some(UVec2::new(px_w, px_h));

    let canvas_resp = ui.allocate_rect(canvas_rect, Sense::click_and_drag());
    let corner = CornerRadius {
        nw: 0,
        ne: 0,
        sw: RADIUS_PANEL,
        se: RADIUS_PANEL,
    };

    if let Some(tex_id) = viewport_texture_id {
        let mut mesh = egui::Mesh::with_texture(tex_id);
        mesh.add_rect_with_uv(
            canvas_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        ui.painter().add(egui::Shape::mesh(mesh));
    } else {
        ui.painter()
            .rect_filled(canvas_rect, corner, Color32::from_rgb(0x0C, 0x1A, 0x22));
    }

    // Pending ghost entities rendered in the 3D viewport with animated yellow dashed contour
    if state.proposal == ProposalState::Pending {
        paint_pending_ghosts_in_viewport(ui.painter(), world, canvas_rect, state);
    }

    // 3. Interactive 3D Transform Gizmos (W / E / R) + 3D Viewport Picking
    let proposal_card_rect = if state.proposal == ProposalState::Pending {
        Some(Rect::from_min_size(
            Pos2::new(canvas_rect.right() - 300.0, canvas_rect.bottom() - 162.0),
            Vec2::new(290.0, 152.0),
        ))
    } else {
        None
    };

    let gizmo_consumed_pointer = !state.playing
        && interact_and_draw_transform_gizmo(ui, world, state, canvas_rect, &canvas_resp);

    // 3D Viewport Picking when clicking on the canvas (outside overlay cards and gizmo handles)
    if !gizmo_consumed_pointer
        && canvas_resp.clicked()
        && let Some(click_pos) = canvas_resp.interact_pointer_pos()
        && proposal_card_rect.is_none_or(|r| !r.contains(click_pos))
    {
        let multi = ui.input(|i| i.modifiers.ctrl || i.modifiers.shift || i.modifiers.command);
        if let Some(hit_entity) = pick_entity_in_viewport(world, canvas_rect, click_pos) {
            let mut selection = world.resource_mut::<Selection>();
            if multi {
                if selection.entities().contains(&hit_entity) {
                    selection.remove(hit_entity);
                } else {
                    selection.add(hit_entity);
                }
            } else {
                selection.select_only(hit_entity);
            }
            state.focus_tab(EditorTab::Inspector);
        } else if !multi {
            world.resource_mut::<Selection>().clear();
        }
    }

    // 4. Top-left Camera & Selection Info Overlay
    let primary_entity = world
        .resource::<Selection>()
        .primary()
        .map(|e| resolve_entity(world, e));
    let (sel_name, comp_count) = if let Some(ent) = primary_entity {
        let nm = world
            .get::<Name>(ent)
            .map(|n| n.as_str().to_owned())
            .unwrap_or_else(|| format!("Entity #{}", ent.index()));
        let cnt = component_views(world, ent).map_or(0, |v| v.len());
        (nm, cnt)
    } else {
        ("Aucune sélection".to_owned(), 0)
    };

    let info_pos = Pos2::new(canvas_rect.left() + 10.0, canvas_rect.top() + 10.0);
    let shade_label = match state.shading {
        ShadingMode::Wireframe => "Wireframe",
        ShadingMode::Solid => "Solid",
        ShadingMode::Lit => "Lit",
    };
    ui.painter().text(
        info_pos,
        Align2::LEFT_TOP,
        format!("Perspective · {shade_label}"),
        font_mono(10.5),
        TEXT_CONTROL,
    );
    ui.painter().text(
        Pos2::new(info_pos.x, info_pos.y + 16.0),
        Align2::LEFT_TOP,
        format!("{sel_name} · {comp_count} composants"),
        font_mono(10.5),
        TEXT_SECONDARY,
    );

    // 5. Top-right Orientation Gizmo Cube (`64x64`)
    let orient_rect = Rect::from_min_size(
        Pos2::new(canvas_rect.right() - 74.0, canvas_rect.top() + 10.0),
        Vec2::splat(64.0),
    );
    paint_orientation_gizmo(ui.painter(), orient_rect);

    // 6. Bottom-left Play Mode Badge
    if state.playing {
        let badge_rect = Rect::from_min_size(
            Pos2::new(canvas_rect.left() + 10.0, canvas_rect.bottom() - 34.0),
            Vec2::new(225.0, 24.0),
        );
        ui.painter()
            .rect_filled(badge_rect, CornerRadius::same(5), ACCENT_GREEN_FILL);
        let pulse = ((state.current_time * std::f64::consts::TAU / 1.4).sin() * 0.5 + 0.5) as f32;
        ui.painter().circle_filled(
            Pos2::new(badge_rect.left() + 12.0, badge_rect.center().y),
            3.0,
            Color32::WHITE.gamma_multiply(0.35 + 0.65 * pulse),
        );
        ui.painter().text(
            Pos2::new(badge_rect.left() + 22.0, badge_rect.center().y),
            Align2::LEFT_CENTER,
            "EN JEU · état restauré à l'arrêt",
            font_mono(10.5),
            Color32::WHITE,
        );
    }

    // 7. Bottom-right Pending Agent Proposal Floating Card
    if state.proposal == ProposalState::Pending {
        draw_viewport_proposal_card(ui, world, canvas_rect, state);
    }

    // 8. Bottom-center Animated Toast (clicking "Annuler Ctrl Z" triggers History::undo)
    let current_toast = state.active_toast.as_ref().map(|(d, _)| d.clone());
    let mut cache = state.last_toast_cache.take();
    let undo_clicked = animated_toast(ui, canvas_rect, current_toast.as_ref(), &mut cache);
    state.last_toast_cache = cache;
    if undo_clicked {
        state.active_toast = None;
        let _ = History::undo(world);
    }

    // 9. Play Mode Breathing Green Border (2px `#3FBF9F`) + Breathing Halo (2.4s cycle)
    let play_anim =
        ui.ctx()
            .animate_bool_with_time(Id::new("viewport_play_border_anim"), state.playing, 0.25);
    if play_anim > 0.01 {
        let breathe = ((state.current_time * std::f64::consts::TAU / 2.4).sin() * 0.5 + 0.5) as f32;
        let glow_alpha = ((45.0 + breathe * 55.0) * play_anim) as u8;
        let border_col = ACCENT_GREEN.gamma_multiply(play_anim);
        let vp_panel_rect = full_rect.expand(1.0);

        for (expand, factor) in [(6.0, 0.25), (3.0, 0.55), (1.0, 0.85)] {
            ui.painter().rect_stroke(
                vp_panel_rect.expand(expand),
                CornerRadius::same(RADIUS_PANEL + expand as u8),
                Stroke::new(
                    2.5,
                    Color32::from_rgba_unmultiplied(
                        0x3F,
                        0xBF,
                        0x9F,
                        (glow_alpha as f32 * factor) as u8,
                    ),
                ),
                StrokeKind::Outside,
            );
        }
        ui.painter().rect_stroke(
            vp_panel_rect,
            CornerRadius::same(RADIUS_PANEL),
            Stroke::new(2.0, border_col),
            StrokeKind::Inside,
        );
    }
}

/// Projects, draws, and handles pointer drag for the 3D Transform Gizmo (`W` Move, `E` Rotate, `R` Scale)
/// on the currently selected entity. Every drag step executes `SetComponentField::typed::<Transform>`.
fn interact_and_draw_transform_gizmo(
    ui: &mut Ui,
    world: &mut World,
    state: &mut EditorUiState,
    canvas_rect: Rect,
    canvas_resp: &egui::Response,
) -> bool {
    let Some(selected) = world
        .resource::<Selection>()
        .primary()
        .map(|e| resolve_entity(world, e))
    else {
        state.active_gizmo_drag = None;
        state.hovered_gizmo_axis = None;
        return false;
    };

    let Some(local_tr) = world.get::<Transform>(selected).copied() else {
        state.active_gizmo_drag = None;
        state.hovered_gizmo_axis = None;
        return false;
    };

    let Some(cam_view) = active_scene_camera_view(world, canvas_rect) else {
        return false;
    };

    let world_tr = compute_world_transform(world, selected);
    let origin_3d = world_tr.translation;
    let Some(origin_2d) = cam_view.world_to_canvas(canvas_rect, origin_3d) else {
        return false;
    };

    // Selection bounding brackets around the selected entity in the viewport
    let sel_box = Rect::from_center_size(origin_2d, Vec2::new(44.0, 44.0));
    ui.painter().rect_stroke(
        sel_box,
        CornerRadius::same(6),
        Stroke::new(1.5, ACCENT_BLUE.gamma_multiply(0.75)),
        StrokeKind::Outside,
    );

    if state.tool == TransformTool::Select {
        state.active_gizmo_drag = None;
        state.hovered_gizmo_axis = None;
        return false;
    }

    let cam_dist = cam_view.eye.distance(origin_3d).max(0.5);
    let gizmo_world_len = (cam_dist * 0.18).max(0.4);

    let use_local = state.space == CoordSpace::Local || state.tool == TransformTool::Scale;
    let basis_rot = if use_local {
        world_tr.rotation
    } else {
        Quat::IDENTITY
    };
    let axes_3d = [
        basis_rot * Vec3::X,
        basis_rot * Vec3::Y,
        basis_rot * Vec3::Z,
    ];
    let axis_colors = [AXIS_X, AXIS_Y, AXIS_Z];

    let mut ends_2d = [origin_2d; 3];
    for i in 0..3 {
        let tip_3d = origin_3d + axes_3d[i] * gizmo_world_len;
        ends_2d[i] = cam_view
            .world_to_canvas(canvas_rect, tip_3d)
            .unwrap_or(origin_2d + Vec2::new(60.0, 0.0));
    }

    // Build projected 3D rings for Rotate mode
    let mut ring_points: [Vec<Pos2>; 3] = Default::default();
    if state.tool == TransformTool::Rotate {
        let ring_r = gizmo_world_len * 0.85;
        let tangents = [
            (basis_rot * Vec3::Y, basis_rot * Vec3::Z),
            (basis_rot * Vec3::Z, basis_rot * Vec3::X),
            (basis_rot * Vec3::X, basis_rot * Vec3::Y),
        ];
        for (i, (u, v)) in tangents.into_iter().enumerate() {
            for step in 0..=28 {
                let angle = (step as f32 / 28.0) * std::f32::consts::TAU;
                let pt_3d = origin_3d + (u * angle.cos() + v * angle.sin()) * ring_r;
                if let Some(pt_2d) = cam_view.world_to_canvas(canvas_rect, pt_3d) {
                    ring_points[i].push(pt_2d);
                }
            }
        }
    }

    // Hit-test pointer against gizmo handles
    let pointer_pos = ui.ctx().pointer_interact_pos();
    let mut hovered_axis = None;
    if let Some(pos) = pointer_pos
        && canvas_rect.contains(pos)
    {
        let mut best_dist = 12.0_f32;
        for i in 0..3 {
            let d = match state.tool {
                TransformTool::Move | TransformTool::Scale => {
                    dist_point_to_segment(pos, origin_2d, ends_2d[i])
                }
                TransformTool::Rotate => dist_point_to_polyline(pos, &ring_points[i]),
                TransformTool::Select => f32::INFINITY,
            };
            if d < best_dist {
                best_dist = d;
                hovered_axis = Some(i);
            }
        }
    }
    state.hovered_gizmo_axis = hovered_axis;

    if hovered_axis.is_some() || state.active_gizmo_drag.is_some() {
        ui.ctx()
            .set_cursor_icon(if state.active_gizmo_drag.is_some() {
                CursorIcon::Grabbing
            } else {
                CursorIcon::Grab
            });
    }

    let mut consumed = false;
    if canvas_resp.drag_started()
        && let Some(axis) = hovered_axis
    {
        state.active_gizmo_drag = Some(ActiveGizmoDrag {
            entity: selected,
            tool: state.tool,
            axis,
        });
        consumed = true;
    }

    if let Some(drag) = state.active_gizmo_drag {
        consumed = true;
        if canvas_resp.dragged() {
            let delta_2d = canvas_resp.drag_delta();
            if delta_2d.length_sq() > f32::EPSILON {
                apply_gizmo_drag_step(
                    world,
                    drag,
                    local_tr,
                    origin_2d,
                    ends_2d[drag.axis],
                    gizmo_world_len,
                    delta_2d,
                );
            }
        }
        if canvas_resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
            state.active_gizmo_drag = None;
            world.resource_mut::<History>().finish_group();
        }
    }

    // Paint the 3D gizmo handles
    let painter = ui.painter();
    let active_axis = state
        .active_gizmo_drag
        .map(|d| d.axis)
        .or(state.hovered_gizmo_axis);

    match state.tool {
        TransformTool::Select => {}
        TransformTool::Move => {
            for i in 0..3 {
                let col = if active_axis == Some(i) {
                    Color32::WHITE
                } else {
                    axis_colors[i]
                };
                let tip = ends_2d[i];
                painter.line_segment([origin_2d, tip], Stroke::new(2.5, col));
                let dir = (tip - origin_2d).normalized();
                let perp = Vec2::new(-dir.y, dir.x);
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        tip + dir * 4.0,
                        tip - dir * 8.0 + perp * 4.5,
                        tip - dir * 8.0 - perp * 4.5,
                    ],
                    col,
                    Stroke::NONE,
                ));
            }
        }
        TransformTool::Rotate => {
            for (i, pts) in ring_points.into_iter().enumerate() {
                if pts.len() >= 2 {
                    let col = if active_axis == Some(i) {
                        Color32::WHITE
                    } else {
                        axis_colors[i]
                    };
                    painter.add(egui::Shape::line(pts, Stroke::new(2.4, col)));
                }
            }
        }
        TransformTool::Scale => {
            for i in 0..3 {
                let col = if active_axis == Some(i) {
                    Color32::WHITE
                } else {
                    axis_colors[i]
                };
                let tip = ends_2d[i];
                painter.line_segment([origin_2d, tip], Stroke::new(2.5, col));
                painter.rect_filled(
                    Rect::from_center_size(tip, Vec2::splat(9.0)),
                    CornerRadius::same(1),
                    col,
                );
            }
        }
    }

    consumed || hovered_axis.is_some()
}

/// Applies one incremental step of a gizmo drag via `SetComponentField::typed::<Transform>`.
pub fn apply_gizmo_drag_step(
    world: &mut World,
    drag: ActiveGizmoDrag,
    local_tr: Transform,
    origin_2d: Pos2,
    axis_end_2d: Pos2,
    gizmo_world_len: f32,
    delta_2d: Vec2,
) {
    let axis_names = ["x", "y", "z"];
    let axis_name = axis_names[drag.axis.min(2)];
    let screen_vec = axis_end_2d - origin_2d;
    let screen_len = screen_vec.length().max(8.0);
    let screen_dir = screen_vec / screen_len;
    let projected_px = delta_2d.dot(screen_dir);
    let world_delta = (projected_px / screen_len) * gizmo_world_len;

    match drag.tool {
        TransformTool::Move => {
            let new_val = local_tr.translation[drag.axis] + world_delta;
            let path = format!("translation.{axis_name}");
            let _ = History::execute(
                world,
                SetComponentField::typed::<Transform>(
                    drag.entity,
                    path,
                    format_float_ron(new_val as f64),
                ),
            );
        }
        TransformTool::Scale => {
            let new_val = (local_tr.scale[drag.axis] + world_delta).max(0.01);
            let path = format!("scale.{axis_name}");
            let _ = History::execute(
                world,
                SetComponentField::typed::<Transform>(
                    drag.entity,
                    path,
                    format_float_ron(new_val as f64),
                ),
            );
        }
        TransformTool::Rotate => {
            let angle_rad = (delta_2d.x - delta_2d.y) * 0.015;
            let axis_vec = [Vec3::X, Vec3::Y, Vec3::Z][drag.axis.min(2)];
            let new_rot =
                (local_tr.rotation * Quat::from_axis_angle(axis_vec, angle_rad)).normalize();
            if let Some(ron_rot) = serialize_quat_ron(world, new_rot) {
                let _ = History::execute(
                    world,
                    SetComponentField::typed::<Transform>(drag.entity, "rotation", ron_rot),
                );
            }
        }
        TransformTool::Select => {}
    }
}

fn serialize_quat_ron(world: &World, quat: Quat) -> Option<String> {
    let registry = world.get_resource::<AppTypeRegistry>()?.read();
    ron::to_string(&TypedReflectSerializer::new(
        quat.as_partial_reflect(),
        &registry,
    ))
    .ok()
}

fn dist_point_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_sq();
    if len_sq <= 1e-4 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

fn dist_point_to_polyline(p: Pos2, pts: &[Pos2]) -> f32 {
    pts.windows(2)
        .map(|w| dist_point_to_segment(p, w[0], w[1]))
        .fold(f32::INFINITY, f32::min)
}

fn draw_segmented_group(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui)) {
    Frame::NONE
        .fill(BG_FIELD)
        .stroke(Stroke::new(1.0, BORDER_PANEL))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 1.0;
                add_contents(ui);
            });
        });
}

fn paint_orientation_gizmo(painter: &egui::Painter, rect: Rect) {
    let c = rect.center();
    painter.circle_filled(
        c,
        30.0,
        Color32::from_rgba_unmultiplied(0x09, 0x0D, 0x10, 145),
    );

    let y_pos = Pos2::new(c.x, c.y - 22.0);
    let x_pos = Pos2::new(c.x + 20.0, c.y + 10.0);
    let z_pos = Pos2::new(c.x - 20.0, c.y + 10.0);

    for (pos, col, label) in [
        (y_pos, AXIS_Y, "Y"),
        (x_pos, AXIS_X, "X"),
        (z_pos, AXIS_Z, "Z"),
    ] {
        painter.line_segment([c, pos], Stroke::new(2.0, col));
        painter.circle_filled(pos, 6.5, col);
        painter.text(pos, Align2::CENTER_CENTER, label, font_mono(8.5), BG_APP);
    }
}

fn paint_pending_ghosts_in_viewport(
    painter: &egui::Painter,
    world: &mut World,
    canvas_rect: Rect,
    state: &EditorUiState,
) {
    let phase = (state.current_time * 16.0) as f32;
    let cam_view = active_scene_camera_view(world, canvas_rect);
    let stroke = Stroke::new(1.6, ACCENT_YELLOW);
    let fill = Color32::from_rgba_unmultiplied(0xFF, 0xC8, 0x32, 24);

    for (idx, ghost) in state.pending_ghosts.iter().enumerate() {
        let projected_box = cam_view.and_then(|cv| {
            let hx = ghost.scale.x * 0.5;
            let hy = ghost.scale.y * 0.5;
            let hz = ghost.scale.z * 0.5;
            let corners_3d = [
                ghost.position + Vec3::new(-hx, -hy, -hz),
                ghost.position + Vec3::new(hx, -hy, -hz),
                ghost.position + Vec3::new(hx, hy, -hz),
                ghost.position + Vec3::new(-hx, hy, -hz),
                ghost.position + Vec3::new(-hx, -hy, hz),
                ghost.position + Vec3::new(hx, -hy, hz),
                ghost.position + Vec3::new(hx, hy, hz),
                ghost.position + Vec3::new(-hx, hy, hz),
            ];
            let mut min_pt = Pos2::new(f32::INFINITY, f32::INFINITY);
            let mut max_pt = Pos2::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
            for pt_3d in corners_3d {
                let p2 = cv.world_to_canvas(canvas_rect, pt_3d)?;
                min_pt.x = min_pt.x.min(p2.x);
                min_pt.y = min_pt.y.min(p2.y);
                max_pt.x = max_pt.x.max(p2.x);
                max_pt.y = max_pt.y.max(p2.y);
            }
            Some(Rect::from_min_max(min_pt, max_pt))
        });

        let fallback_offsets = [
            Vec2::new(95.0, -28.0),
            Vec2::new(-135.0, 75.0),
            Vec2::new(125.0, 65.0),
        ];
        let box_rect = projected_box.unwrap_or_else(|| {
            let off = fallback_offsets[idx % fallback_offsets.len()];
            Rect::from_center_size(canvas_rect.center() + off, Vec2::new(56.0, 32.0))
        });

        painter.rect_filled(box_rect, CornerRadius::same(3), fill);
        paint_dashed_rect(painter, box_rect, stroke, 5.0, 4.0, phase);
        painter.text(
            Pos2::new(box_rect.center().x, box_rect.top() - 8.0),
            Align2::CENTER_BOTTOM,
            format!("{} {}", ghost.op.symbol(), ghost.name),
            font_mono(10.0),
            TEXT_YELLOW,
        );
    }
}

fn paint_dashed_rect(
    painter: &egui::Painter,
    rect: Rect,
    stroke: Stroke,
    dash: f32,
    gap: f32,
    phase: f32,
) {
    let corners = [
        (rect.left_top(), rect.right_top()),
        (rect.right_top(), rect.right_bottom()),
        (rect.right_bottom(), rect.left_bottom()),
        (rect.left_bottom(), rect.left_top()),
    ];
    let step = (dash + gap).max(1.0);
    let offset = phase.rem_euclid(step);
    for (a, b) in corners {
        let dir = b - a;
        let len = dir.length();
        if len <= 0.001 {
            continue;
        }
        let unit = dir / len;
        let mut t = -offset;
        while t < len {
            let s = t.max(0.0);
            let e = (t + dash).min(len);
            if e > s {
                painter.line_segment([a + unit * s, a + unit * e], stroke);
            }
            t += step;
        }
    }
}

fn draw_viewport_proposal_card(
    ui: &mut Ui,
    world: &mut World,
    canvas_rect: Rect,
    state: &mut EditorUiState,
) {
    let pending_lines: Vec<AgentDiffLine> = state
        .agent_log
        .iter()
        .rev()
        .find(|e| e.status == ProposalState::Pending)
        .map(|e| e.diff_lines.clone())
        .unwrap_or_default();

    let change_count = pending_lines.len().max(1);
    let shown_lines = pending_lines.len().clamp(1, 5) as f32;
    let card_size = Vec2::new(290.0, 96.0 + 20.0 * shown_lines);
    // Keep clear of the bottom-centre toast (~56 px).
    let toast_clearance = if state.active_toast.is_some() {
        56.0
    } else {
        10.0
    };
    let card_rect = Rect::from_min_size(
        Pos2::new(
            canvas_rect.right() - card_size.x - 10.0,
            canvas_rect.bottom() - card_size.y - toast_clearance,
        ),
        card_size,
    );

    let shadow = Shadow {
        offset: [0, 10],
        blur: 30,
        spread: 0,
        color: Color32::from_black_alpha(128),
    };
    ui.painter()
        .add(shadow.as_shape(card_rect, CornerRadius::same(RADIUS_CARD)));
    ui.painter().rect(
        card_rect,
        CornerRadius::same(RADIUS_CARD),
        BG_TAB_BAR,
        Stroke::new(1.0, BORDER_AGENT),
        StrokeKind::Inside,
    );

    ui.scope_builder(egui::UiBuilder::new().max_rect(card_rect), |ui| {
        let header_rect = Rect::from_min_size(card_rect.min, Vec2::new(card_rect.width(), 32.0));
        ui.painter().line_segment(
            [header_rect.left_bottom(), header_rect.right_bottom()],
            Stroke::new(1.0, BORDER_AGENT),
        );
        let pulse = ((state.current_time * std::f64::consts::TAU / 1.4).sin() * 0.5 + 0.5) as f32;
        ui.painter().circle_filled(
            Pos2::new(header_rect.left() + 13.0, header_rect.center().y),
            3.0,
            ACCENT_YELLOW.gamma_multiply(0.35 + 0.65 * pulse),
        );
        ui.painter().text(
            Pos2::new(header_rect.left() + 24.0, header_rect.center().y),
            Align2::LEFT_CENTER,
            "Proposition de l'agent",
            font_sans_bold(12.0),
            TEXT_YELLOW_LIGHT,
        );
        ui.painter().text(
            Pos2::new(header_rect.right() - 10.0, header_rect.center().y),
            Align2::RIGHT_CENTER,
            format!("{change_count} changements"),
            font_mono(10.5),
            TEXT_YELLOW,
        );

        ui.add_space(36.0);
        Frame::NONE
            .inner_margin(Margin::symmetric(10, 0))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for line in pending_lines.iter().take(5) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        ui.label(
                            RichText::new(line.op.symbol())
                                .font(font_mono(11.0))
                                .color(line.op.color()),
                        );
                        ui.label(
                            RichText::new(&line.text)
                                .font(font_mono(11.0))
                                .color(TEXT_PRIMARY),
                        );
                    });
                }

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if CruftButton::new("prop_apply", "Appliquer")
                        .variant(ButtonVariant::Primary)
                        .kbd("A")
                        .show(ui)
                        .clicked()
                    {
                        apply_pending_proposal(world, state);
                    }
                    if CruftButton::new("prop_reject", "Rejeter")
                        .variant(ButtonVariant::Danger)
                        .show(ui)
                        .clicked()
                    {
                        reject_pending_proposal(world, state);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if CruftButton::new("prop_details", "Détails")
                            .variant(ButtonVariant::Default)
                            .show(ui)
                            .clicked()
                        {
                            state.focus_tab(EditorTab::Assistant);
                        }
                    });
                });
            });
    });
}

// ---------------------------------------------------------------------------
// Right Panel: Generic Inspector Driven by `cruft_core::component_views`
// ---------------------------------------------------------------------------

fn draw_inspector_tab(ui: &mut Ui, world: &mut World, state: &mut EditorUiState) {
    let Some(raw_selected) = world.resource::<Selection>().primary() else {
        Frame::NONE.inner_margin(Margin::same(16)).show(ui, |ui| {
            ui.label(
                RichText::new("Aucune entité sélectionnée.")
                    .font(font_sans(12.0))
                    .color(TEXT_SECONDARY),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new("Cliquez sur une entité dans l'Outliner ou dans le Viewport 3D.")
                    .font(font_sans(11.5))
                    .color(TEXT_MUTED),
            );
        });
        return;
    };

    let entity = resolve_entity(world, raw_selected);
    let Ok(views) = component_views(world, entity) else {
        Frame::NONE.inner_margin(Margin::same(16)).show(ui, |ui| {
            ui.label(
                RichText::new("Entité introuvable.")
                    .font(font_sans(12.0))
                    .color(TEXT_SECONDARY),
            );
        });
        return;
    };

    let current_name = world
        .get::<Name>(entity)
        .map(|n| n.as_str().to_owned())
        .unwrap_or_else(|| format!("Entity #{}", entity.index()));
    let entity_id_badge = format!("#{}v{}", entity.index(), entity.generation());
    let mut visible_on = !matches!(world.get::<Visibility>(entity), Some(Visibility::Hidden));

    ScrollArea::vertical()
        .id_salt("inspector_scroll")
        .show(ui, |ui| {
            // 1. Entity Header (Visibility Switch + Editable Name Box + Entity ID)
            Frame::NONE.inner_margin(Margin::same(10)).show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if toggle_switch(ui, "insp_entity_vis", &mut visible_on).changed() {
                        let (value, variant) = if visible_on {
                            (Visibility::Inherited, "Inherited")
                        } else {
                            (Visibility::Hidden, "Hidden")
                        };
                        set_or_insert(world, entity, value, variant);
                    }

                    let id_w = 42.0;
                    let name_w = (ui.available_width() - id_w - 8.0).max(80.0);
                    let name_key = (entity, "bevy_ecs::name::Name".to_owned(), String::new());
                    let name_buf = state
                        .field_edit_buffers
                        .entry(name_key.clone())
                        .or_insert_with(|| current_name.clone());

                    Frame::NONE
                        .fill(BG_FIELD)
                        .stroke(Stroke::new(1.0, BORDER_PANEL))
                        .corner_radius(CornerRadius::same(RADIUS_WIDGET))
                        .inner_margin(Margin::symmetric(8, 3))
                        .show(ui, |ui| {
                            let resp = ui.add_sized(
                                [name_w - 16.0, 20.0],
                                egui::TextEdit::singleline(name_buf)
                                    .frame(Frame::NONE)
                                    .font(font_sans_bold(13.0)),
                            );
                            if !resp.has_focus() && !resp.lost_focus() {
                                *name_buf = current_name.clone();
                            }
                            if resp.lost_focus() || ui.input(|i| i.key_pressed(Key::Enter)) {
                                let committed = name_buf.clone();
                                rename_entity_command(world, entity, &committed);
                            }
                        });

                    ui.label(
                        RichText::new(entity_id_badge)
                            .font(font_mono(10.5))
                            .color(TEXT_MUTED),
                    );
                });

                ui.add_space(6.0);
                // Component short tags
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for comp in &views {
                        let short = short_type_name(&comp.name);
                        Frame::NONE
                            .fill(BG_MENU_HOVER)
                            .corner_radius(CornerRadius::same(4))
                            .inner_margin(Margin::symmetric(6, 2))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(short)
                                        .font(font_mono(10.5))
                                        .color(TEXT_CONTROL),
                                );
                            });
                    }
                });
            });

            // 2. Render every ComponentView from `cruft_core::component_views`
            for comp in &views {
                draw_component_view_section(ui, world, state, entity, comp);
            }
        });
}

fn draw_component_view_section(
    ui: &mut Ui,
    world: &mut World,
    state: &mut EditorUiState,
    entity: Entity,
    comp: &ComponentView,
) {
    let short_name = short_type_name(&comp.name);
    let source_crate = crate_prefix(&comp.name);
    let is_collapsed = state.collapsed_components.contains(&comp.name);

    let top_rect = ui.available_rect_before_wrap();
    ui.painter().line_segment(
        [top_rect.left_top(), top_rect.right_top()],
        Stroke::new(1.0, BORDER_PANEL),
    );

    let id = ui.make_persistent_id(("comp_head", &comp.name));
    let (head_rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());
    if resp.clicked() && !state.collapsed_components.remove(&comp.name) {
        state.collapsed_components.insert(comp.name.clone());
    }
    let hover_t = ui
        .ctx()
        .animate_bool_with_time(id, resp.hovered(), theme::HOVER_ANIM_SECS);
    if hover_t > 0.01 {
        ui.painter().rect_filled(
            head_rect,
            CornerRadius::ZERO,
            lerp_color(
                Color32::TRANSPARENT,
                Color32::from_rgb(0x1B, 0x24, 0x28),
                hover_t,
            ),
        );
    }

    let painter = ui.painter();
    let tri_c = Pos2::new(head_rect.left() + 14.0, head_rect.center().y);
    let tri_pts = if is_collapsed {
        vec![
            Pos2::new(tri_c.x - 2.0, tri_c.y - 3.5),
            Pos2::new(tri_c.x + 2.5, tri_c.y),
            Pos2::new(tri_c.x - 2.0, tri_c.y + 3.5),
        ]
    } else {
        vec![
            Pos2::new(tri_c.x - 3.5, tri_c.y - 2.0),
            Pos2::new(tri_c.x + 3.5, tri_c.y - 2.0),
            Pos2::new(tri_c.x, tri_c.y + 3.0),
        ]
    };
    painter.add(egui::Shape::convex_polygon(
        tri_pts,
        TEXT_SECONDARY,
        Stroke::NONE,
    ));

    let tick_rect = Rect::from_min_size(
        Pos2::new(head_rect.left() + 24.0, head_rect.center().y - 6.0),
        Vec2::new(2.0, 12.0),
    );
    painter.rect_filled(tick_rect, CornerRadius::same(1), ACCENT_BLUE);

    painter.text(
        Pos2::new(tick_rect.right() + 7.0, head_rect.center().y),
        Align2::LEFT_CENTER,
        short_name,
        font_sans_bold(12.0),
        TEXT_PRIMARY,
    );
    painter.text(
        Pos2::new(head_rect.right() - 10.0, head_rect.center().y),
        Align2::RIGHT_CENTER,
        source_crate,
        font_mono(10.0),
        TEXT_MUTED,
    );

    if is_collapsed {
        return;
    }

    Frame::NONE
        .inner_margin(Margin {
            left: 10,
            right: 10,
            top: 2,
            bottom: 10,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 5.0;

            let mut consumed_indices = HashSet::new();
            for (idx, field) in comp.fields.iter().enumerate() {
                if consumed_indices.contains(&idx) {
                    continue;
                }

                // 1. Check if this field is a Struct/Tuple container whose `.x`, `.y`, `.z` float children exist
                if matches!(field.kind, FieldKind::Struct | FieldKind::Tuple)
                    && !field.path.is_empty()
                {
                    let px = format!("{}.x", field.path);
                    let py = format!("{}.y", field.path);
                    let pz = format!("{}.z", field.path);
                    let pw = format!("{}.w", field.path);
                    let has_w = comp.fields.iter().any(|f| f.path == pw);

                    if !has_w
                        && let (Some((ix, fx)), Some((iy, fy)), Some((iz, fz))) = (
                            find_float_field(&comp.fields, &px),
                            find_float_field(&comp.fields, &py),
                            find_float_field(&comp.fields, &pz),
                        )
                    {
                        consumed_indices.insert(idx);
                        consumed_indices.insert(ix);
                        consumed_indices.insert(iy);
                        consumed_indices.insert(iz);

                        let mut vals = [fx, fy, fz];
                        let out = vec3_field_detailed(
                            ui,
                            &format!("{}_{}", comp.name, field.path),
                            &field.path,
                            &mut vals,
                            0.02,
                            "",
                        );
                        if let Some((axis_idx, new_v)) = out.changed_axis {
                            let sub_path = [&px, &py, &pz][axis_idx];
                            let _ = History::execute(
                                world,
                                SetComponentField::new(
                                    entity,
                                    &comp.name,
                                    sub_path,
                                    format_float_ron(new_v as f64),
                                ),
                            );
                        }
                        if out.drag_stopped {
                            world.resource_mut::<History>().finish_group();
                        }
                        continue;
                    }
                }

                // 2. Skip container Struct/Tuple row if it has child fields below it in `comp.fields`
                let has_child_fields = !field.path.is_empty()
                    && comp.fields.iter().any(|other| {
                        other.path.len() > field.path.len()
                            && other.path.starts_with(&field.path)
                            && matches!(other.path.as_bytes()[field.path.len()], b'.' | b'[')
                    });
                if has_child_fields && matches!(field.kind, FieldKind::Struct | FieldKind::Tuple) {
                    continue;
                }

                // 3. Skip unserializable internal subfields (e.g. `Name.0` HashedStr when `Name` root `""` is already editable)
                if field.error.is_some() && !field.path.is_empty() && comp.fields[0].editable {
                    continue;
                }

                draw_single_field_view(ui, world, state, entity, &comp.name, field);
            }
        });
}

fn find_float_field(fields: &[FieldView], path: &str) -> Option<(usize, f32)> {
    fields
        .iter()
        .enumerate()
        .find(|(_, f)| f.path == path && f.kind == FieldKind::Float && f.editable)
        .and_then(|(idx, f)| f.value.parse::<f32>().ok().map(|v| (idx, v)))
}

fn draw_single_field_view(
    ui: &mut Ui,
    world: &mut World,
    state: &mut EditorUiState,
    entity: Entity,
    comp_name: &str,
    field: &FieldView,
) {
    let label = if field.path.is_empty() {
        short_type_name(comp_name)
    } else {
        field.path.as_str()
    };
    let id_salt = format!("{comp_name}::{}", field.path);

    match &field.kind {
        FieldKind::Float => {
            let current = field.value.parse::<f64>().unwrap_or(0.0);
            let step = (current.abs() * 0.01).clamp(0.01, 50.0);
            let display = format_float_display(current);
            let (delta_x, stopped) =
                draggable_scalar_row(ui, &id_salt, label, &display, field.editable);
            if let Some(dx) = delta_x {
                let next = current + (dx as f64) * step;
                let _ = History::execute(
                    world,
                    SetComponentField::new(entity, comp_name, &field.path, format_float_ron(next)),
                );
            }
            if stopped {
                world.resource_mut::<History>().finish_group();
            }
        }
        FieldKind::SignedInteger => {
            let current = field.value.parse::<i64>().unwrap_or(0);
            let (delta_x, stopped) =
                draggable_scalar_row(ui, &id_salt, label, &field.value, field.editable);
            if let Some(dx) = delta_x {
                let step = if dx > 0.0 {
                    (dx.ceil() as i64).max(1)
                } else {
                    (dx.floor() as i64).min(-1)
                };
                let next = current.saturating_add(step);
                if next != current {
                    let _ = History::execute(
                        world,
                        SetComponentField::new(entity, comp_name, &field.path, next.to_string()),
                    );
                }
            }
            if stopped {
                world.resource_mut::<History>().finish_group();
            }
        }
        FieldKind::UnsignedInteger => {
            let current = field.value.parse::<u64>().unwrap_or(0);
            let (delta_x, stopped) =
                draggable_scalar_row(ui, &id_salt, label, &field.value, field.editable);
            if let Some(dx) = delta_x {
                let next = if dx > 0.0 {
                    current.saturating_add((dx.ceil() as u64).max(1))
                } else {
                    current.saturating_sub(((-dx).ceil() as u64).max(1))
                };
                if next != current {
                    let _ = History::execute(
                        world,
                        SetComponentField::new(entity, comp_name, &field.path, next.to_string()),
                    );
                }
            }
            if stopped {
                world.resource_mut::<History>().finish_group();
            }
        }
        FieldKind::Bool => {
            let mut val = field.value.trim() == "true";
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (lbl_rect, _) = ui.allocate_exact_size(Vec2::new(96.0, 22.0), Sense::hover());
                ui.painter().text(
                    Pos2::new(lbl_rect.left(), lbl_rect.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    font_sans(11.5),
                    TEXT_CONTROL,
                );
                if field.editable && toggle_switch(ui, &id_salt, &mut val).changed() {
                    let _ = History::execute(
                        world,
                        SetComponentField::new(entity, comp_name, &field.path, val.to_string()),
                    );
                    world.resource_mut::<History>().finish_group();
                }
            });
        }
        FieldKind::Enum { variants } if field.editable && !variants.is_empty() => {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let (lbl_rect, _) = ui.allocate_exact_size(Vec2::new(96.0, 22.0), Sense::hover());
                ui.painter().text(
                    Pos2::new(lbl_rect.left(), lbl_rect.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    font_sans(11.5),
                    TEXT_CONTROL,
                );
                for variant in variants {
                    let is_active = field.value == *variant
                        || field.value.starts_with(&format!("{variant}("))
                        || field.value.starts_with(&format!("{variant}{{"));
                    if CruftButton::new(&format!("{id_salt}_{variant}"), variant)
                        .variant(ButtonVariant::Icon { active: is_active })
                        .bordered(true)
                        .show(ui)
                        .clicked()
                        && !is_active
                    {
                        let _ = History::execute(
                            world,
                            SetComponentField::new(entity, comp_name, &field.path, variant.clone()),
                        );
                        world.resource_mut::<History>().finish_group();
                    }
                }
            });
        }
        _ => {
            // Special case: Transform.rotation Euler X/Y/Z draggable helper if on Transform
            if field.path == "rotation"
                && field.editable
                && let Some(tr) = world.get::<Transform>(entity).copied()
            {
                let (ry, rx, rz) = tr.rotation.to_euler(EulerRot::YXZ);
                let mut deg = [rx.to_degrees(), ry.to_degrees(), rz.to_degrees()];
                let out = vec3_field_detailed(
                    ui,
                    &format!("{comp_name}_rotation_euler"),
                    "rotation",
                    &mut deg,
                    0.35,
                    "°",
                );
                if out.changed_axis.is_some() {
                    let new_rot = Quat::from_euler(
                        EulerRot::YXZ,
                        deg[1].to_radians(),
                        deg[0].to_radians(),
                        deg[2].to_radians(),
                    )
                    .normalize();
                    if let Some(ron_rot) = serialize_quat_ron(world, new_rot) {
                        let _ = History::execute(
                            world,
                            SetComponentField::new(entity, comp_name, "rotation", ron_rot),
                        );
                    }
                }
                if out.drag_stopped {
                    world.resource_mut::<History>().finish_group();
                }
                return;
            }

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (lbl_rect, _) = ui.allocate_exact_size(Vec2::new(96.0, 22.0), Sense::hover());
                ui.painter().text(
                    Pos2::new(lbl_rect.left(), lbl_rect.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    font_sans(11.5),
                    TEXT_CONTROL,
                );

                if field.editable {
                    let key = (entity, comp_name.to_owned(), field.path.clone());
                    let is_string_field = field.kind == FieldKind::String
                        || (field.path.is_empty() && comp_name.ends_with("::Name"));
                    let initial_txt = if is_string_field {
                        ron::from_str::<String>(&field.value)
                            .unwrap_or_else(|_| field.value.clone())
                    } else {
                        field.value.clone()
                    };

                    let buf = state
                        .field_edit_buffers
                        .entry(key)
                        .or_insert_with(|| initial_txt.clone());

                    Frame::NONE
                        .fill(BG_FIELD)
                        .stroke(Stroke::new(1.0, BORDER_PANEL))
                        .corner_radius(CornerRadius::same(RADIUS_WIDGET))
                        .inner_margin(Margin::symmetric(7, 2))
                        .show(ui, |ui| {
                            let resp = ui.add_sized(
                                [ui.available_width().max(40.0), 18.0],
                                egui::TextEdit::singleline(buf)
                                    .frame(Frame::NONE)
                                    .font(font_mono(11.0)),
                            );
                            if !resp.has_focus() && !resp.lost_focus() {
                                *buf = initial_txt.clone();
                            }
                            if (resp.lost_focus() || ui.input(|i| i.key_pressed(Key::Enter)))
                                && *buf != initial_txt
                            {
                                let ron_payload = if is_string_field {
                                    ron::to_string(buf).unwrap_or_else(|_| buf.clone())
                                } else {
                                    buf.clone()
                                };
                                let _ = History::execute(
                                    world,
                                    SetComponentField::new(
                                        entity,
                                        comp_name,
                                        &field.path,
                                        ron_payload,
                                    ),
                                );
                                world.resource_mut::<History>().finish_group();
                            }
                        });
                } else {
                    let (_, _) = draggable_scalar_row(ui, &id_salt, "", &field.value, false);
                }
            });
        }
    }
}

/// Formats a floating-point value as a valid RON float literal (`1.0`, `-2.5`, etc.).
pub fn format_float_ron(val: f64) -> String {
    if !val.is_finite() {
        return "0.0".to_owned();
    }
    let s = format!("{val:.4}");
    let trimmed = s.trim_end_matches('0');
    if trimmed.ends_with('.') {
        format!("{trimmed}0")
    } else {
        trimmed.to_owned()
    }
}

fn format_float_display(val: f64) -> String {
    if val.abs() >= 1000.0 {
        format!("{val:.0}")
    } else {
        format!("{val:.2}")
    }
}

fn short_type_name(type_path: &str) -> &str {
    let base = type_path.split('<').next().unwrap_or(type_path);
    base.rsplit("::").next().unwrap_or(type_path)
}

fn crate_prefix(type_path: &str) -> &str {
    let first = type_path.split("::").next().unwrap_or("app");
    if first.starts_with("bevy") {
        "bevy"
    } else if first.starts_with("cruft") {
        "cruft"
    } else {
        first
    }
}

// ---------------------------------------------------------------------------
// Bottom Panel: Assets, Console, Timeline & Right Assistant IA Tab
// ---------------------------------------------------------------------------

fn draw_assets_tab(ui: &mut Ui, world: &mut World, state: &mut EditorUiState) {
    if let Some(mut catalog) = world.get_resource_mut::<AssetCatalog>() {
        ui.horizontal(|ui| {
            ui.label(RichText::new("assets/").font(font_mono(11.0)));
            if ui.button("Actualiser").clicked() {
                catalog.refresh();
            }
            ui.label(format!("{} fichiers", catalog.entries().len()));
        });
        for error in &catalog.errors {
            ui.colored_label(ACCENT_RED, error);
        }
    }
    let entries = world
        .get_resource::<AssetCatalog>()
        .map(|c| c.entries().to_vec())
        .unwrap_or_default();
    if entries.is_empty() {
        ui.label("Aucun asset reconnu dans assets/ · GLB, PNG, EXR, RON, SCENE");
    }
    ScrollArea::vertical()
        .id_salt("assets_grid_scroll")
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
                for entry in &entries {
                    let asset = AssetTile {
                        name: &entry.path,
                        kind: entry.kind.label(),
                        c1: ACCENT_BLUE,
                        c2: BG_SELECTION,
                        c3: ACCENT_BLUE_LIGHT,
                    };
                    let response = draw_asset_tile(ui, &asset).on_hover_text(&entry.path);
                    let pressed = ui.input(|input| input.events.iter().any(|event| {
                        matches!(event, egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, .. }
                            if response.rect.intersect(ui.clip_rect()).contains(*pos))
                    }));
                    if entry.kind == AssetKind::Glb && (response.drag_started() || pressed) {
                        state.dragged_asset = Some(AssetDrag { path: entry.path.clone() });
                    }
                }
            });
        });
}

fn finish_asset_drag(ctx: &egui::Context, world: &mut World, state: &mut EditorUiState) {
    let (released, cancelled, position) = ctx.input(|input| {
        (
            input.pointer.any_released(),
            input.key_pressed(Key::Escape),
            input.pointer.interact_pos(),
        )
    });
    if cancelled {
        state.dragged_asset = None;
    }
    if released
        && let Some(asset) = state.dragged_asset.take()
        && position.is_some_and(|pos| {
            state
                .last_viewport_rect
                .is_some_and(|rect| rect.contains(pos))
        })
    {
        if let Err(error) = import_glb(world, &asset.path, bevy::prelude::Vec3::ZERO) {
            world.trigger(EditorEvent::Toast(error.to_string()));
        }
        world.resource_mut::<History>().finish_group();
    }
    if let Some(asset) = &state.dragged_asset
        && let Some(position) = position
    {
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            Id::new("asset_drag"),
        ))
        .text(
            position + Vec2::new(16.0, 16.0),
            Align2::LEFT_TOP,
            &asset.path,
            font_mono(11.0),
            TEXT_PRIMARY,
        );
        ctx.set_cursor_icon(CursorIcon::Grabbing);
    }
}

fn draw_asset_tile(ui: &mut Ui, asset: &AssetTile<'_>) -> egui::Response {
    let id = ui.make_persistent_id(("asset_tile", asset.name));
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(88.0, 96.0), Sense::click_and_drag());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let hover_t = ui
        .ctx()
        .animate_bool_with_time(id, resp.hovered(), theme::HOVER_ANIM_SECS);

    let bg = lerp_color(
        Color32::TRANSPARENT,
        Color32::from_rgb(0x1B, 0x24, 0x28),
        hover_t,
    );
    let border = lerp_color(Color32::TRANSPARENT, BORDER_CONTROL, hover_t);
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(7),
        bg,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );

    let thumb_rect = Rect::from_min_size(
        Pos2::new(rect.left() + 5.0, rect.top() + 5.0),
        Vec2::new(78.0, 54.0),
    );
    painter.rect_filled(thumb_rect, CornerRadius::same(RADIUS_WIDGET), BG_FIELD);

    let cx = thumb_rect.center().x;
    let cy = thumb_rect.center().y + 2.0;
    let top_face = vec![
        Pos2::new(cx, cy - 17.0),
        Pos2::new(cx + 18.0, cy - 7.0),
        Pos2::new(cx, cy + 3.0),
        Pos2::new(cx - 18.0, cy - 7.0),
    ];
    let left_face = vec![
        Pos2::new(cx - 18.0, cy - 7.0),
        Pos2::new(cx, cy + 3.0),
        Pos2::new(cx, cy + 17.0),
        Pos2::new(cx - 18.0, cy + 7.0),
    ];
    let right_face = vec![
        Pos2::new(cx + 18.0, cy - 7.0),
        Pos2::new(cx, cy + 3.0),
        Pos2::new(cx, cy + 17.0),
        Pos2::new(cx + 18.0, cy + 7.0),
    ];
    painter.add(egui::Shape::convex_polygon(
        top_face,
        asset.c1,
        Stroke::NONE,
    ));
    painter.add(egui::Shape::convex_polygon(
        left_face,
        asset.c2,
        Stroke::NONE,
    ));
    painter.add(egui::Shape::convex_polygon(
        right_face,
        asset.c3,
        Stroke::NONE,
    ));

    painter.text(
        Pos2::new(rect.left() + 5.0, thumb_rect.bottom() + 6.0),
        Align2::LEFT_TOP,
        asset.name,
        font_sans(11.0),
        TEXT_BODY,
    );
    painter.text(
        Pos2::new(rect.left() + 5.0, thumb_rect.bottom() + 21.0),
        Align2::LEFT_TOP,
        asset.kind,
        font_mono(9.5),
        TEXT_MUTED,
    );

    resp
}

fn draw_console_tab(ui: &mut Ui, world: &World) {
    let Some(logs) = world.get_resource::<ConsoleLog>() else {
        ui.label("Capture tracing non installée");
        return;
    };
    ui.horizontal(|ui| {
        if ui.button("Effacer").clicked() {
            logs.clear();
        }
        ui.label(format!("{} anciens messages évincés", logs.dropped()));
    });
    let entries = logs.entries();
    ScrollArea::vertical()
        .id_salt("console_logs_scroll")
        .stick_to_bottom(true)
        .show_rows(ui, 20.0, entries.len(), |ui, range| {
            for entry in &entries[range] {
                let color = match entry.level {
                    bevy::log::Level::ERROR => ACCENT_RED,
                    bevy::log::Level::WARN => ACCENT_YELLOW,
                    _ => TEXT_SECONDARY,
                };
                let time = entry
                    .timestamp
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{}.{:03} {} [{}] {}",
                            time.as_secs() % 86400,
                            time.subsec_millis(),
                            entry.level,
                            entry.target,
                            entry.message
                        ))
                        .font(font_mono(11.0))
                        .color(color),
                    );
                });
            }
        });
}

fn draw_timeline_tab(ui: &mut Ui) {
    let full_rect = ui.available_rect_before_wrap();
    let left_w = 180.0;
    let left_rect = Rect::from_min_size(full_rect.min, Vec2::new(left_w, full_rect.height()));
    let right_rect = Rect::from_min_max(
        Pos2::new(left_rect.right() + 1.0, full_rect.top()),
        full_rect.max,
    );

    ui.painter().line_segment(
        [left_rect.right_top(), left_rect.right_bottom()],
        Stroke::new(1.0, BORDER_PANEL),
    );

    ui.scope_builder(egui::UiBuilder::new().max_rect(left_rect), |ui| {
        let cap_rect = Rect::from_min_size(left_rect.min, Vec2::new(left_w, 24.0));
        ui.painter().line_segment(
            [cap_rect.left_bottom(), cap_rect.right_bottom()],
            Stroke::new(1.0, BORDER_PANEL),
        );
        ui.painter().text(
            Pos2::new(cap_rect.left() + 10.0, cap_rect.center().y),
            Align2::LEFT_CENTER,
            "PLAYER · CLIP « IDLE_LOOK »",
            font_mono(9.5),
            TEXT_SECONDARY,
        );

        ui.add_space(24.0);
        for (label, col) in [
            ("Transform.translation", AXIS_X),
            ("Transform.rotation", AXIS_Y),
            ("Camera.fov", AXIS_Z),
            ("Event · footstep", ACCENT_YELLOW),
        ] {
            let (row_rect, _) = ui.allocate_exact_size(Vec2::new(left_w, 26.0), Sense::hover());
            let tick = Rect::from_min_size(
                Pos2::new(row_rect.left() + 10.0, row_rect.center().y - 6.0),
                Vec2::new(2.0, 12.0),
            );
            ui.painter().rect_filled(tick, CornerRadius::same(1), col);
            ui.painter().text(
                Pos2::new(tick.right() + 7.0, row_rect.center().y),
                Align2::LEFT_CENTER,
                label,
                font_sans(11.5),
                TEXT_BODY,
            );
        }
    });

    let painter = ui.painter();
    let ruler_rect = Rect::from_min_size(right_rect.min, Vec2::new(right_rect.width(), 24.0));
    painter.rect_filled(ruler_rect, CornerRadius::ZERO, BG_TAB_BAR);
    painter.line_segment(
        [ruler_rect.left_bottom(), ruler_rect.right_bottom()],
        Stroke::new(1.0, BORDER_PANEL),
    );

    for (i, frame_num) in [0, 24, 48, 72, 96, 120].into_iter().enumerate() {
        let x = right_rect.left() + 40.0 + (i as f32) * 110.0;
        if x < right_rect.right() - 20.0 {
            painter.line_segment(
                [
                    Pos2::new(x, ruler_rect.bottom() - 8.0),
                    Pos2::new(x, ruler_rect.bottom()),
                ],
                Stroke::new(1.0, BORDER_HOVER),
            );
            painter.text(
                Pos2::new(x + 4.0, ruler_rect.top() + 11.0),
                Align2::LEFT_CENTER,
                format!("{frame_num}"),
                font_mono(10.0),
                TEXT_SECONDARY,
            );
        }
    }

    for (row_idx, (col, keys)) in [
        (AXIS_X, &[0.0f32, 1.5, 3.0, 4.0][..]),
        (AXIS_Y, &[0.0f32, 1.0, 2.5, 4.0][..]),
        (AXIS_Z, &[0.0f32, 3.6][..]),
    ]
    .into_iter()
    .enumerate()
    {
        let y = ruler_rect.bottom() + 13.0 + (row_idx as f32) * 26.0;
        let x_start = right_rect.left() + 40.0;
        let x_end =
            (x_start + keys.last().copied().unwrap_or(1.0) * 110.0).min(right_rect.right() - 12.0);
        painter.line_segment(
            [Pos2::new(x_start, y), Pos2::new(x_end, y)],
            Stroke::new(2.0, col.gamma_multiply(0.28)),
        );
        for &k in keys {
            let kx = x_start + k * 110.0;
            if kx < right_rect.right() - 8.0 {
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        Pos2::new(kx, y - 5.0),
                        Pos2::new(kx + 5.0, y),
                        Pos2::new(kx, y + 5.0),
                        Pos2::new(kx - 5.0, y),
                    ],
                    col,
                    Stroke::NONE,
                ));
            }
        }
    }

    let playhead_x =
        (right_rect.left() + 40.0 + (46.0 / 24.0) * 110.0).min(right_rect.right() - 10.0);
    painter.line_segment(
        [
            Pos2::new(playhead_x, right_rect.top()),
            Pos2::new(playhead_x, right_rect.bottom()),
        ],
        Stroke::new(1.5, ACCENT_RED),
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(playhead_x - 6.0, right_rect.top()),
            Pos2::new(playhead_x + 6.0, right_rect.top()),
            Pos2::new(playhead_x + 6.0, right_rect.top() + 9.0),
            Pos2::new(playhead_x, right_rect.top() + 14.0),
            Pos2::new(playhead_x - 6.0, right_rect.top() + 9.0),
        ],
        ACCENT_RED,
        Stroke::NONE,
    ));
}

fn draw_assistant_tab(ui: &mut Ui, world: &mut World, state: &mut EditorUiState) {
    let full_rect = ui.available_rect_before_wrap();
    let header_h = 34.0;
    let footer_h = 74.0;

    // 1. Header: pulsing dot + claude-code + MCP status + Mode toggle (proposer / direct)
    let header_rect = Rect::from_min_size(full_rect.min, Vec2::new(full_rect.width(), header_h));
    ui.painter().line_segment(
        [header_rect.left_bottom(), header_rect.right_bottom()],
        Stroke::new(1.0, BORDER_PANEL),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(header_rect), |ui| {
        Frame::NONE
            .inner_margin(Margin::symmetric(10, 5))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let pulse = ((state.current_time * std::f64::consts::TAU / 1.4).sin() * 0.5
                        + 0.5) as f32;
                    let (dot_rect, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                    ui.painter().circle_filled(
                        dot_rect.center(),
                        3.0,
                        state.agent_status.color.gamma_multiply(0.35 + 0.65 * pulse),
                    );
                    ui.label(
                        RichText::new(state.agent_status.label.as_str())
                            .font(font_mono(11.0))
                            .color(TEXT_YELLOW_LIGHT),
                    );
                    ui.label(
                        RichText::new(state.agent_status.detail.as_str())
                            .font(font_mono(10.5))
                            .color(TEXT_MUTED),
                    )
                    .on_hover_text("Adresse du serveur MCP de Cruft (voir docs/mcp.md)");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let mode_label = match state.agent_mode {
                            AgentMode::Propose => "Mode : proposer ▾",
                            AgentMode::Direct => "Mode : direct ▾",
                        };
                        if CruftButton::new("agent_mode_btn", mode_label)
                            .variant(ButtonVariant::Icon {
                                active: state.agent_mode == AgentMode::Direct,
                            })
                            .bordered(true)
                            .tooltip("Basculer entre proposer (fantôme) et appliquer directement")
                            .show(ui)
                            .clicked()
                        {
                            state.agent_mode = match state.agent_mode {
                                AgentMode::Propose => AgentMode::Direct,
                                AgentMode::Direct => AgentMode::Propose,
                            };
                            *world.resource_mut::<AgentMode>() = state.agent_mode;
                        }
                    });
                });
            });
    });

    // 2. Action Journal (NO chat bubbles — structured log with time, title, status, diff block, MCP tools + duration)
    let log_rect = Rect::from_min_max(
        Pos2::new(full_rect.left(), header_rect.bottom()),
        Pos2::new(full_rect.right(), full_rect.bottom() - footer_h),
    );
    let mut trigger_apply = false;
    let mut trigger_reject = false;
    let entries_snapshot = state.agent_log.clone();

    ui.scope_builder(egui::UiBuilder::new().max_rect(log_rect), |ui| {
        ScrollArea::vertical()
            .id_salt("agent_journal_scroll")
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for (entry_idx, entry) in entries_snapshot.iter().enumerate() {
                    let (act_apply, act_reject) = draw_agent_log_entry(ui, entry_idx, entry);
                    if act_apply {
                        trigger_apply = true;
                    }
                    if act_reject {
                        trigger_reject = true;
                    }
                }
            });
    });

    if trigger_apply {
        apply_pending_proposal(world, state);
    } else if trigger_reject {
        reject_pending_proposal(world, state);
    }

    // 3. Bottom Instruction Bar with Context Chips (@entité sélectionnée, @scène)
    let footer_rect = Rect::from_min_max(
        Pos2::new(full_rect.left(), full_rect.bottom() - footer_h),
        full_rect.max,
    );
    ui.painter().line_segment(
        [footer_rect.left_top(), footer_rect.right_top()],
        Stroke::new(1.0, BORDER_PANEL),
    );
    let sel_name = world
        .resource::<Selection>()
        .primary()
        .and_then(|e| world.get::<Name>(resolve_entity(world, e)))
        .map_or("aucune_entité", |n| n.as_str())
        .to_owned();

    let mut submit_now = false;
    ui.scope_builder(egui::UiBuilder::new().max_rect(footer_rect), |ui| {
        Frame::NONE
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;

                    let sel_chip_resp = Frame::NONE
                        .fill(if state.context_include_selected {
                            BG_SELECTION
                        } else {
                            BG_FIELD
                        })
                        .stroke(Stroke::new(
                            1.0,
                            if state.context_include_selected {
                                ACCENT_BLUE
                            } else {
                                BORDER_PANEL
                            },
                        ))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(Margin::symmetric(6, 2))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(format!("@{sel_name}"))
                                    .font(font_mono(10.5))
                                    .color(if state.context_include_selected {
                                        TEXT_BLUE_CHIP
                                    } else {
                                        TEXT_MUTED
                                    }),
                            );
                        })
                        .response
                        .interact(Sense::click());
                    if sel_chip_resp.clicked() {
                        state.context_include_selected = !state.context_include_selected;
                    }

                    let scene_chip_resp = Frame::NONE
                        .fill(if state.context_include_scene {
                            BG_MENU_HOVER
                        } else {
                            BG_FIELD
                        })
                        .stroke(Stroke::new(
                            1.0,
                            if state.context_include_scene {
                                BORDER_CONTROL
                            } else {
                                BORDER_PANEL
                            },
                        ))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(Margin::symmetric(6, 2))
                        .show(ui, |ui| {
                            ui.label(RichText::new("@arena_01").font(font_mono(10.5)).color(
                                if state.context_include_scene {
                                    TEXT_CONTROL
                                } else {
                                    TEXT_MUTED
                                },
                            ));
                        })
                        .response
                        .interact(Sense::click());
                    if scene_chip_resp.clicked() {
                        state.context_include_scene = !state.context_include_scene;
                    }
                });

                ui.add_space(4.0);
                Frame::NONE
                    .fill(BG_FIELD)
                    .stroke(Stroke::new(1.0, BORDER_CONTROL))
                    .corner_radius(CornerRadius::same(RADIUS_WIDGET))
                    .inner_margin(Margin::symmetric(6, 3))
                    .show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            let send_w = 64.0;
                            let input_w = (ui.available_width() - send_w - 6.0).max(60.0);
                            let resp = ui.add_sized(
                                [input_w, 22.0],
                                egui::TextEdit::singleline(&mut state.agent_prompt)
                                    .frame(Frame::NONE)
                                    .font(font_sans(12.0))
                                    .hint_text(
                                        RichText::new("Ex. ajoute une rampe vers le pilier")
                                            .font(font_sans(12.0))
                                            .color(TEXT_SECONDARY),
                                    ),
                            );
                            let enter =
                                resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                            if CruftButton::new("agent_send_btn", "Envoyer")
                                .variant(ButtonVariant::Primary)
                                .min_size(Vec2::new(62.0, 24.0))
                                .show(ui)
                                .clicked()
                                || enter
                            {
                                submit_now = true;
                            }
                        });
                    });
            });
    });

    if submit_now {
        let prompt = std::mem::take(&mut state.agent_prompt);
        submit_agent_instruction(world, state, &prompt);
    }
}

fn draw_agent_log_entry(ui: &mut Ui, idx: usize, entry: &AgentLogEntry) -> (bool, bool) {
    let mut clicked_apply = false;
    let mut clicked_reject = false;
    let (status_txt, status_col) = match entry.status {
        ProposalState::Pending => ("en attente", ACCENT_YELLOW),
        ProposalState::Applied => ("appliqué", ACCENT_GREEN),
        ProposalState::Rejected => ("rejeté", ACCENT_RED),
    };

    Frame::NONE.inner_margin(Margin::same(10)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(
                RichText::new(&entry.time)
                    .font(font_mono(10.5))
                    .color(TEXT_MUTED),
            );
            ui.label(
                RichText::new(&entry.title)
                    .font(font_sans_bold(12.0))
                    .color(TEXT_PRIMARY),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(status_txt)
                        .font(font_mono(10.5))
                        .color(status_col),
                );
            });
        });

        ui.add_space(4.0);
        let diff_frame = Frame::NONE
            .fill(BG_TAB_BAR)
            .inner_margin(Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for line in &entry.diff_lines {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.label(
                            RichText::new(line.op.symbol())
                                .font(font_mono(10.5))
                                .color(line.op.color()),
                        );
                        ui.label(
                            RichText::new(&line.text)
                                .font(font_mono(10.5))
                                .color(TEXT_PRIMARY),
                        );
                    });
                }
            });
        let r = diff_frame.response.rect;
        ui.painter().line_segment(
            [r.left_top(), r.left_bottom()],
            Stroke::new(2.0, BORDER_CONTROL),
        );

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(&entry.tools)
                    .font(font_mono(10.0))
                    .color(TEXT_MUTED),
            );
            if entry.status == ProposalState::Pending {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if CruftButton::new(&format!("log_rej_{idx}"), "Rejeter")
                        .variant(ButtonVariant::Danger)
                        .min_size(Vec2::new(52.0, 22.0))
                        .show(ui)
                        .clicked()
                    {
                        clicked_reject = true;
                    }
                    if CruftButton::new(&format!("log_app_{idx}"), "Appliquer")
                        .variant(ButtonVariant::Primary)
                        .kbd("A")
                        .min_size(Vec2::new(72.0, 22.0))
                        .show(ui)
                        .clicked()
                    {
                        clicked_apply = true;
                    }
                });
            }
        });
    });

    let sep = ui.available_rect_before_wrap();
    ui.painter().line_segment(
        [sep.left_top(), sep.right_top()],
        Stroke::new(1.0, BORDER_PANEL),
    );

    (clicked_apply, clicked_reject)
}

// ---------------------------------------------------------------------------
// Ctrl+K Command Palette Overlay (Fuzzy Search over Commands & Scene Entities)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteAction {
    Undo,
    Redo,
    SpawnEntity,
    TogglePlay,
    ApplyProposal,
    RejectProposal,
    SetTool(TransformTool),
    OpenAssistant,
    SelectEntity(Entity, String),
    AskAssistant(String),
}

#[derive(Clone, Debug)]
pub struct PaletteMatch {
    pub label: String,
    pub shortcut: String,
    pub is_entity: bool,
    pub score: i32,
    pub action: PaletteAction,
}

/// Searches commands and scene entities in `world` using fuzzy matching (`fuzzy_score`).
pub fn search_palette(world: &World, query: &str) -> (Vec<PaletteMatch>, Vec<PaletteMatch>) {
    let command_defs = [
        ("Annuler la dernière action", "Ctrl Z", PaletteAction::Undo),
        ("Rétablir l'action", "Ctrl Shift Z", PaletteAction::Redo),
        ("Créer une entité", "Shift A", PaletteAction::SpawnEntity),
        ("Basculer le mode jeu", "Ctrl P", PaletteAction::TogglePlay),
        (
            "Appliquer la proposition de l'agent",
            "A",
            PaletteAction::ApplyProposal,
        ),
        (
            "Rejeter la proposition de l'agent",
            "Esc",
            PaletteAction::RejectProposal,
        ),
        (
            "Outil Déplacer (Translation)",
            "W",
            PaletteAction::SetTool(TransformTool::Move),
        ),
        (
            "Outil Pivoter (Rotation)",
            "E",
            PaletteAction::SetTool(TransformTool::Rotate),
        ),
        (
            "Outil Échelle (Scale)",
            "R",
            PaletteAction::SetTool(TransformTool::Scale),
        ),
        (
            "Ouvrir le panneau Assistant IA",
            "Agent",
            PaletteAction::OpenAssistant,
        ),
    ];

    let mut matched_commands: Vec<PaletteMatch> = command_defs
        .into_iter()
        .filter_map(|(label, shortcut, action)| {
            fuzzy_score(query, label).map(|score| PaletteMatch {
                label: label.to_owned(),
                shortcut: shortcut.to_owned(),
                is_entity: false,
                score,
                action,
            })
        })
        .collect();
    matched_commands.sort_by_key(|a| std::cmp::Reverse(a.score));
    matched_commands.truncate(4);

    let mut matched_entities: Vec<PaletteMatch> = world
        .iter_entities()
        .filter(|e| is_scene_entity(world, e.id()))
        .filter_map(|e| {
            let name = e
                .get::<Name>()
                .map(|n| n.as_str().to_owned())
                .unwrap_or_else(|| format!("Entity #{}", e.id().index()));
            fuzzy_score(query, &name).map(|score| PaletteMatch {
                label: name.clone(),
                shortcut: format!("#{}v{}", e.id().index(), e.id().generation()),
                is_entity: true,
                score,
                action: PaletteAction::SelectEntity(e.id(), name),
            })
        })
        .collect();
    matched_entities.sort_by_key(|a| (std::cmp::Reverse(a.score), a.label.clone()));
    matched_entities.truncate(4);

    (matched_commands, matched_entities)
}

fn execute_palette_action(world: &mut World, state: &mut EditorUiState, action: PaletteAction) {
    state.palette_open = false;
    match action {
        PaletteAction::Undo => {
            let _ = History::undo(world);
        }
        PaletteAction::Redo => {
            let _ = History::redo(world);
        }
        PaletteAction::SpawnEntity => {
            state.spawn_counter += 1;
            let name = format!("Entité.{}", state.spawn_counter);
            let _ = History::execute(
                world,
                SpawnEntity::new(name).with_component(Transform::default()),
            );
            world.resource_mut::<History>().finish_group();
        }
        PaletteAction::TogglePlay => toggle_play_mode(world, state),
        PaletteAction::ApplyProposal => apply_pending_proposal(world, state),
        PaletteAction::RejectProposal => reject_pending_proposal(world, state),
        PaletteAction::SetTool(t) => state.tool = t,
        PaletteAction::OpenAssistant => state.focus_tab(EditorTab::Assistant),
        PaletteAction::SelectEntity(entity, name) => {
            world.resource_mut::<Selection>().select_only(entity);
            state.focus_tab(EditorTab::Inspector);
            world.trigger(EditorEvent::Toast(format!("Entité sélectionnée : {name}")));
        }
        PaletteAction::AskAssistant(prompt) => {
            submit_agent_instruction(world, state, &prompt);
            state.focus_tab(EditorTab::Assistant);
        }
    }
}

fn draw_command_palette(ctx: &egui::Context, world: &mut World, state: &mut EditorUiState) {
    let screen = ctx.content_rect();
    let (matched_cmds, matched_ents) = search_palette(world, &state.palette_query);
    let total_matches = matched_cmds.len() + matched_ents.len();
    let total_rows = total_matches + 1; // +1 for "Demander à l'assistant"
    if state.palette_selected_idx >= total_rows {
        state.palette_selected_idx = 0;
    }

    let mut triggered_action: Option<PaletteAction> = None;

    egui::Area::new(Id::new("cruft_palette_backdrop"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let (bg_rect, bg_resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter()
                .rect_filled(bg_rect, CornerRadius::ZERO, Color32::from_black_alpha(140));

            let dynamic_h = (118.0 + (total_rows as f32) * 34.0 + 36.0).clamp(180.0, 390.0);
            let pal_w = 560.0_f32.min(screen.width() * 0.92);
            let pal_rect = Rect::from_min_size(
                Pos2::new(screen.center().x - pal_w * 0.5, screen.top() + 86.0),
                Vec2::new(pal_w, dynamic_h),
            );

            if bg_resp.clicked()
                && bg_resp
                    .interact_pointer_pos()
                    .is_some_and(|p| !pal_rect.contains(p))
            {
                state.palette_open = false;
            }

            let shadow = Shadow {
                offset: [0, 20],
                blur: 48,
                spread: 0,
                color: Color32::from_black_alpha(160),
            };
            ui.painter()
                .add(shadow.as_shape(pal_rect, CornerRadius::same(12)));
            ui.painter().rect(
                pal_rect,
                CornerRadius::same(12),
                BG_PANEL,
                Stroke::new(1.0, BORDER_KBD),
                StrokeKind::Inside,
            );

            ui.scope_builder(egui::UiBuilder::new().max_rect(pal_rect), |ui| {
                let head_rect =
                    Rect::from_min_size(pal_rect.min, Vec2::new(pal_rect.width(), 44.0));
                ui.painter().line_segment(
                    [head_rect.left_bottom(), head_rect.right_bottom()],
                    Stroke::new(1.0, BORDER_PANEL),
                );
                Frame::NONE
                    .inner_margin(Margin::symmetric(14, 11))
                    .show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            ui.label(
                                RichText::new("⌕")
                                    .font(font_mono(14.0))
                                    .color(TEXT_SECONDARY),
                            );
                            let prev_q = state.palette_query.clone();
                            let te = ui.add(
                                egui::TextEdit::singleline(&mut state.palette_query)
                                    .frame(Frame::NONE)
                                    .font(font_sans(14.0))
                                    .hint_text(
                                        RichText::new("Commande, entité ou demande à l'agent…")
                                            .font(font_sans(14.0))
                                            .color(TEXT_SECONDARY),
                                    )
                                    .desired_width(ui.available_width() - 40.0),
                            );
                            te.request_focus();
                            if state.palette_query != prev_q {
                                state.palette_selected_idx = 0;
                            }
                            kbd_badge(ui, "Esc");
                        });
                    });

                // Keyboard navigation inside the palette
                let (down, up, enter, tab) = ui.input(|i| {
                    (
                        i.key_pressed(Key::ArrowDown),
                        i.key_pressed(Key::ArrowUp),
                        i.key_pressed(Key::Enter),
                        i.key_pressed(Key::Tab),
                    )
                });
                if down {
                    state.palette_selected_idx = (state.palette_selected_idx + 1) % total_rows;
                } else if up {
                    state.palette_selected_idx =
                        (state.palette_selected_idx + total_rows - 1) % total_rows;
                }

                let mut flat_actions: Vec<PaletteAction> = Vec::with_capacity(total_rows);
                let mut cur_row = 0usize;

                Frame::NONE.inner_margin(Margin::same(6)).show(ui, |ui| {
                    if !matched_cmds.is_empty() {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add_space(10.0);
                            ui.label(
                                RichText::new("COMMANDES")
                                    .font(font_mono(10.0))
                                    .color(TEXT_SECONDARY),
                            );
                        });
                        for item in &matched_cmds {
                            let is_sel = state.palette_selected_idx == cur_row;
                            flat_actions.push(item.action.clone());
                            if draw_palette_row(ui, &item.label, &item.shortcut, is_sel, None)
                                .clicked()
                            {
                                triggered_action = Some(item.action.clone());
                            }
                            cur_row += 1;
                        }
                    }

                    if !matched_ents.is_empty() {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add_space(10.0);
                            ui.label(
                                RichText::new("ENTITÉS")
                                    .font(font_mono(10.0))
                                    .color(TEXT_SECONDARY),
                            );
                        });
                        for item in &matched_ents {
                            let is_sel = state.palette_selected_idx == cur_row;
                            flat_actions.push(item.action.clone());
                            if draw_palette_row(
                                ui,
                                &item.label,
                                &item.shortcut,
                                is_sel,
                                Some(ACCENT_BLUE_LIGHT),
                            )
                            .clicked()
                            {
                                triggered_action = Some(item.action.clone());
                            }
                            cur_row += 1;
                        }
                    }

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.add_space(10.0);
                        let cap_txt = if total_matches == 0 {
                            "AUCUN RÉSULTAT · DEMANDER À L'ASSISTANT"
                        } else {
                            "AGENT"
                        };
                        ui.label(RichText::new(cap_txt).font(font_mono(10.0)).color(
                            if total_matches == 0 {
                                ACCENT_YELLOW
                            } else {
                                TEXT_SECONDARY
                            },
                        ));
                    });

                    let prompt_preview = if state.palette_query.trim().is_empty() {
                        "Nouvelle instruction…"
                    } else {
                        state.palette_query.trim()
                    };
                    let ask_label = format!("Demander à l'assistant : « {prompt_preview} »");
                    let ask_action = PaletteAction::AskAssistant(state.palette_query.clone());
                    let is_ask_selected =
                        total_matches == 0 || state.palette_selected_idx == cur_row;
                    flat_actions.push(ask_action.clone());

                    let ask_kbd = if total_matches == 0 { "Entrée" } else { "Tab" };
                    if draw_palette_row(
                        ui,
                        &ask_label,
                        ask_kbd,
                        is_ask_selected,
                        Some(ACCENT_YELLOW),
                    )
                    .clicked()
                    {
                        triggered_action = Some(ask_action);
                    }
                });

                if tab {
                    triggered_action =
                        Some(PaletteAction::AskAssistant(state.palette_query.clone()));
                } else if enter
                    && let Some(act) = flat_actions
                        .get(state.palette_selected_idx)
                        .or_else(|| flat_actions.last())
                        .cloned()
                {
                    triggered_action = Some(act);
                }
            });
        });

    if let Some(action) = triggered_action {
        execute_palette_action(world, state, action);
    }
}

fn draw_palette_row(
    ui: &mut Ui,
    label: &str,
    shortcut: &str,
    highlighted: bool,
    prefix_arrow: Option<Color32>,
) -> egui::Response {
    let id = ui.make_persistent_id(("pal_row", label));
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let hover_t = ui
        .ctx()
        .animate_bool_with_time(id, resp.hovered(), theme::HOVER_ANIM_SECS);

    let bg = if highlighted {
        BG_SELECTION
    } else {
        lerp_color(Color32::TRANSPARENT, BG_BUTTON_HOVER, hover_t)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(RADIUS_WIDGET), bg);

    let mut text_x = rect.left() + 12.0;
    if let Some(arrow_col) = prefix_arrow {
        painter.text(
            Pos2::new(text_x, rect.center().y),
            Align2::LEFT_CENTER,
            "→",
            font_mono(12.5),
            arrow_col,
        );
        text_x += 18.0;
    }

    painter.text(
        Pos2::new(text_x, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font_sans(12.5),
        if highlighted || resp.hovered() {
            Color32::WHITE
        } else {
            TEXT_BODY
        },
    );

    let kbd_w = (shortcut.chars().count() as f32 * 6.5 + 12.0).max(24.0);
    let kbd_rect = Rect::from_min_max(
        Pos2::new(rect.right() - kbd_w - 10.0, rect.center().y - 8.0),
        Pos2::new(rect.right() - 10.0, rect.center().y + 8.0),
    );
    paint_kbd_in_rect(painter, kbd_rect, shortcut, false);

    resp
}

// ---------------------------------------------------------------------------
// File menu: new / open / save / save as (scenes live under assets/)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileDialogMode {
    Open,
    SaveAs,
}

#[derive(Clone, Debug)]
pub struct FileDialog {
    pub mode: FileDialogMode,
    pub path: String,
}

const DEFAULT_SCENE_PATH: &str = "scenes/sans_titre.scn.ron";

fn assets_root(world: &World) -> Option<std::path::PathBuf> {
    world
        .get_resource::<AssetCatalog>()
        .map(|catalog| catalog.root().to_path_buf())
}

fn checked_scene_path(world: &World, relative: &str) -> Result<std::path::PathBuf, String> {
    let path = std::path::Path::new(relative.trim());
    if relative.trim().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("Chemin relatif à assets/ attendu (ex. scenes/niveau.scn.ron)".into());
    }
    Ok(assets_root(world)
        .ok_or("Dossier assets/ inconnu")?
        .join(path))
}

/// Saves to `path` (or the current scene path; opens "Enregistrer sous" if none).
pub fn save_scene_action(world: &mut World, state: &mut EditorUiState, path: Option<String>) {
    let Some(relative) = path.or_else(|| state.scene_path.clone()) else {
        state.file_dialog = Some(FileDialog {
            mode: FileDialogMode::SaveAs,
            path: DEFAULT_SCENE_PATH.into(),
        });
        return;
    };
    let result = checked_scene_path(world, &relative).and_then(|full| {
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let entities = cruft_scene::scene_entities(world);
        cruft_scene::save_scene_file(world, entities, &full).map_err(|e| e.to_string())
    });
    match result {
        Ok(()) => {
            state.scene_path = Some(relative.clone());
            state.saved_undo_len = Some(world.resource::<History>().undo_len());
            if let Some(mut catalog) = world.get_resource_mut::<AssetCatalog>() {
                catalog.refresh();
            }
            world.trigger(EditorEvent::Toast(format!(
                "Scène enregistrée : {relative}"
            )));
        }
        Err(error) => world.trigger(EditorEvent::Toast(format!(
            "Échec de l'enregistrement : {error}"
        ))),
    }
}

/// Replaces the scene: removes every root entity then loads the file, as ONE
/// undoable command (`None` = new empty scene).
pub fn replace_scene_action(world: &mut World, state: &mut EditorUiState, path: Option<String>) {
    let ron = match &path {
        Some(relative) => match checked_scene_path(world, relative)
            .and_then(|full| std::fs::read_to_string(full).map_err(|e| e.to_string()))
        {
            Ok(ron) => Some(ron),
            Err(error) => {
                world.trigger(EditorEvent::Toast(format!(
                    "Ouverture impossible : {error}"
                )));
                return;
            }
        },
        None => None,
    };
    let label = match &path {
        Some(relative) => format!("Ouvrir {relative}"),
        None => "Nouvelle scène".to_owned(),
    };
    let mut command = cruft_core::CompositeCommand::new(label);
    for root in cruft_scene::scene_roots(world) {
        command.push(DespawnEntity::new(root));
    }
    if let Some(ron) = ron {
        command.push(cruft_scene::LoadScene::new(ron));
    }
    if command.is_empty() {
        state.scene_path = None;
        state.saved_undo_len = Some(world.resource::<History>().undo_len());
        return;
    }
    world.resource_mut::<History>().finish_group();
    if History::execute(world, command).is_ok() {
        world.resource_mut::<History>().finish_group();
        world.resource_mut::<Selection>().clear();
        state.scene_path = path;
        state.saved_undo_len = Some(world.resource::<History>().undo_len());
    }
}

fn draw_file_menu(button: &egui::Response, world: &mut World, state: &mut EditorUiState) {
    egui::Popup::menu(button).show(|ui| {
        ui.set_min_width(220.0);
        let item = |ui: &mut Ui, label: &str, shortcut: &str| {
            ui.add(egui::Button::new(label).shortcut_text(shortcut))
                .clicked()
        };
        if item(ui, "Nouvelle scène", "") {
            replace_scene_action(world, state, None);
        }
        if item(ui, "Ouvrir…", "") {
            state.file_dialog = Some(FileDialog {
                mode: FileDialogMode::Open,
                path: state.scene_path.clone().unwrap_or_default(),
            });
        }
        ui.separator();
        if item(ui, "Enregistrer", "Ctrl S") {
            save_scene_action(world, state, None);
        }
        if item(ui, "Enregistrer sous…", "") {
            state.file_dialog = Some(FileDialog {
                mode: FileDialogMode::SaveAs,
                path: state
                    .scene_path
                    .clone()
                    .unwrap_or_else(|| DEFAULT_SCENE_PATH.into()),
            });
        }
    });
}

fn draw_file_dialog(ctx: &egui::Context, world: &mut World, state: &mut EditorUiState) {
    let Some(mut dialog) = state.file_dialog.take() else {
        return;
    };
    let scenes: Vec<String> = world
        .get_resource::<AssetCatalog>()
        .map(|catalog| {
            catalog
                .entries()
                .iter()
                .filter(|entry| entry.path.ends_with(".ron") || entry.path.ends_with(".scn"))
                .map(|entry| entry.path.replace('\\', "/"))
                .collect()
        })
        .unwrap_or_default();
    let title = match dialog.mode {
        FileDialogMode::Open => "Ouvrir une scène",
        FileDialogMode::SaveAs => "Enregistrer la scène sous",
    };
    let mut keep_open = true;
    let mut confirm = false;
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .fixed_size(Vec2::new(380.0, 0.0))
        .show(ctx, |ui| {
            ui.label(
                RichText::new("Chemin relatif au dossier assets/")
                    .font(font_sans(11.0))
                    .color(TEXT_SECONDARY),
            );
            let edit = ui.add(
                egui::TextEdit::singleline(&mut dialog.path)
                    .font(font_mono(12.0))
                    .desired_width(f32::INFINITY),
            );
            if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                confirm = true;
            }
            if !scenes.is_empty() {
                ui.add_space(6.0);
                ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                    for scene in &scenes {
                        let selected = dialog.path == *scene;
                        let resp = ui
                            .selectable_label(selected, RichText::new(scene).font(font_mono(11.0)));
                        if resp.clicked() {
                            dialog.path.clone_from(scene);
                        }
                        if resp.double_clicked() {
                            confirm = true;
                        }
                    }
                });
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let label = match dialog.mode {
                    FileDialogMode::Open => "Ouvrir",
                    FileDialogMode::SaveAs => "Enregistrer",
                };
                if CruftButton::new("file_dialog_ok", label)
                    .variant(ButtonVariant::Primary)
                    .show(ui)
                    .clicked()
                {
                    confirm = true;
                }
                if CruftButton::new("file_dialog_cancel", "Annuler")
                    .show(ui)
                    .clicked()
                    || ui.input(|i| i.key_pressed(Key::Escape))
                {
                    keep_open = false;
                }
            });
        });
    if confirm {
        let path = dialog.path.trim().to_owned();
        match dialog.mode {
            FileDialogMode::Open => replace_scene_action(world, state, Some(path)),
            FileDialogMode::SaveAs => save_scene_action(world, state, Some(path)),
        }
    } else if keep_open {
        state.file_dialog = Some(dialog);
    }
}

// ---------------------------------------------------------------------------
// Display: UI zoom + "Fenêtre" menu (mode, resolution up to 4K, scales, VSync)
// ---------------------------------------------------------------------------

/// Applies the UI scale (OS scale × setting) and refreshes the status label.
fn sync_ui_zoom(ctx: &egui::Context, world: &mut World, state: &mut EditorUiState) {
    let mut windows = world.query_filtered::<&Window, With<bevy::window::PrimaryWindow>>();
    let Some((os_scale, width, height, mode)) = windows.iter(world).next().map(|w| {
        (
            w.scale_factor(),
            w.physical_width(),
            w.physical_height(),
            w.mode,
        )
    }) else {
        return;
    };
    let Some(settings) = world.get_resource::<DisplaySettings>() else {
        return;
    };
    let (zoom, capped) = settings.applied_ui_scale(os_scale, (width, height));
    if (ctx.zoom_factor() - zoom).abs() > 0.001 {
        ctx.set_zoom_factor(zoom);
    }
    let mode_label = match mode {
        bevy::window::WindowMode::Windowed => "",
        bevy::window::WindowMode::BorderlessFullscreen(_) => " · plein écran",
        bevy::window::WindowMode::Fullscreen(..) => " · exclusif",
    };
    let auto = if capped {
        " (limité par la fenêtre)"
    } else if settings.ui_scale.is_none() {
        " auto"
    } else {
        ""
    };
    state.display_label = format!(
        "{width}×{height}{mode_label} · UI {:.0} %{auto}",
        os_scale * zoom * 100.0
    );
}

fn draw_window_menu(button: &egui::Response, world: &mut World) {
    let monitor = display::primary_monitor_size(world);
    let (os_scale, window_height) = {
        let mut windows = world.query_filtered::<&Window, With<bevy::window::PrimaryWindow>>();
        windows
            .iter(world)
            .next()
            .map_or((1.0, 1080), |w| (w.scale_factor(), w.physical_height()))
    };
    let Some(mut settings) = world.get_resource::<DisplaySettings>().cloned() else {
        return;
    };
    let before = settings.clone();
    egui::Popup::menu(button)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(260.0);
            let heading = |ui: &mut Ui, text: &str| {
                ui.label(
                    RichText::new(text)
                        .font(font_sans_bold(11.0))
                        .color(TEXT_SECONDARY),
                );
            };

            heading(ui, "Mode d'affichage");
            for mode in [
                DisplayMode::Windowed,
                DisplayMode::Borderless,
                DisplayMode::Exclusive,
            ] {
                let shortcut = if mode == DisplayMode::Borderless {
                    "F11"
                } else {
                    ""
                };
                if ui
                    .add(
                        egui::Button::selectable(settings.mode == mode, mode.label())
                            .shortcut_text(shortcut),
                    )
                    .clicked()
                {
                    settings.mode = mode;
                }
            }

            ui.separator();
            heading(
                ui,
                match settings.mode {
                    DisplayMode::Windowed => "Taille de la fenêtre",
                    DisplayMode::Exclusive => "Résolution plein écran",
                    DisplayMode::Borderless => "Résolution (écran natif en plein écran)",
                },
            );
            ui.add_enabled_ui(settings.mode != DisplayMode::Borderless, |ui| {
                for (w, h, label) in display::RESOLUTIONS {
                    let too_big = monitor.is_some_and(|(mw, mh)| w > mw || h > mh);
                    let text = if too_big {
                        format!("{label}  (> écran)")
                    } else {
                        label.to_owned()
                    };
                    let resp = ui.add_enabled(
                        !(too_big && settings.mode == DisplayMode::Exclusive),
                        egui::Button::selectable(settings.resolution == (w, h), text)
                            .shortcut_text(format!("{w}×{h}")),
                    );
                    if resp.clicked() {
                        settings.resolution = (w, h);
                    }
                }
            });
            if let Some((mw, mh)) = monitor {
                ui.label(
                    RichText::new(format!("Écran principal : {mw}×{mh}"))
                        .font(font_mono(10.5))
                        .color(TEXT_MUTED),
                );
            }

            ui.separator();
            heading(ui, "Échelle de l'interface");
            let auto = display::auto_ui_scale(os_scale, window_height);
            if ui
                .add(
                    egui::Button::selectable(
                        settings.ui_scale.is_none(),
                        format!("Automatique ({:.0} %)", os_scale * auto * 100.0),
                    )
                    .shortcut_text("Ctrl 0"),
                )
                .clicked()
            {
                settings.ui_scale = None;
            }
            ui.horizontal_wrapped(|ui| {
                for scale in display::UI_SCALES {
                    let selected = settings.ui_scale.is_some_and(|s| (s - scale).abs() < 0.01);
                    if ui
                        .selectable_label(selected, format!("{:.0} %", scale * 100.0))
                        .clicked()
                    {
                        settings.ui_scale = Some(scale);
                    }
                }
            });
            ui.label(
                RichText::new("Ctrl + / Ctrl - pour ajuster")
                    .font(font_sans(10.5))
                    .color(TEXT_MUTED),
            );

            ui.separator();
            heading(ui, "Rendu du viewport");
            ui.horizontal_wrapped(|ui| {
                for scale in display::RENDER_SCALES {
                    let selected = (settings.viewport_render_scale - scale).abs() < 0.01;
                    if ui
                        .selectable_label(selected, format!("{:.0} %", scale * 100.0))
                        .on_hover_text("Taille de l'image 3D par rapport au panneau (plus bas = plus rapide en 4K)")
                        .clicked()
                    {
                        settings.viewport_render_scale = scale;
                    }
                }
            });
            ui.checkbox(&mut settings.vsync, "Synchronisation verticale (VSync)");
        });
    if settings != before {
        world.insert_resource(settings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cruft_core::CruftCorePlugin;

    #[test]
    fn egui_shortcut_undo_works_when_bevy_reports_the_same_key() {
        let mut app = App::new();
        app.add_plugins(CruftCorePlugin);
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::ControlLeft);
        keys.press(KeyCode::KeyZ);
        app.insert_resource(keys);
        History::execute(app.world_mut(), SpawnEntity::new("undo me")).unwrap();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
                egui::Event::Key {
                    key: Key::Z,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::CTRL,
                },
                egui::Event::ModifiersChanged(egui::Modifiers::NONE),
            ],
            ..default()
        };
        let mut state = EditorUiState::default();
        ctx.run_ui(input, |ui| {
            handle_shortcuts(ui.ctx(), app.world_mut(), &mut state)
        })
        .textures_delta
        .clear();
        assert_eq!(app.world().resource::<History>().undo_len(), 0);
        assert_eq!(app.world().resource::<History>().redo_len(), 1);
    }

    #[test]
    fn custom_fonts_render_after_the_initial_configuration_pass() {
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |ui| theme::apply_theme(ui.ctx()))
            .textures_delta
            .clear();
        ctx.run_ui(egui::RawInput::default(), |root| {
            egui::CentralPanel::default().show(root, |ui| {
                ui.label(RichText::new("Cruft").font(font_sans_bold(12.0)));
            });
        })
        .textures_delta
        .clear();
    }

    #[test]
    fn theme_and_fonts_build_cleanly() {
        let fonts = theme::build_fonts();
        assert!(fonts.font_data.contains_key("Inter-Regular"));
        assert!(fonts.font_data.contains_key("Inter-Bold"));
        assert!(fonts.font_data.contains_key("JetBrainsMono-Regular"));

        let style = theme::build_style();
        assert!(style.visuals.dark_mode);
        assert_eq!(style.visuals.panel_fill, theme::BG_PANEL);

        let dock_style = theme::build_dock_style(&style);
        assert_eq!(dock_style.tab_bar.bg_fill, theme::BG_TAB_BAR);
        assert_eq!(dock_style.tab.active.outline_color, theme::ACCENT_BLUE);
    }

    #[test]
    fn default_dock_layout_contains_all_editor_tabs() {
        let mut state = EditorUiState::default();
        for tab in [
            EditorTab::Outliner,
            EditorTab::Layers,
            EditorTab::Viewport,
            EditorTab::Assets,
            EditorTab::Console,
            EditorTab::Timeline,
            EditorTab::Inspector,
            EditorTab::Assistant,
        ] {
            assert!(
                state.dock_state.find_tab(&tab).is_some(),
                "missing tab {tab:?}"
            );
            state.focus_tab(tab);
        }
    }

    #[test]
    fn hierarchy_reflects_world_reparent_rename_and_undo_redo() {
        let mut app = App::new();
        app.add_plugins(CruftCorePlugin)
            .init_resource::<ToastQueue>()
            .init_resource::<EditorUiState>()
            .add_observer(feedback::collect_toasts);

        let world = app.world_mut();
        let parent = world
            .spawn((
                cruft_core::SceneEntity,
                Name::new("Parent"),
                Transform::default(),
            ))
            .id();
        let child = world
            .spawn((
                cruft_core::SceneEntity,
                Name::new("Enfant"),
                Transform::from_xyz(1.0, 2.0, 3.0),
            ))
            .id();

        // 1. Reparent child under parent (drag & drop command)
        History::execute(world, Reparent::new(child, Some(parent))).unwrap();
        world.resource_mut::<History>().finish_group();

        let (items, total) = collect_hierarchy(world, &EntityHashSet::default(), "");
        assert_eq!(total, 2);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].entity, parent);
        assert_eq!(items[0].depth, 0);
        assert!(items[0].has_children);
        assert_eq!(items[1].entity, child);
        assert_eq!(items[1].depth, 1);

        // 2. Rename child via SetComponentField (double-click rename)
        rename_entity_command(world, child, "Enfant renommé");
        assert_eq!(world.get::<Name>(child).unwrap().as_str(), "Enfant renommé");

        // 3. Undo rename & undo reparent
        History::undo(world).unwrap();
        assert_eq!(world.get::<Name>(child).unwrap().as_str(), "Enfant");
        History::undo(world).unwrap();
        assert!(world.get::<ChildOf>(child).is_none());

        // 4. Redo reparent & redo rename
        History::redo(world).unwrap();
        assert_eq!(world.get::<ChildOf>(child).unwrap().parent(), parent);
        History::redo(world).unwrap();
        assert_eq!(world.get::<Name>(child).unwrap().as_str(), "Enfant renommé");

        // 5. Toasts were collected on EditorEvent::Toast
        let mut state = EditorUiState::default();
        drain_toast_queue(world, &mut state);
        assert!(state.active_toast.is_some());
    }

    #[test]
    fn viewport_picking_and_gizmo_drag_merge_into_single_undo_entry() {
        let mut app = App::new();
        app.add_plugins(CruftCorePlugin)
            .init_resource::<Assets<Mesh>>();
        let world = app.world_mut();

        let cube_mesh = world
            .resource_mut::<Assets<Mesh>>()
            .add(Cuboid::new(2.0, 2.0, 2.0));
        let cube = world
            .spawn((
                cruft_core::SceneEntity,
                Name::new("Cube"),
                Mesh3d(cube_mesh),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();
        world.spawn((
            EditorViewportCamera,
            Camera3d::default(),
            Projection::default(),
            Transform::from_xyz(0.0, 0.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y),
        ));

        let canvas_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));

        // 1. Clicking at the center of the viewport picks the Cube
        let picked = pick_entity_in_viewport(world, canvas_rect, canvas_rect.center());
        assert_eq!(picked, Some(cube));
        world.resource_mut::<Selection>().select_only(cube);
        assert_eq!(world.resource::<Selection>().primary(), Some(cube));

        // 2. Simulate a multi-step W (Move) gizmo drag along X
        let drag = ActiveGizmoDrag {
            entity: cube,
            tool: TransformTool::Move,
            axis: 0,
        };
        for _ in 0..5 {
            let tr = *world.get::<Transform>(cube).unwrap();
            apply_gizmo_drag_step(
                world,
                drag,
                tr,
                Pos2::new(400.0, 300.0),
                Pos2::new(480.0, 300.0),
                1.6,
                Vec2::new(16.0, 0.0),
            );
        }
        world.resource_mut::<History>().finish_group();

        // All 5 drag steps merged into 1 single History entry!
        assert_eq!(world.resource::<History>().undo_len(), 1);
        assert!(world.get::<Transform>(cube).unwrap().translation.x > 1.0);

        // Undo restores the initial position in 1 step
        History::undo(world).unwrap();
        assert!(world.get::<Transform>(cube).unwrap().translation.x.abs() < 1e-4);
    }

    #[test]
    fn command_palette_fuzzy_matches_commands_entities_and_falls_back_to_assistant() {
        let mut app = App::new();
        app.add_plugins(CruftCorePlugin);
        let world = app.world_mut();
        let pilier = world
            .spawn((
                cruft_core::SceneEntity,
                Name::new("Pilier central"),
                Transform::default(),
            ))
            .id();
        world.spawn((
            cruft_core::SceneEntity,
            Name::new("Cube bleu"),
            Transform::default(),
        ));

        // 1. Fuzzy match on entity ("pilr" -> "Pilier central")
        let (cmds, ents) = search_palette(world, "pilr");
        assert!(cmds.is_empty());
        assert_eq!(ents.len(), 1);
        assert_eq!(
            ents[0].action,
            PaletteAction::SelectEntity(pilier, "Pilier central".into())
        );

        // 2. Fuzzy match on command ("annul" -> "Annuler la dernière action")
        let (cmds, _ents) = search_palette(world, "annul");
        assert!(!cmds.is_empty());
        assert_eq!(cmds[0].action, PaletteAction::Undo);

        // 3. Unmatched query ("ajoute une rampe vers le toit") -> 0 matches -> falls back to "Demander à l'assistant"
        let (cmds, ents) = search_palette(world, "ajoute une rampe vers le toit");
        assert!(cmds.is_empty());
        assert!(ents.is_empty());
    }

    #[test]
    fn agent_proposal_apply_and_undo_is_single_history_entry() {
        use cruft_core::ProposedChange;
        let mut app = App::new();
        app.add_plugins(CruftCorePlugin)
            .init_resource::<ToastQueue>()
            .add_observer(feedback::collect_toasts);
        let world = app.world_mut();
        let mut state = EditorUiState::default();
        sync_agent_state(world, &mut state);
        assert_ne!(state.proposal, ProposalState::Pending);
        assert!(state.agent_log.is_empty(), "no fake journal entries");

        let spawn = |name: &str, x: f32| ProposedChange::Spawn {
            name: name.into(),
            parent: None,
            components: serde_json::json!({"Transform": {"translation": [x, 0.5, 0.0]}})
                .as_object()
                .unwrap()
                .clone(),
            primitive: None,
            color: None,
        };
        cruft_core::proposals::propose(
            world,
            "Ajouter 3 couverts",
            "mcp",
            vec![
                spawn("Cover_A", 1.0),
                spawn("Cover_B", 2.0),
                spawn("Cover_C", 3.0),
            ],
        )
        .unwrap();
        sync_agent_state(world, &mut state);
        assert_eq!(state.proposal, ProposalState::Pending);
        assert_eq!(state.pending_ghosts.len(), 3);
        assert_eq!(state.pending_ghosts[1].position.x, 2.0);
        assert_eq!(state.agent_log.last().unwrap().diff_lines.len(), 3);
        assert_eq!(
            world.query::<&Name>().iter(world).count(),
            0,
            "proposal is not applied"
        );

        apply_pending_proposal(world, &mut state);
        assert_eq!(state.proposal, ProposalState::Applied);
        assert!(state.pending_ghosts.is_empty());
        assert_eq!(world.resource::<History>().undo_len(), 1);
        assert_eq!(world.query::<&Name>().iter(world).count(), 3);

        // Undoing reverts all 3 proposed entities in ONE step
        History::undo(world).unwrap();
        assert_eq!(world.query::<&Name>().iter(world).count(), 0);

        cruft_core::proposals::propose(world, "Tourelle", "mcp", vec![spawn("Tourelle", 0.0)])
            .unwrap();
        sync_agent_state(world, &mut state);
        reject_pending_proposal(world, &mut state);
        assert_eq!(state.proposal, ProposalState::Rejected);
        assert_eq!(world.query::<&Name>().iter(world).count(), 0);

        // Instructions are copied for the external MCP client, never faked.
        submit_agent_instruction(world, &mut state, "Ajoute une rampe");
        assert!(
            state
                .clipboard_request
                .as_ref()
                .unwrap()
                .contains("Ajoute une rampe")
        );
        assert_eq!(state.agent_log.len(), 2);
    }

    #[test]
    fn deleting_a_multi_selection_is_one_undo_entry_and_skips_selected_descendants() {
        let mut app = App::new();
        app.add_plugins(CruftCorePlugin);
        let world = app.world_mut();
        let parent = world
            .spawn((
                cruft_core::SceneEntity,
                Name::new("A"),
                Transform::default(),
            ))
            .id();
        let child = world
            .spawn((
                cruft_core::SceneEntity,
                Name::new("A1"),
                Transform::default(),
                ChildOf(parent),
            ))
            .id();
        let other = world
            .spawn((
                cruft_core::SceneEntity,
                Name::new("B"),
                Transform::default(),
            ))
            .id();
        {
            let mut selection = world.resource_mut::<Selection>();
            selection.select_only(child);
            selection.add(parent);
            selection.add(other);
        }
        delete_selection(world);
        assert_eq!(world.query::<&Name>().iter(world).count(), 0);
        assert_eq!(world.resource::<History>().undo_len(), 1);
        History::undo(world).unwrap();
        assert_eq!(world.query::<&Name>().iter(world).count(), 3);
    }
}
