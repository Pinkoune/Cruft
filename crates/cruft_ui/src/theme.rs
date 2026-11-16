//! Cruft design system: color palette, typography, `egui::Style`/`Visuals`, and `egui_dock::Style`.
//!
//! All tokens follow `AGENTS.md` and `cruft-mockup.dc.html` (Rust official palette adapted for dark UI).

use bevy_egui::egui::{
    self, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily, FontId, Margin,
    Shadow, Stroke, Style, TextStyle, Vec2, Visuals,
    style::{Selection, WidgetVisuals, Widgets},
};
use std::sync::Arc;

/// Embedded UI proportional font (`Inter-Regular.ttf`, Fira Sans metrics).
pub const FONT_INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
/// Embedded UI bold font (`Inter-Bold.ttf`, Fira Sans Bold metrics).
pub const FONT_INTER_BOLD: &[u8] = include_bytes!("../assets/fonts/Inter-Bold.ttf");
/// Embedded monospace font (`JetBrainsMono-Regular.ttf`, Fira Mono Medium metrics).
pub const FONT_JETBRAINS_MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");

// ---------------------------------------------------------------------------
// Surface & Background Tokens
// ---------------------------------------------------------------------------

/// App background `#090D10` (quasi-noir bleuté).
pub const BG_APP: Color32 = Color32::from_rgb(0x09, 0x0D, 0x10);
/// Panel body background `#141B1F`.
pub const BG_PANEL: Color32 = Color32::from_rgb(0x14, 0x1B, 0x1F);
/// Tab bar / header / status bar background `#0F1518`.
pub const BG_TAB_BAR: Color32 = Color32::from_rgb(0x0F, 0x15, 0x18);
/// Field / input / segmented control background `#0B1013`.
pub const BG_FIELD: Color32 = Color32::from_rgb(0x0B, 0x10, 0x13);
/// Hovered numeric field background `#0E1316`.
pub const BG_FIELD_HOVER: Color32 = Color32::from_rgb(0x0E, 0x13, 0x16);
/// Active / dragged numeric field background `#10202A`.
pub const BG_FIELD_ACTIVE: Color32 = Color32::from_rgb(0x10, 0x20, 0x2A);

/// Standard button background `#1B2428`.
pub const BG_BUTTON: Color32 = Color32::from_rgb(0x1B, 0x24, 0x28);
/// Hovered button / icon button background `#243035`.
pub const BG_BUTTON_HOVER: Color32 = Color32::from_rgb(0x24, 0x30, 0x35);
/// Menu hover / tag pill background `#26323A`.
pub const BG_MENU_HOVER: Color32 = Color32::from_rgb(0x26, 0x32, 0x3A);
/// Pressed button background `#36444A`.
pub const BG_BUTTON_ACTIVE: Color32 = Color32::from_rgb(0x36, 0x44, 0x4A);
/// Hovered outliner row background `#1A2327`.
pub const BG_ROW_HOVER: Color32 = Color32::from_rgb(0x1A, 0x23, 0x27);

// ---------------------------------------------------------------------------
// Border & Relief Tokens
// ---------------------------------------------------------------------------

/// Panel and field border `#243035`.
pub const BORDER_PANEL: Color32 = Color32::from_rgb(0x24, 0x30, 0x35);
/// Button / control border `#36444A`.
pub const BORDER_CONTROL: Color32 = Color32::from_rgb(0x36, 0x44, 0x4A);
/// Kbd / modal border `#3E4C53`.
pub const BORDER_KBD: Color32 = Color32::from_rgb(0x3E, 0x4C, 0x53);
/// Hovered field / button border `#4B5B63`.
pub const BORDER_HOVER: Color32 = Color32::from_rgb(0x4B, 0x5B, 0x63);
/// Subtle 1px top highlight on panels `rgba(255, 255, 255, 0.035)`.
pub const RELIEF_TOP_HIGHLIGHT: Color32 = Color32::from_rgba_premultiplied(9, 9, 9, 9);

// ---------------------------------------------------------------------------
// Text Tokens
// ---------------------------------------------------------------------------

