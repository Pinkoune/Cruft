//! Custom animated widgets for the Cruft editor:
//! - Animated buttons (`CruftButton`, `animated_button`, `toggle_switch`, `kbd_badge`)
//! - Outliner / hierarchy row with colored icon, badge, collapse toggle, double-click rename, and drag & drop (`hierarchy_row`)
//! - Draggable `vec3_field` / `vec3_field_detailed` with colored X/Y/Z bars and labels, plus `draggable_scalar_field`
//! - Animated viewport toast (`animated_toast`)

use crate::theme::{
    ACCENT_BLUE, ACCENT_BLUE_LIGHT, ACCENT_GREEN, ACCENT_GREEN_FILL, ACCENT_RED_FILL,
    ACCENT_YELLOW, ACCENT_YELLOW_HOVER, AXIS_X, AXIS_Y, AXIS_Z, BG_AGENT_BADGE, BG_BUTTON,
    BG_BUTTON_ACTIVE, BG_BUTTON_HOVER, BG_DANGER_HOVER, BG_FIELD, BG_FIELD_ACTIVE, BG_FIELD_HOVER,
    BG_MENU_HOVER, BG_ROW_HOVER, BG_SELECTION, BG_SELECTION_FLASH, BORDER_AGENT, BORDER_CONTROL,
    BORDER_HOVER, BORDER_KBD, BORDER_PANEL, HOVER_ANIM_SECS, PRESS_ANIM_SECS, RADIUS_CARD,
    RADIUS_WIDGET, TEXT_BLUE_CHIP, TEXT_BODY, TEXT_CONTROL, TEXT_DANGER, TEXT_DANGER_HOVER,
    TEXT_MUTED, TEXT_ON_ACCENT, TEXT_ON_YELLOW, TEXT_PRIMARY, TEXT_SECONDARY, TEXT_YELLOW,
    font_mono, font_sans, font_sans_bold, lerp_color,
};
use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Frame, Id, Key, Pos2, Rect, Response, Sense,
    Shadow, Stroke, StrokeKind, Ui, Vec2,
};

// ---------------------------------------------------------------------------
// 1. Animated Button & Kbd Badge
// ---------------------------------------------------------------------------

/// Visual variant for [`CruftButton`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonVariant {
    /// Standard `.btn` (`#1B2428` -> `#243035`, border `#36444A` -> `#4B5B63`).
    Default,
    /// Primary yellow `.btn.pri` (`#FFC832` -> `#FFD65C`, dark text `#1E1600`).
    Primary,
    /// Destructive `.btn.danger` (red hover `#3A1520`, border `#A72145`).
    Danger,
    /// Compact icon / segmented button `.ib` (active uses `#4299BF` blue fill).
    Icon { active: bool },
    /// Top bar menu item `.menu`.
    Menu,
    /// Transport Play/Pause/Step button `.play` (active uses `#0B7261` green + glow).
    Play { active: bool },
}

/// Builder for a Cruft button with hover/press animations via `ctx.animate_bool`.
pub struct CruftButton<'a> {
    id_salt: &'a str,
    label: &'a str,
    variant: ButtonVariant,
    kbd: Option<&'a str>,
    tooltip: Option<&'a str>,
    min_size: Vec2,
    monospace: bool,
    bordered: bool,
}

impl<'a> CruftButton<'a> {
    /// Creates a new animated button with the given ID salt and label.
    pub fn new(id_salt: &'a str, label: &'a str) -> Self {
        Self {
            id_salt,
            label,
            variant: ButtonVariant::Default,
            kbd: None,
            tooltip: None,
            min_size: Vec2::new(24.0, 26.0),
            monospace: false,
            bordered: false,
        }
    }