/// Primary text `#E8ECEE`.
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xE8, 0xEC, 0xEE);
/// Row / body text `#D3DADD`.
pub const TEXT_BODY: Color32 = Color32::from_rgb(0xD3, 0xDA, 0xDD);
/// Control / icon button text `#BCC7CC`.
pub const TEXT_CONTROL: Color32 = Color32::from_rgb(0xBC, 0xC7, 0xCC);
/// Secondary text `#A3B0B6`.
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0xA3, 0xB0, 0xB6);
/// Muted / tag / timestamp text `#8E9CA3`.
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x8E, 0x9C, 0xA3);
/// Dark text on bright accent backgrounds `#0B1418`.
pub const TEXT_ON_ACCENT: Color32 = Color32::from_rgb(0x0B, 0x14, 0x18);

// ---------------------------------------------------------------------------
// Semantic & Axis Accent Tokens (Rust Palette)
// ---------------------------------------------------------------------------

/// Interaction blue `#4299BF` (selection, active tab, active tool, focus ring, sliders, Z axis).
pub const ACCENT_BLUE: Color32 = Color32::from_rgb(0x42, 0x99, 0xBF);
/// Selection background `#1E3A48`.
pub const BG_SELECTION: Color32 = Color32::from_rgb(0x1E, 0x3A, 0x48);
/// Selection flash start `#2A5A70`.
pub const BG_SELECTION_FLASH: Color32 = Color32::from_rgb(0x2A, 0x5A, 0x70);
/// Info / camera / mesh icon blue `#7FC4E0`.
pub const ACCENT_BLUE_LIGHT: Color32 = Color32::from_rgb(0x7F, 0xC4, 0xE0);
/// Context chip text `#BFE3F2`.
pub const TEXT_BLUE_CHIP: Color32 = Color32::from_rgb(0xBF, 0xE3, 0xF2);

/// Healthy / Play mode green fill `#0B7261`.
pub const ACCENT_GREEN_FILL: Color32 = Color32::from_rgb(0x0B, 0x72, 0x61);
/// Healthy / Play mode stroke & Y axis `#3FBF9F`.
pub const ACCENT_GREEN: Color32 = Color32::from_rgb(0x3F, 0xBF, 0x9F);

/// Destructive / alert red fill `#A72145`.
pub const ACCENT_RED_FILL: Color32 = Color32::from_rgb(0xA7, 0x21, 0x45);
/// Destructive / alert text, unsaved indicator & X axis `#E0567A`.
pub const ACCENT_RED: Color32 = Color32::from_rgb(0xE0, 0x56, 0x7A);
/// Danger button idle text `#F08AA5`.
pub const TEXT_DANGER: Color32 = Color32::from_rgb(0xF0, 0x8A, 0xA5);
/// Danger button hover text `#FFC2D2`.
pub const TEXT_DANGER_HOVER: Color32 = Color32::from_rgb(0xFF, 0xC2, 0xD2);
/// Danger button hover fill `#3A1520`.
pub const BG_DANGER_HOVER: Color32 = Color32::from_rgb(0x3A, 0x15, 0x20);
/// Logo anvil red `#C8344F`.
pub const LOGO_RED: Color32 = Color32::from_rgb(0xC8, 0x34, 0x4F);

/// Pending / agent proposal yellow `#FFC832`.
pub const ACCENT_YELLOW: Color32 = Color32::from_rgb(0xFF, 0xC8, 0x32);
/// Primary yellow button hover `#FFD65C`.
pub const ACCENT_YELLOW_HOVER: Color32 = Color32::from_rgb(0xFF, 0xD6, 0x5C);
/// Agent / MCP text yellow `#FFD970`.
pub const TEXT_YELLOW: Color32 = Color32::from_rgb(0xFF, 0xD9, 0x70);
/// Agent header warm yellow `#FFE7A3`.
pub const TEXT_YELLOW_LIGHT: Color32 = Color32::from_rgb(0xFF, 0xE7, 0xA3);
/// Dark text on yellow primary button `#1E1600`.
pub const TEXT_ON_YELLOW: Color32 = Color32::from_rgb(0x1E, 0x16, 0x00);
/// Agent badge background `#221E12`.
pub const BG_AGENT_BADGE: Color32 = Color32::from_rgb(0x22, 0x1E, 0x12);
/// Agent badge / proposal card border `#4D4020`.
pub const BORDER_AGENT: Color32 = Color32::from_rgb(0x4D, 0x40, 0x20);