    /// Sets the button visual variant.
    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        if matches!(
            variant,
            ButtonVariant::Icon { .. } | ButtonVariant::Menu | ButtonVariant::Play { .. }
        ) {
            self.min_size = Vec2::new(24.0, 24.0);
        }
        if matches!(variant, ButtonVariant::Play { .. }) {
            self.min_size = Vec2::new(30.0, 24.0);
        }
        self
    }

    /// Appends a `<kbd>` shortcut badge inside the button.
    pub fn kbd(mut self, kbd: &'a str) -> Self {
        self.kbd = Some(kbd);
        self
    }

    /// Attaches a tooltip (`.data-tip` style).
    pub fn tooltip(mut self, tooltip: &'a str) -> Self {
        self.tooltip = Some(tooltip);
        self
    }

    /// Overrides the minimum button size.
    pub fn min_size(mut self, min_size: Vec2) -> Self {
        self.min_size = min_size;
        self
    }

    /// Uses the monospace font for the button label.
    pub fn monospace(mut self, monospace: bool) -> Self {
        self.monospace = monospace;
        self
    }

    /// Draws a subtle `#243035` border around an `.ib` button.
    pub fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }

    /// Shows the button and returns its [`Response`].
    pub fn show(self, ui: &mut Ui) -> Response {
        let id = ui.make_persistent_id(self.id_salt);
        let font = if self.monospace {
            font_mono(11.0)
        } else {
            match self.variant {
                ButtonVariant::Menu => font_sans(12.0),
                _ => font_sans(11.5),
            }
        };

        let text_galley =
            ui.painter()
                .layout_no_wrap(self.label.to_owned(), font.clone(), TEXT_PRIMARY);
        let kbd_width = self
            .kbd
            .map_or(0.0, |k| (k.chars().count() as f32 * 6.5) + 14.0);
        let pad_x = match self.variant {
            ButtonVariant::Default | ButtonVariant::Primary | ButtonVariant::Danger => 10.0,
            ButtonVariant::Menu => 8.0,
            ButtonVariant::Icon { .. } => 7.0,
            ButtonVariant::Play { .. } => 6.0,
        };

        let desired_width = (text_galley.size().x + pad_x * 2.0 + kbd_width).max(self.min_size.x);
        let desired_size = Vec2::new(desired_width, self.min_size.y);
        let (rect, response) = ui.allocate_exact_size(desired_size, Sense::click());

        let ctx = ui.ctx();
        let hover_t =
            ctx.animate_bool_with_time(id.with("hover"), response.hovered(), HOVER_ANIM_SECS);
        let press_t = ctx.animate_bool_with_time(
            id.with("press"),
            response.is_pointer_button_down_on(),
            PRESS_ANIM_SECS,
        );

        if response.hovered() {
            ctx.set_cursor_icon(CursorIcon::PointingHand);
        }

        let y_offset = match self.variant {
            ButtonVariant::Default | ButtonVariant::Primary | ButtonVariant::Danger => {
                press_t * 1.0
            }
            _ => 0.0,
        };
        let draw_rect = rect.translate(Vec2::new(0.0, y_offset));

        let (bg_color, border_stroke, text_color) = match self.variant {
            ButtonVariant::Default => {
                let bg = lerp_color(
                    lerp_color(BG_BUTTON, BG_BUTTON_HOVER, hover_t),
                    BG_BUTTON_ACTIVE,
                    press_t,
                );
                let border = lerp_color(BORDER_CONTROL, BORDER_HOVER, hover_t);
                (bg, Stroke::new(1.0, border), TEXT_PRIMARY)
            }
            ButtonVariant::Primary => {
                let bg = lerp_color(ACCENT_YELLOW, ACCENT_YELLOW_HOVER, hover_t);
                (bg, Stroke::new(1.0, bg), TEXT_ON_YELLOW)
            }
            ButtonVariant::Danger => {
                let bg = lerp_color(BG_BUTTON, BG_DANGER_HOVER, hover_t);
                let border = lerp_color(BORDER_CONTROL, ACCENT_RED_FILL, hover_t);
                let fg = lerp_color(TEXT_DANGER, TEXT_DANGER_HOVER, hover_t);
                (bg, Stroke::new(1.0, border), fg)
            }
            ButtonVariant::Icon { active } => {
                if active {
                    (ACCENT_BLUE, Stroke::NONE, TEXT_ON_ACCENT)
                } else {
                    let bg = lerp_color(
                        lerp_color(Color32::TRANSPARENT, BG_BUTTON_HOVER, hover_t),
                        BG_BUTTON_ACTIVE,
                        press_t,
                    );
                    let fg = lerp_color(TEXT_CONTROL, Color32::WHITE, hover_t);
                    let stroke = if self.bordered {
                        Stroke::new(1.0, BORDER_PANEL)
                    } else {
                        Stroke::NONE
                    };
                    (bg, stroke, fg)
                }
            }
            ButtonVariant::Menu => {
                let bg = lerp_color(Color32::TRANSPARENT, BG_MENU_HOVER, hover_t);
                let fg = lerp_color(TEXT_CONTROL, Color32::WHITE, hover_t);
                (bg, Stroke::NONE, fg)
            }
            ButtonVariant::Play { active } => {
                if active {
                    ui.painter().rect_filled(
                        draw_rect.expand(3.0),
                        CornerRadius::same(RADIUS_WIDGET + 2),
                        Color32::from_rgba_unmultiplied(0x3F, 0xBF, 0x9F, 56),
                    );
                    (ACCENT_GREEN_FILL, Stroke::NONE, Color32::WHITE)
                } else {
                    let bg = lerp_color(
                        lerp_color(Color32::TRANSPARENT, BG_BUTTON_HOVER, hover_t),
                        BG_BUTTON_ACTIVE,
                        press_t,
                    );
                    let fg = lerp_color(TEXT_CONTROL, Color32::WHITE, hover_t);
                    (bg, Stroke::NONE, fg)
                }
            }
        };

        let painter = ui.painter();
        painter.rect(
            draw_rect,
            CornerRadius::same(RADIUS_WIDGET),
            bg_color,
            border_stroke,
            StrokeKind::Inside,
        );

        if response.has_focus() {
            painter.rect_stroke(
                draw_rect.expand(1.5),
                CornerRadius::same(RADIUS_WIDGET + 1),
                Stroke::new(2.0, ACCENT_BLUE),
                StrokeKind::Outside,
            );
        }

        if let Some(kbd_text) = self.kbd {
            let text_pos = Pos2::new(
                draw_rect.left() + pad_x,
                draw_rect.center().y - text_galley.size().y * 0.5,
            );
            painter.text(text_pos, Align2::LEFT_TOP, self.label, font, text_color);
            let kbd_rect = Rect::from_min_max(
                Pos2::new(
                    draw_rect.right() - kbd_width - 4.0,
                    draw_rect.center().y - 8.0,
                ),
                Pos2::new(draw_rect.right() - 6.0, draw_rect.center().y + 8.0),
            );
            let on_yellow = matches!(self.variant, ButtonVariant::Primary);
            paint_kbd_in_rect(painter, kbd_rect, kbd_text, on_yellow);
        } else {
            painter.text(
                draw_rect.center(),
                Align2::CENTER_CENTER,
                self.label,
                font,
                text_color,
            );
        }

        if let Some(tip) = self.tooltip {
            response.clone().on_hover_ui(|ui| {
                ui.label(
                    egui::RichText::new(tip)
                        .font(font_sans(11.0))
                        .color(TEXT_PRIMARY),
                );
            });
        }

        response
    }
}