/// X axis color (`#E0567A`).
pub const AXIS_X: Color32 = ACCENT_RED;
/// Y axis color (`#3FBF9F`).
pub const AXIS_Y: Color32 = ACCENT_GREEN;
/// Z axis color (`#4299BF`).
pub const AXIS_Z: Color32 = ACCENT_BLUE;

// ---------------------------------------------------------------------------
// Radii & Spacing Constants
// ---------------------------------------------------------------------------

/// Gap between docked panels (`6 px`).
pub const PANEL_GAP: f32 = 6.0;
/// Panel corner radius (`10 px`).
pub const RADIUS_PANEL: u8 = 10;
/// Field / button corner radius (`6 px`).
pub const RADIUS_WIDGET: u8 = 6;
/// Toast / floating card corner radius (`9 px`).
pub const RADIUS_CARD: u8 = 9;
/// Hover animation duration (`~120 ms` as specified in `AGENTS.md`).
pub const HOVER_ANIM_SECS: f32 = 0.12;
/// Press animation duration (`~60 ms`).
pub const PRESS_ANIM_SECS: f32 = 0.06;

/// Returns a proportional [`FontId`] of the given size.
#[inline]
pub fn font_sans(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// Returns a bold proportional [`FontId`] of the given size.
#[inline]
pub fn font_sans_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("Inter-Bold".into()))
}

/// Returns a monospace [`FontId`] of the given size.
#[inline]
pub fn font_mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// Builds the embedded [`FontDefinitions`] with Inter (Fira Sans) and JetBrains Mono (Fira Mono).
pub fn build_fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();

    fonts.font_data.insert(
        "Inter-Regular".to_owned(),
        Arc::new(FontData::from_static(FONT_INTER_REGULAR)),
    );
    fonts.font_data.insert(
        "Inter-Bold".to_owned(),
        Arc::new(FontData::from_static(FONT_INTER_BOLD)),
    );
    fonts.font_data.insert(
        "JetBrainsMono-Regular".to_owned(),
        Arc::new(FontData::from_static(FONT_JETBRAINS_MONO)),
    );

    if let Some(proportional) = fonts.families.get_mut(&FontFamily::Proportional) {
        proportional.insert(0, "Inter-Regular".to_owned());
    }
    if let Some(monospace) = fonts.families.get_mut(&FontFamily::Monospace) {
        monospace.insert(0, "JetBrainsMono-Regular".to_owned());
    }

    fonts.families.insert(
        FontFamily::Name("Inter".into()),
        vec!["Inter-Regular".to_owned()],
    );
    fonts.families.insert(
        FontFamily::Name("Inter-Bold".into()),
        vec!["Inter-Bold".to_owned(), "Inter-Regular".to_owned()],
    );
    fonts.families.insert(
        FontFamily::Name("JetBrainsMono".into()),
        vec!["JetBrainsMono-Regular".to_owned()],
    );
    fonts.families.insert(
        FontFamily::Name("FiraSans".into()),
        vec!["Inter-Regular".to_owned()],
    );
    fonts.families.insert(
        FontFamily::Name("FiraMono".into()),
        vec!["JetBrainsMono-Regular".to_owned()],
    );

    fonts
}

/// Builds the Cruft [`Visuals`] matching all tokens from `AGENTS.md`.
pub fn build_visuals() -> Visuals {
    let mut visuals = Visuals::dark();

    visuals.dark_mode = true;
    visuals.override_text_color = None;
    visuals.weak_text_color = Some(TEXT_SECONDARY);
    visuals.hyperlink_color = ACCENT_BLUE_LIGHT;
    visuals.faint_bg_color = BG_ROW_HOVER;
    visuals.extreme_bg_color = BG_FIELD;
    visuals.text_edit_bg_color = Some(BG_FIELD);
    visuals.code_bg_color = BG_FIELD;
    visuals.warn_fg_color = ACCENT_YELLOW;
    visuals.error_fg_color = ACCENT_RED;

    visuals.panel_fill = BG_PANEL;
    visuals.window_fill = BG_PANEL;
    visuals.window_stroke = Stroke::new(1.0, BORDER_PANEL);
    visuals.window_corner_radius = CornerRadius::same(RADIUS_PANEL);
    visuals.menu_corner_radius = CornerRadius::same(RADIUS_WIDGET);

    let panel_shadow = Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(90),
    };
    visuals.window_shadow = panel_shadow;
    visuals.popup_shadow = Shadow {
        offset: [0, 10],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(130),
    };

    visuals.selection = Selection {
        bg_fill: BG_SELECTION,
        stroke: Stroke::new(1.5, ACCENT_BLUE),
    };

    visuals.widgets = Widgets {
        noninteractive: WidgetVisuals {
            weak_bg_fill: BG_PANEL,
            bg_fill: BG_PANEL,
            bg_stroke: Stroke::new(1.0, BORDER_PANEL),
            fg_stroke: Stroke::new(1.0, TEXT_PRIMARY),
            corner_radius: CornerRadius::same(RADIUS_WIDGET),
            expansion: 0.0,
        },
        inactive: WidgetVisuals {
            weak_bg_fill: BG_BUTTON,
            bg_fill: BG_FIELD,
            bg_stroke: Stroke::new(1.0, BORDER_CONTROL),
            fg_stroke: Stroke::new(1.0, TEXT_PRIMARY),
            corner_radius: CornerRadius::same(RADIUS_WIDGET),
            expansion: 0.0,
        },
        hovered: WidgetVisuals {
            weak_bg_fill: BG_BUTTON_HOVER,
            bg_fill: BG_FIELD_HOVER,
            bg_stroke: Stroke::new(1.0, BORDER_HOVER),
            fg_stroke: Stroke::new(1.0, Color32::WHITE),
            corner_radius: CornerRadius::same(RADIUS_WIDGET),
            expansion: 0.0,
        },
        active: WidgetVisuals {
            weak_bg_fill: BG_BUTTON_ACTIVE,
            bg_fill: BG_FIELD_ACTIVE,
            bg_stroke: Stroke::new(1.5, ACCENT_BLUE),
            fg_stroke: Stroke::new(1.0, Color32::WHITE),
            corner_radius: CornerRadius::same(RADIUS_WIDGET),
            expansion: 0.0,
        },
        open: WidgetVisuals {
            weak_bg_fill: BG_MENU_HOVER,
            bg_fill: BG_MENU_HOVER,
            bg_stroke: Stroke::new(1.0, ACCENT_BLUE),
            fg_stroke: Stroke::new(1.0, Color32::WHITE),
            corner_radius: CornerRadius::same(RADIUS_WIDGET),
            expansion: 0.0,
        },
    };

    visuals.slider_trailing_fill = true;
    visuals.interact_cursor = Some(CursorIcon::PointingHand);
    visuals.text_cursor.stroke = Stroke::new(1.5, ACCENT_BLUE);

    visuals
}

/// Builds the complete [`egui::Style`] for the Cruft editor.
pub fn build_style() -> Style {
    let mut style = Style {
        visuals: build_visuals(),
        animation_time: HOVER_ANIM_SECS,
        ..Default::default()
    };

    style.spacing.item_spacing = Vec2::new(6.0, 5.0);
    style.spacing.window_margin = Margin::same(8);
    style.spacing.button_padding = Vec2::new(8.0, 4.0);
    style.spacing.menu_margin = Margin::same(6);
    style.spacing.indent = 16.0;
    style.spacing.interact_size = Vec2::new(24.0, 24.0);
    style.spacing.scroll.bar_width = 6.0;
    style.spacing.scroll.floating = true;

    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(13.5, FontFamily::Name("Inter-Bold".into())),
    );
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(12.0, FontFamily::Proportional));
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(11.5, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(10.5, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(11.0, FontFamily::Monospace),
    );

    style
}