/// Convenience helper for a standard animated button.
pub fn animated_button(
    ui: &mut Ui,
    id_salt: &str,
    label: &str,
    variant: ButtonVariant,
) -> Response {
    CruftButton::new(id_salt, label).variant(variant).show(ui)
}

/// Paints a `<kbd>` badge inside the given rect.
pub fn paint_kbd_in_rect(painter: &egui::Painter, rect: Rect, text: &str, on_yellow: bool) {
    let (bg, border, fg) = if on_yellow {
        (
            Color32::TRANSPARENT,
            Color32::from_rgba_unmultiplied(30, 22, 0, 90),
            TEXT_ON_YELLOW,
        )
    } else {
        (BG_FIELD, BORDER_KBD, TEXT_SECONDARY)
    };
    painter.rect(
        rect,
        CornerRadius::same(4),
        bg,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        font_mono(9.5),
        fg,
    );
}

/// Standalone `<kbd>` widget for inline shortcuts.
pub fn kbd_badge(ui: &mut Ui, text: &str) -> Response {
    let width = (text.chars().count() as f32 * 6.5 + 10.0).max(18.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 16.0), Sense::hover());
    paint_kbd_in_rect(ui.painter(), rect, text, false);
    response
}

/// Animated pill toggle switch (`.sw` / `.sw.on` from the mockup).
pub fn toggle_switch(ui: &mut Ui, id_salt: &str, on: &mut bool) -> Response {
    let id = ui.make_persistent_id(id_salt);
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(26.0, 14.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }

    let t = ui.ctx().animate_bool_with_time(id, *on, 0.15);
    let bg = lerp_color(BORDER_KBD, ACCENT_BLUE, t);
    let knob_color = lerp_color(TEXT_PRIMARY, TEXT_ON_ACCENT, t);
    let knob_x = rect.left() + 2.0 + t * 12.0 + 5.0;

    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(7), bg);
    painter.circle_filled(Pos2::new(knob_x, rect.center().y), 5.0, knob_color);

    response
}

// ---------------------------------------------------------------------------
// 2. Hierarchy Row with Colored Badge, Rename, Collapse, and Drag & Drop
// ---------------------------------------------------------------------------

/// Visual icon type for an entity in the Outliner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityKind {
    Light,
    Camera,
    Mesh,
    Entity,
    Ghost,
}

/// Semantic color for the optional badge pill on a hierarchy row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadgeKind {
    /// Muted count or general tag (`#8E9CA3`).
    Neutral,
    /// Blue prefab tag (`#7FC4E0` on `#1E3A48`).
    Prefab,
    /// Yellow agent / proposed tag (`#FFD970` on `#221E12`).
    Agent,
    /// Green active tag (`#3FBF9F`).
    Success,
}

/// Input properties for [`hierarchy_row`].
pub struct HierarchyRowProps<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub kind: EntityKind,
    pub depth: usize,
    pub tag: &'a str,
    pub badge_kind: BadgeKind,
    pub selected: bool,
    pub hidden: bool,
    pub has_children: bool,
    pub collapsed: bool,
    pub is_drop_target: bool,
}

/// Interaction output of [`hierarchy_row`].
#[derive(Default)]
pub struct HierarchyRowOutput {
    pub clicked: bool,
    pub double_clicked: bool,
    pub toggled_visibility: bool,
    pub toggled_collapse: bool,
    pub drag_started: bool,
    pub pointer_hovered: bool,
}