/// Applies fonts, visuals, and style to an [`egui::Context`].
pub fn apply_theme(ctx: &egui::Context) {
    ctx.set_fonts(build_fonts());
    let style = Arc::new(build_style());
    ctx.set_visuals(style.visuals.clone());
    ctx.set_style_of(egui::Theme::Dark, Arc::clone(&style));
    ctx.set_style_of(egui::Theme::Light, style);
}

/// Builds the [`egui_dock::Style`] matching the Cruft mockup:
/// - `6 px` `#090D10` gap between docked panels
/// - `#0F1518` tab bars (`28 px` height) and `#141B1F` panel bodies with `#243035` border and `10 px` rounding
/// - Active tab has `#141B1F` fill, `#E8ECEE` text, and `#4299BF` bottom indicator
pub fn build_dock_style(egui_style: &Style) -> egui_dock::Style {
    let mut style = egui_dock::Style::from_egui(egui_style);

    style.dock_area_padding = Some(Margin::same(PANEL_GAP as i8));
    style.main_surface_border_stroke = Stroke::NONE;
    style.main_surface_border_rounding = CornerRadius::same(RADIUS_PANEL);

    style.separator.width = PANEL_GAP;
    style.separator.extra_interact_width = 4.0;
    style.separator.color_idle = BG_APP;
    style.separator.color_hovered = BORDER_PANEL;
    style.separator.color_dragged = ACCENT_BLUE;

    style.tab_bar.bg_fill = BG_TAB_BAR;
    style.tab_bar.height = 28.0;
    style.tab_bar.inner_margin = Margin::ZERO;
    style.tab_bar.corner_radius = CornerRadius {
        nw: RADIUS_PANEL,
        ne: RADIUS_PANEL,
        sw: 0,
        se: 0,
    };
    style.tab_bar.hline_color = BORDER_PANEL;
    style.tab_bar.fill_tab_bar = false;

    let tab_corner = CornerRadius::ZERO;
    style.tab.spacing = 0.0;
    style.tab.hline_below_active_tab_name = true;

    style.tab.active.bg_fill = BG_PANEL;
    style.tab.active.text_color = TEXT_PRIMARY;
    style.tab.active.outline_color = ACCENT_BLUE;
    style.tab.active.corner_radius = tab_corner;

    style.tab.focused = style.tab.active.clone();
    style.tab.active_with_kb_focus = style.tab.active.clone();
    style.tab.focused_with_kb_focus = style.tab.active.clone();

    style.tab.inactive.bg_fill = BG_TAB_BAR;
    style.tab.inactive.text_color = TEXT_SECONDARY;
    style.tab.inactive.outline_color = BORDER_PANEL;
    style.tab.inactive.corner_radius = tab_corner;
    style.tab.inactive_with_kb_focus = style.tab.inactive.clone();

    style.tab.hovered.bg_fill = BG_PANEL;
    style.tab.hovered.text_color = TEXT_PRIMARY;
    style.tab.hovered.outline_color = BORDER_HOVER;
    style.tab.hovered.corner_radius = tab_corner;

    style.tab.tab_body.inner_margin = Margin::ZERO;
    style.tab.tab_body.stroke = Stroke::new(1.0, BORDER_PANEL);
    style.tab.tab_body.corner_radius = CornerRadius {
        nw: 0,
        ne: 0,
        sw: RADIUS_PANEL,
        se: RADIUS_PANEL,
    };
    style.tab.tab_body.bg_fill = BG_PANEL;

    style.overlay.selection_color = Color32::from_rgba_unmultiplied(0x42, 0x99, 0xBF, 90);
    style
}

/// Linearly interpolates between two [`Color32`] values by `t` in `0.0..=1.0`.
#[inline]
pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let inv = 1.0 - t;
    Color32::from_rgba_unmultiplied(
        (a.r() as f32 * inv + b.r() as f32 * t).round() as u8,
        (a.g() as f32 * inv + b.g() as f32 * t).round() as u8,
        (a.b() as f32 * inv + b.b() as f32 * t).round() as u8,
        (a.a() as f32 * inv + b.a() as f32 * t).round() as u8,
    )
}