/// Draws a single row in the Outliner hierarchy with animated hover, selection flash,
/// expand/collapse triangle, colored entity icon, colored badge, visibility eye toggle,
/// double-click rename trigger, and drag & drop reparenting support.
pub fn hierarchy_row(ui: &mut Ui, props: HierarchyRowProps<'_>) -> HierarchyRowOutput {
    let row_id = ui.make_persistent_id(props.id);
    let avail_width = ui.available_width();
    let row_height = 24.0;
    let (row_rect, _) = ui.allocate_exact_size(Vec2::new(avail_width, row_height), Sense::hover());

    let indent = 8.0 + (props.depth as f32) * 16.0;
    let fold_rect = Rect::from_min_size(
        Pos2::new(row_rect.left() + indent - 6.0, row_rect.center().y - 8.0),
        Vec2::new(14.0, 16.0),
    );
    let eye_rect = Rect::from_min_size(
        Pos2::new(row_rect.right() - 26.0, row_rect.center().y - 11.0),
        Vec2::new(22.0, 22.0),
    );
    let pick_left = if props.has_children {
        fold_rect.right()
    } else {
        row_rect.left()
    };
    let pick_rect = Rect::from_min_max(
        Pos2::new(pick_left, row_rect.top()),
        Pos2::new(eye_rect.left() - 2.0, row_rect.max.y),
    );

    let fold_resp = if props.has_children {
        Some(ui.interact(fold_rect, row_id.with("fold"), Sense::click()))
    } else {
        None
    };
    let pick_resp = ui.interact(pick_rect, row_id.with("pick"), Sense::click_and_drag());
    let eye_resp = ui.interact(eye_rect, row_id.with("eye"), Sense::click());

    let pointer_in_row = ui.rect_contains_pointer(row_rect);
    let row_hovered = pointer_in_row || pick_resp.hovered() || eye_resp.hovered();
    if row_hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }

    let hover_t =
        ui.ctx()
            .animate_bool_with_time(row_id.with("hover"), row_hovered, HOVER_ANIM_SECS);
    let sel_t = ui
        .ctx()
        .animate_bool_with_time(row_id.with("sel"), props.selected, 0.22);

    let painter = ui.painter();

    // Row background (selected flash -> selected, ghost stripes, or hover)
    if props.selected {
        let flash_factor = (1.0 - sel_t).clamp(0.0, 1.0);
        let bg = lerp_color(BG_SELECTION, BG_SELECTION_FLASH, flash_factor);
        painter.rect_filled(row_rect, CornerRadius::ZERO, bg);
    } else if props.kind == EntityKind::Ghost {
        let base_alpha = (18.0 + hover_t * 16.0) as u8;
        painter.rect_filled(
            row_rect,
            CornerRadius::ZERO,
            Color32::from_rgba_unmultiplied(0xFF, 0xC8, 0x32, base_alpha),
        );
        let step = 12.0;
        let mut x = row_rect.left();
        while x < row_rect.right() {
            let p1 = Pos2::new(x, row_rect.bottom());
            let p2 = Pos2::new((x + 8.0).min(row_rect.right()), row_rect.top());
            painter.line_segment(
                [p1, p2],
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(0xFF, 0xC8, 0x32, 18)),
            );
            x += step;
        }
    } else if hover_t > 0.001 {
        let bg = lerp_color(Color32::TRANSPARENT, BG_ROW_HOVER, hover_t);
        painter.rect_filled(row_rect, CornerRadius::ZERO, bg);
    }

    if props.is_drop_target {
        painter.rect_stroke(
            row_rect.shrink(1.0),
            CornerRadius::same(4),
            Stroke::new(1.5, ACCENT_BLUE),
            StrokeKind::Inside,
        );
    }

    // Expand / collapse triangle for parent entities
    if props.has_children {
        let c = fold_rect.center();
        let tri_pts = if props.collapsed {
            vec![
                Pos2::new(c.x - 2.0, c.y - 3.5),
                Pos2::new(c.x + 2.5, c.y),
                Pos2::new(c.x - 2.0, c.y + 3.5),
            ]
        } else {
            vec![
                Pos2::new(c.x - 3.5, c.y - 2.0),
                Pos2::new(c.x + 3.5, c.y - 2.0),
                Pos2::new(c.x, c.y + 2.5),
            ]
        };
        painter.add(egui::Shape::convex_polygon(
            tri_pts,
            TEXT_SECONDARY,
            Stroke::NONE,
        ));
    }

    let icon_offset = if props.has_children { 10.0 } else { 0.0 };
    let icon_center = Pos2::new(
        row_rect.left() + indent + icon_offset + 6.5,
        row_rect.center().y,
    );
    paint_entity_icon(painter, icon_center, props.kind);

    // Entity name
    let name_color = if props.hidden {
        TEXT_MUTED
    } else if props.selected {
        Color32::WHITE
    } else if props.kind == EntityKind::Ghost {
        TEXT_YELLOW
    } else {
        TEXT_BODY
    };

    let name_pos = Pos2::new(icon_center.x + 13.0, row_rect.center().y);
    let galley = painter.layout_no_wrap(props.name.to_owned(), font_sans(12.0), name_color);
    let text_top_left = Pos2::new(name_pos.x, name_pos.y - galley.size().y * 0.5);
    painter.galley(text_top_left, galley.clone(), name_color);

    if props.hidden {
        let y = row_rect.center().y;
        painter.line_segment(
            [
                Pos2::new(text_top_left.x, y),
                Pos2::new(text_top_left.x + galley.size().x, y),
            ],
            Stroke::new(1.0, TEXT_MUTED),
        );
    }

    // Colored badge on the right (before the eye icon)
    if !props.tag.is_empty() {
        let (badge_bg, badge_border, badge_fg) = match props.badge_kind {
            BadgeKind::Neutral => (Color32::TRANSPARENT, Color32::TRANSPARENT, TEXT_MUTED),
            BadgeKind::Prefab => (
                Color32::from_rgba_unmultiplied(0x1E, 0x3A, 0x48, 200),
                Color32::from_rgba_unmultiplied(0x42, 0x99, 0xBF, 110),
                TEXT_BLUE_CHIP,
            ),
            BadgeKind::Agent => (BG_AGENT_BADGE, BORDER_AGENT, TEXT_YELLOW),
            BadgeKind::Success => (
                Color32::from_rgba_unmultiplied(0x0B, 0x72, 0x61, 90),
                ACCENT_GREEN,
                ACCENT_GREEN,
            ),
        };

        let tag_galley = painter.layout_no_wrap(props.tag.to_owned(), font_mono(10.0), badge_fg);
        let pill_w = tag_galley.size().x + 10.0;
        let pill_rect = Rect::from_min_size(
            Pos2::new(eye_rect.left() - pill_w - 4.0, row_rect.center().y - 8.0),
            Vec2::new(pill_w, 16.0),
        );
        if badge_bg != Color32::TRANSPARENT {
            painter.rect(
                pill_rect,
                CornerRadius::same(4),
                badge_bg,
                Stroke::new(1.0, badge_border),
                StrokeKind::Inside,
            );
        }
        painter.galley(
            Pos2::new(
                pill_rect.center().x - tag_galley.size().x * 0.5,
                pill_rect.center().y - tag_galley.size().y * 0.5,
            ),
            tag_galley,
            badge_fg,
        );
    }

    // Visibility eye button (visible on row hover, when selected, or when hidden)
    let eye_visible = row_hovered || props.selected || props.hidden;
    let eye_alpha_t =
        ui.ctx()
            .animate_bool_with_time(row_id.with("eye_vis"), eye_visible, HOVER_ANIM_SECS);
    let eye_hover_t = ui.ctx().animate_bool_with_time(
        row_id.with("eye_hov"),
        eye_resp.hovered(),
        HOVER_ANIM_SECS,
    );

    if eye_alpha_t > 0.01 {
        if eye_hover_t > 0.01 {
            painter.rect_filled(
                eye_rect,
                CornerRadius::same(RADIUS_WIDGET),
                lerp_color(Color32::TRANSPARENT, BG_BUTTON_ACTIVE, eye_hover_t),
            );
        }
        let base_eye_col = if props.hidden {
            TEXT_MUTED
        } else {
            lerp_color(TEXT_SECONDARY, Color32::WHITE, eye_hover_t)
        };
        let eye_col = base_eye_col.gamma_multiply(eye_alpha_t);
        let c = eye_rect.center();
        painter.circle_stroke(c, 4.5, Stroke::new(1.3, eye_col));
        if props.hidden {
            painter.line_segment(
                [
                    Pos2::new(c.x - 5.0, c.y + 4.5),
                    Pos2::new(c.x + 5.0, c.y - 4.5),
                ],
                Stroke::new(1.4, eye_col),
            );
        } else {
            painter.circle_filled(c, 1.8, eye_col);
        }
    }

    HierarchyRowOutput {
        clicked: pick_resp.clicked(),
        double_clicked: pick_resp.double_clicked(),
        toggled_visibility: eye_resp.clicked(),
        toggled_collapse: fold_resp.is_some_and(|r| r.clicked()),
        drag_started: pick_resp.drag_started(),
        pointer_hovered: pointer_in_row,
    }
}

/// Renders an inline rename text field inside a hierarchy row.
/// Returns `Some(Some(new_name))` when committed (`Enter` or focus lost), `Some(None)` on `Escape`, or `None` while editing.
pub fn hierarchy_rename_row(
    ui: &mut Ui,
    depth: usize,
    kind: EntityKind,
    buffer: &mut String,
) -> Option<Option<String>> {
    let avail_width = ui.available_width();
    let row_height = 24.0;
    let (row_rect, _) = ui.allocate_exact_size(Vec2::new(avail_width, row_height), Sense::hover());
    ui.painter()
        .rect_filled(row_rect, CornerRadius::ZERO, BG_SELECTION);

    let indent = 8.0 + (depth as f32) * 16.0;
    let icon_center = Pos2::new(row_rect.left() + indent + 6.5, row_rect.center().y);
    paint_entity_icon(ui.painter(), icon_center, kind);

    let edit_rect = Rect::from_min_max(
        Pos2::new(icon_center.x + 12.0, row_rect.top() + 2.0),
        Pos2::new(row_rect.right() - 8.0, row_rect.bottom() - 2.0),
    );
    ui.painter().rect(
        edit_rect,
        CornerRadius::same(4),
        BG_FIELD,
        Stroke::new(1.5, ACCENT_BLUE),
        StrokeKind::Inside,
    );

    let mut result = None;
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(edit_rect.shrink2(Vec2::new(4.0, 1.0))),
        |ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(buffer)
                    .frame(Frame::NONE)
                    .font(font_sans(12.0))
                    .desired_width(edit_rect.width() - 8.0),
            );
            if !resp.has_focus() && !resp.lost_focus() {
                resp.request_focus();
            }
            let enter_pressed = ui.input(|i| i.key_pressed(Key::Enter));
            let esc_pressed = ui.input(|i| i.key_pressed(Key::Escape));
            if esc_pressed {
                result = Some(None);
            } else if enter_pressed || resp.lost_focus() {
                result = Some(Some(buffer.clone()));
            }
        },
    );

    result
}

fn paint_entity_icon(painter: &egui::Painter, center: Pos2, kind: EntityKind) {
    match kind {
        EntityKind::Light => {
            let stroke = Stroke::new(1.5, ACCENT_YELLOW);
            painter.circle_stroke(center, 3.0, stroke);
            for i in 0..8 {
                let angle = (i as f32) * std::f32::consts::TAU / 8.0;
                let dir = Vec2::new(angle.cos(), angle.sin());
                painter.line_segment([center + dir * 4.5, center + dir * 6.5], stroke);
            }
        }
        EntityKind::Camera => {
            let stroke = Stroke::new(1.5, ACCENT_BLUE_LIGHT);
            let body = Rect::from_center_size(center - Vec2::new(1.5, 0.0), Vec2::new(8.0, 6.0));
            painter.rect_stroke(body, CornerRadius::same(1), stroke, StrokeKind::Inside);
            painter.line_segment(
                [
                    Pos2::new(body.right(), center.y - 1.5),
                    Pos2::new(body.right() + 3.5, center.y - 3.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(body.right() + 3.5, center.y - 3.0),
                    Pos2::new(body.right() + 3.5, center.y + 3.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(body.right() + 3.5, center.y + 3.0),
                    Pos2::new(body.right(), center.y + 1.5),
                ],
                stroke,
            );
        }
        EntityKind::Mesh => {
            let stroke = Stroke::new(1.4, ACCENT_BLUE_LIGHT);
            let top = Pos2::new(center.x, center.y - 5.5);
            let right = Pos2::new(center.x + 5.0, center.y - 2.5);
            let bottom = Pos2::new(center.x, center.y + 5.5);
            let left = Pos2::new(center.x - 5.0, center.y - 2.5);
            let br = Pos2::new(center.x + 5.0, center.y + 2.5);
            let bl = Pos2::new(center.x - 5.0, center.y + 2.5);
            painter.line_segment([top, right], stroke);
            painter.line_segment([right, br], stroke);
            painter.line_segment([br, bottom], stroke);
            painter.line_segment([bottom, bl], stroke);
            painter.line_segment([bl, left], stroke);
            painter.line_segment([left, top], stroke);
            painter.line_segment([left, center], stroke);
            painter.line_segment([right, center], stroke);
            painter.line_segment([center, bottom], stroke);
        }
        EntityKind::Entity => {
            painter.circle_stroke(center, 5.0, Stroke::new(1.5, TEXT_CONTROL));
            painter.circle_filled(center, 1.5, TEXT_CONTROL);
        }
        EntityKind::Ghost => {
            let r = Rect::from_center_size(center, Vec2::splat(10.0));
            let stroke = Stroke::new(1.4, ACCENT_YELLOW);
            painter.line_segment([r.left_top(), Pos2::new(r.left() + 4.0, r.top())], stroke);
            painter.line_segment([Pos2::new(r.right() - 4.0, r.top()), r.right_top()], stroke);
            painter.line_segment([r.right_top(), Pos2::new(r.right(), r.top() + 4.0)], stroke);
            painter.line_segment(
                [Pos2::new(r.right(), r.bottom() - 4.0), r.right_bottom()],
                stroke,
            );
            painter.line_segment(
                [r.right_bottom(), Pos2::new(r.right() - 4.0, r.bottom())],
                stroke,
            );
            painter.line_segment(
                [Pos2::new(r.left() + 4.0, r.bottom()), r.left_bottom()],
                stroke,
            );
            painter.line_segment(
                [r.left_bottom(), Pos2::new(r.left(), r.bottom() - 4.0)],
                stroke,
            );
            painter.line_segment([Pos2::new(r.left(), r.top() + 4.0), r.left_top()], stroke);
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Vec3 Field & Draggable Numeric Scalar Field
// ---------------------------------------------------------------------------

/// Detailed interaction output for [`vec3_field_detailed`].
#[derive(Default, Clone, Copy, Debug)]
pub struct Vec3FieldOutput {
    /// `(axis_index, new_value)` if an axis was dragged this frame.
    pub changed_axis: Option<(usize, f32)>,
    /// `true` if a drag gesture on any axis ended this frame.
    pub drag_stopped: bool,
}

/// Renders a 3-component vector row (`label` + draggable `X`, `Y`, `Z` fields with colored bars and labels)
/// and reports which specific axis was modified and when the drag gesture stops.
pub fn vec3_field_detailed(
    ui: &mut Ui,
    id_salt: &str,
    label: &str,
    values: &mut [f32; 3],
    speed: f32,
    suffix: &str,
) -> Vec3FieldOutput {
    let mut output = Vec3FieldOutput::default();

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (label_rect, _) = ui.allocate_exact_size(Vec2::new(72.0, 22.0), Sense::hover());
        ui.painter().text(
            Pos2::new(label_rect.left(), label_rect.center().y),
            Align2::LEFT_CENTER,
            label,
            font_sans(11.5),
            TEXT_CONTROL,
        );

        let avail = ui.available_width();
        let gap = 3.0;
        let cell_w = ((avail - gap * 2.0) / 3.0).max(42.0);
        let axes = [("X", AXIS_X), ("Y", AXIS_Y), ("Z", AXIS_Z)];

        ui.spacing_mut().item_spacing.x = gap;
        for (i, (axis_name, axis_color)) in axes.into_iter().enumerate() {
            let cell_id = ui.make_persistent_id((id_salt, axis_name));
            let (rect, resp) =
                ui.allocate_exact_size(Vec2::new(cell_w, 22.0), Sense::click_and_drag());

            if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            if resp.dragged() {
                let delta = resp.drag_delta().x * speed;
                if delta.abs() > f32::EPSILON {
                    values[i] += delta;
                    output.changed_axis = Some((i, values[i]));
                }
            }
            if resp.drag_stopped() {
                output.drag_stopped = true;
            }

            let hover_t = ui.ctx().animate_bool_with_time(
                cell_id.with("hover"),
                resp.hovered(),
                HOVER_ANIM_SECS,
            );
            let active_t = ui.ctx().animate_bool_with_time(
                cell_id.with("active"),
                resp.dragged() || resp.is_pointer_button_down_on(),
                PRESS_ANIM_SECS,
            );

            let bg = lerp_color(
                lerp_color(BG_FIELD, BG_FIELD_HOVER, hover_t),
                BG_FIELD_ACTIVE,
                active_t,
            );
            let border = lerp_color(
                lerp_color(BORDER_PANEL, BORDER_HOVER, hover_t),
                ACCENT_BLUE,
                active_t,
            );

            let painter = ui.painter();
            painter.rect(
                rect,
                CornerRadius::same(RADIUS_WIDGET),
                bg,
                Stroke::new(1.0, border),
                StrokeKind::Inside,
            );

            let tick_rect = Rect::from_min_size(
                Pos2::new(rect.left() + 6.0, rect.center().y - 6.0),
                Vec2::new(2.0, 12.0),
            );
            painter.rect_filled(tick_rect, CornerRadius::same(1), axis_color);

            painter.text(
                Pos2::new(tick_rect.right() + 4.0, rect.center().y),
                Align2::LEFT_CENTER,
                axis_name,
                font_mono(9.5),
                axis_color,
            );

            let formatted = if suffix.is_empty() {
                format!("{:.2}", values[i])
            } else {
                format!("{:.1}{}", values[i], suffix)
            };
            painter.text(
                Pos2::new(rect.right() - 6.0, rect.center().y),
                Align2::RIGHT_CENTER,
                formatted,
                font_mono(11.0),
                TEXT_PRIMARY,
            );
        }
    });

    output
}

/// Renders a 3-component vector row (`label` + draggable `X`, `Y`, `Z` fields with colored bars and labels).
/// Returns `true` if any component was modified by dragging.
pub fn vec3_field(
    ui: &mut Ui,
    id_salt: &str,
    label: &str,
    values: &mut [f32; 3],
    speed: f32,
    suffix: &str,
) -> bool {
    vec3_field_detailed(ui, id_salt, label, values, speed, suffix)
        .changed_axis
        .is_some()
}

/// Renders a single draggable numeric field row (`label` + `.num` box with horizontal drag cursor `↔`).
/// Returns `(Some(delta_x), drag_stopped)`.
pub fn draggable_scalar_row(
    ui: &mut Ui,
    id_salt: &str,
    label: &str,
    display_value: &str,
    editable: bool,
) -> (Option<f32>, bool) {
    let mut delta_x = None;
    let mut drag_stopped = false;

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

        let id = ui.make_persistent_id(id_salt);
        let sense = if editable {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        };
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), sense);

        if editable && (resp.hovered() || resp.dragged()) {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
        if editable && resp.dragged() {
            let dx = resp.drag_delta().x;
            if dx.abs() > f32::EPSILON {
                delta_x = Some(dx);
            }
        }
        if editable && resp.drag_stopped() {
            drag_stopped = true;
        }

        let hover_t = ui.ctx().animate_bool_with_time(
            id.with("hover"),
            editable && resp.hovered(),
            HOVER_ANIM_SECS,
        );
        let active_t = ui.ctx().animate_bool_with_time(
            id.with("active"),
            editable && (resp.dragged() || resp.is_pointer_button_down_on()),
            PRESS_ANIM_SECS,
        );

        let bg = lerp_color(
            lerp_color(BG_FIELD, BG_FIELD_HOVER, hover_t),
            BG_FIELD_ACTIVE,
            active_t,
        );
        let border = lerp_color(
            lerp_color(BORDER_PANEL, BORDER_HOVER, hover_t),
            ACCENT_BLUE,
            active_t,
        );

        let painter = ui.painter();
        painter.rect(
            rect,
            CornerRadius::same(RADIUS_WIDGET),
            bg,
            Stroke::new(1.0, border),
            StrokeKind::Inside,
        );
        painter.text(
            Pos2::new(rect.left() + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            display_value,
            font_mono(11.0),
            if editable { TEXT_PRIMARY } else { TEXT_MUTED },
        );
    });

    (delta_x, drag_stopped)
}

// ---------------------------------------------------------------------------
// 4. Animated Toast
// ---------------------------------------------------------------------------

/// Semantic kind for a viewport toast notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    /// Green checkmark (`#0B7261`).
    Ok,
    /// Green play triangle (`#0B7261`).
    Play,
    /// Yellow/amber agent or warning (`#B8860B`).
    Warn,
    /// Red error or rejection (`#A72145`).
    Error,
}

/// Active toast state displayed at the bottom of the viewport.
#[derive(Clone, Debug)]
pub struct ToastData {
    pub message: String,
    pub kind: ToastKind,
}

/// Renders an animated toast at the bottom-center of `viewport_rect` using `ctx.animate_bool`.
/// Returns `true` if the user clicked the `"Annuler  Ctrl Z"` button.
pub fn animated_toast(
    ui: &mut Ui,
    viewport_rect: Rect,
    toast: Option<&ToastData>,
    last_toast_cache: &mut Option<ToastData>,
) -> bool {
    if let Some(current) = toast {
        *last_toast_cache = Some(current.clone());
    }

    let anim_id = Id::new("cruft_viewport_toast_anim");
    let t = ui
        .ctx()
        .animate_bool_with_time(anim_id, toast.is_some(), 0.22);

    if t <= 0.01 {
        return false;
    }

    let Some(data) = toast.or(last_toast_cache.as_ref()) else {
        return false;
    };

    let painter = ui.painter();
    let msg_galley =
        painter.layout_no_wrap(data.message.clone(), font_sans_bold(12.0), TEXT_ON_ACCENT);
    let undo_width = 96.0;
    let toast_width = (34.0 + msg_galley.size().x + 14.0 + undo_width + 8.0).max(220.0);
    let toast_height = 32.0;

    let y_slide = (1.0 - t) * 8.0;
    let center = Pos2::new(
        viewport_rect.center().x,
        viewport_rect.bottom() - 16.0 - toast_height * 0.5 + y_slide,
    );
    let toast_rect = Rect::from_center_size(center, Vec2::new(toast_width, toast_height));

    let shadow = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha((125.0 * t) as u8),
    };
    painter.add(shadow.as_shape(toast_rect, CornerRadius::same(RADIUS_CARD)));

    let bg = TEXT_PRIMARY.gamma_multiply(t);
    let fg = TEXT_ON_ACCENT.gamma_multiply(t);
    painter.rect_filled(toast_rect, CornerRadius::same(RADIUS_CARD), bg);

    let icon_color = match data.kind {
        ToastKind::Ok | ToastKind::Play => ACCENT_GREEN_FILL,
        ToastKind::Warn => Color32::from_rgb(0xB8, 0x86, 0x0B),
        ToastKind::Error => ACCENT_RED_FILL,
    }
    .gamma_multiply(t);

    let icon_center = Pos2::new(toast_rect.left() + 18.0, toast_rect.center().y);
    let stroke = Stroke::new(2.2, icon_color);
    match data.kind {
        ToastKind::Ok => {
            painter.line_segment(
                [
                    Pos2::new(icon_center.x - 5.0, icon_center.y),
                    Pos2::new(icon_center.x - 1.5, icon_center.y + 3.5),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(icon_center.x - 1.5, icon_center.y + 3.5),
                    Pos2::new(icon_center.x + 5.5, icon_center.y - 4.0),
                ],
                stroke,
            );
        }
        ToastKind::Play => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(icon_center.x - 4.0, icon_center.y - 5.0),
                    Pos2::new(icon_center.x + 5.0, icon_center.y),
                    Pos2::new(icon_center.x - 4.0, icon_center.y + 5.0),
                ],
                icon_color,
                Stroke::NONE,
            ));
        }
        ToastKind::Warn => {
            painter.line_segment(
                [
                    Pos2::new(icon_center.x, icon_center.y - 5.0),
                    Pos2::new(icon_center.x, icon_center.y + 1.0),
                ],
                stroke,
            );
            painter.circle_filled(
                Pos2::new(icon_center.x, icon_center.y + 4.5),
                1.3,
                icon_color,
            );
        }
        ToastKind::Error => {
            painter.line_segment(
                [
                    Pos2::new(icon_center.x - 4.5, icon_center.y - 4.5),
                    Pos2::new(icon_center.x + 4.5, icon_center.y + 4.5),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(icon_center.x + 4.5, icon_center.y - 4.5),
                    Pos2::new(icon_center.x - 4.5, icon_center.y + 4.5),
                ],
                stroke,
            );
        }
    }

    painter.text(
        Pos2::new(toast_rect.left() + 32.0, toast_rect.center().y),
        Align2::LEFT_CENTER,
        &data.message,
        font_sans_bold(12.0),
        fg,
    );

    let btn_rect = Rect::from_min_max(
        Pos2::new(
            toast_rect.right() - undo_width - 6.0,
            toast_rect.center().y - 11.0,
        ),
        Pos2::new(toast_rect.right() - 6.0, toast_rect.center().y + 11.0),
    );
    let btn_resp = ui.interact(btn_rect, anim_id.with("undo_btn"), Sense::click());
    if btn_resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let btn_hover = ui.ctx().animate_bool_with_time(
        anim_id.with("undo_hov"),
        btn_resp.hovered(),
        HOVER_ANIM_SECS,
    );
    let btn_alpha = ((20.0 + btn_hover * 22.0) * t) as u8;
    painter.rect_filled(
        btn_rect,
        CornerRadius::same(RADIUS_WIDGET),
        Color32::from_black_alpha(btn_alpha),
    );
    painter.text(
        Pos2::new(btn_rect.left() + 7.0, btn_rect.center().y),
        Align2::LEFT_CENTER,
        "Annuler",
        font_sans(11.0),
        fg,
    );

    let kbd_rect = Rect::from_min_max(
        Pos2::new(btn_rect.right() - 42.0, btn_rect.center().y - 7.5),
        Pos2::new(btn_rect.right() - 5.0, btn_rect.center().y + 7.5),
    );
    painter.rect_stroke(
        kbd_rect,
        CornerRadius::same(4),
        Stroke::new(1.0, Color32::from_black_alpha((55.0 * t) as u8)),
        StrokeKind::Inside,
    );
    painter.text(
        kbd_rect.center(),
        Align2::CENTER_CENTER,
        "Ctrl Z",
        font_mono(9.5),
        BORDER_KBD.gamma_multiply(t),
    );

    btn_resp.clicked()
}
