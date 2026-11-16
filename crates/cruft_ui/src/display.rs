//! Display settings: window mode (windowed / borderless / exclusive fullscreen),
//! resolution presets up to 4K, UI scale and viewport render scale.
//! Persisted in the user config folder (`%APPDATA%\Cruft\display.ron`).
use bevy::{
    prelude::*,
    window::{
        Monitor, MonitorSelection, PresentMode, PrimaryMonitor, PrimaryWindow, VideoMode,
        VideoModeSelection, WindowMode, WindowResizeConstraints, WindowResolution,
    },
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayMode {
    #[default]
    Windowed,
    /// Borderless window covering the monitor (fast alt-tab, native resolution).
    Borderless,
    /// Exclusive fullscreen at the chosen resolution (if the monitor supports it).
    Exclusive,
}

impl DisplayMode {
    pub fn label(self) -> &'static str {
        match self {
            DisplayMode::Windowed => "Fenêtré",
            DisplayMode::Borderless => "Plein écran (sans bordure)",
            DisplayMode::Exclusive => "Plein écran exclusif",
        }
    }
}

/// Resolution presets (physical pixels).
pub const RESOLUTIONS: [(u32, u32, &str); 7] = [
    (1280, 720, "HD 720p"),
    (1600, 900, "HD+ 900p"),
    (1920, 1080, "Full HD 1080p"),
    (2560, 1080, "UW-FHD 2560×1080"),
    (2560, 1440, "QHD 1440p"),
    (3440, 1440, "UW-QHD 3440×1440"),
    (3840, 2160, "4K UHD 2160p"),
];

/// The editor layout needs this many logical pixels of width; the UI scale is
/// capped so small windows never overflow (e.g. 150 % in a 1280×720 window).
pub const MIN_LOGICAL_WIDTH: f32 = 1200.0;
pub const MIN_LOGICAL_HEIGHT: f32 = 640.0;

pub const UI_SCALES: [f32; 7] = [0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5];
pub const RENDER_SCALES: [f32; 4] = [0.5, 0.67, 0.75, 1.0];

#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaySettings {
    pub mode: DisplayMode,
    /// Window size in windowed mode, video mode in exclusive fullscreen.
    pub resolution: (u32, u32),
    /// Extra UI zoom on top of the OS scale; `None` = automatic.
    pub ui_scale: Option<f32>,
    /// Viewport texture size relative to its panel (lower = faster at 4K).
    pub viewport_render_scale: f32,
    pub vsync: bool,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            mode: DisplayMode::Windowed,
            resolution: (1600, 900),
            ui_scale: None,
            viewport_render_scale: 1.0,
            vsync: true,
        }
    }
}

impl DisplaySettings {
    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("Cruft").join("display.ron"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| ron::from_str::<Self>(&text).ok())
            .map(Self::sanitized)
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Self::path() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }

    fn sanitized(mut self) -> Self {
        self.resolution = (
            self.resolution.0.clamp(640, 7680),
            self.resolution.1.clamp(360, 4320),
        );
        self.ui_scale = self.ui_scale.map(|s| s.clamp(0.5, 3.0));
        self.viewport_render_scale = self.viewport_render_scale.clamp(0.25, 1.0);
        self
    }

    /// Initial primary window, so the app opens directly in the saved mode.
    pub fn initial_window(&self, title: &str) -> Window {
        let mut window = Window {
            title: title.into(),
            present_mode: present_mode(self.vsync),
            mode: window_mode(self.mode, self.resolution, None),
            ..default()
        };
        window.resolution = WindowResolution::new(self.resolution.0, self.resolution.1);
        window.resize_constraints = WindowResizeConstraints {
            min_width: 1024.0,
            min_height: 600.0,
            ..default()
        };
        window
    }

    /// Requested UI zoom (manual, or automatic for this screen).
    pub fn effective_ui_scale(&self, os_scale: f32, physical_height: u32) -> f32 {
        self.ui_scale
            .unwrap_or_else(|| auto_ui_scale(os_scale, physical_height))
    }

    /// Zoom actually applied: the requested one, capped so the window keeps at
    /// least `MIN_LOGICAL_WIDTH` × `MIN_LOGICAL_HEIGHT` logical pixels.
    /// Returns (zoom, capped).
    pub fn applied_ui_scale(&self, os_scale: f32, physical: (u32, u32)) -> (f32, bool) {
        let wanted = self.effective_ui_scale(os_scale, physical.1);
        let os_scale = os_scale.max(0.1);
        let max = (physical.0 as f32 / os_scale / MIN_LOGICAL_WIDTH)
            .min(physical.1 as f32 / os_scale / MIN_LOGICAL_HEIGHT)
            .max(0.5);
        if wanted > max + 0.001 {
            (max, true)
        } else {
            (wanted, false)
        }
    }

    pub fn toggle_fullscreen(&mut self) {
        self.mode = match self.mode {
            DisplayMode::Windowed => DisplayMode::Borderless,
            DisplayMode::Borderless | DisplayMode::Exclusive => DisplayMode::Windowed,
        };
    }

    pub fn step_ui_scale(&mut self, current: f32, direction: i32) {
        let index = UI_SCALES
            .iter()
            .position(|s| (*s - current).abs() < 0.01)
            .unwrap_or_else(|| {
                UI_SCALES
                    .iter()
                    .position(|s| *s > current)
                    .unwrap_or(UI_SCALES.len() - 1)
            }) as i32;
        let next = (index + direction).clamp(0, UI_SCALES.len() as i32 - 1) as usize;
        self.ui_scale = Some(UI_SCALES[next]);
    }
}

/// Without OS scaling, large screens get a bigger UI (1440p: 125 %, 4K: 150 %).
/// When Windows already scales (125 %, 150 %…), egui follows it: no extra zoom.
pub fn auto_ui_scale(os_scale: f32, physical_height: u32) -> f32 {
    if os_scale > 1.01 {
        return 1.0;
    }
    match physical_height {
        h if h >= 2000 => 1.5,
        h if h >= 1400 => 1.25,
        _ => 1.0,
    }
}

fn present_mode(vsync: bool) -> PresentMode {
    if vsync {
        PresentMode::AutoVsync
    } else {
        PresentMode::AutoNoVsync
    }
}

/// Best video mode of `monitor` with this size (highest refresh, then depth).
pub fn best_video_mode(monitor: &Monitor, size: (u32, u32)) -> Option<VideoMode> {
    monitor
        .video_modes
        .iter()
        .filter(|m| m.physical_size == UVec2::new(size.0, size.1))
        .max_by_key(|m| (m.refresh_rate_millihertz, m.bit_depth))
        .cloned()
}

fn window_mode(mode: DisplayMode, size: (u32, u32), monitor: Option<&Monitor>) -> WindowMode {
    match mode {
        DisplayMode::Windowed => WindowMode::Windowed,
        DisplayMode::Borderless => WindowMode::BorderlessFullscreen(MonitorSelection::Current),
        DisplayMode::Exclusive => WindowMode::Fullscreen(
            MonitorSelection::Current,
            monitor
                .and_then(|m| best_video_mode(m, size))
                .map_or(VideoModeSelection::Current, VideoModeSelection::Specific),
        ),
    }
}

/// Size of the primary monitor, for clamping windowed sizes and the menu.
pub fn primary_monitor_size(world: &mut World) -> Option<(u32, u32)> {
    let mut query = world.query_filtered::<&Monitor, With<PrimaryMonitor>>();
    query
        .iter(world)
        .next()
        .map(|m| (m.physical_width, m.physical_height))
}

/// Present when settings must be written to disk (the editor, not tests).
#[derive(Resource)]
pub struct PersistDisplaySettings;

/// Applies `DisplaySettings` to the primary window whenever they change, and saves them.
pub fn apply_display_settings(
    settings: Res<DisplaySettings>,
    persist: Option<Res<PersistDisplaySettings>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    monitors: Query<&Monitor, With<PrimaryMonitor>>,
    mut first_run: Local<bool>,
) {
    if !settings.is_changed() {
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    let monitor = monitors.iter().next();
    let mode = window_mode(settings.mode, settings.resolution, monitor);
    if window.mode != mode {
        window.mode = mode;
    }
    if settings.mode != DisplayMode::Borderless {
        let (mut w, mut h) = settings.resolution;
        if settings.mode == DisplayMode::Windowed
            && let Some(monitor) = monitor
        {
            // A window larger than the screen would be cut off by the OS.
            w = w.min(monitor.physical_width);
            h = h.min(monitor.physical_height);
        }
        if (window.physical_width(), window.physical_height()) != (w, h) {
            window.resolution.set_physical_resolution(w, h);
        }
        if settings.mode == DisplayMode::Windowed && *first_run {
            window.position = WindowPosition::Centered(MonitorSelection::Current);
        }
    }
    let present = present_mode(settings.vsync);
    if window.present_mode != present {
        window.present_mode = present;
    }
    // Do not rewrite the file for the initial (loaded) value.
    if *first_run
        && persist.is_some()
        && let Err(error) = settings.save()
    {
        warn!("Réglages d'affichage non enregistrés : {error}");
    }
    *first_run = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_scale_respects_os_scaling_and_grows_on_large_screens() {
        assert_eq!(auto_ui_scale(1.0, 1080), 1.0);
        assert_eq!(auto_ui_scale(1.0, 1440), 1.25);
        assert_eq!(auto_ui_scale(1.0, 2160), 1.5);
        assert_eq!(auto_ui_scale(1.5, 2160), 1.0, "Windows already scales");
        let manual = DisplaySettings {
            ui_scale: Some(2.0),
            ..default()
        };
        assert_eq!(manual.effective_ui_scale(1.0, 1080), 2.0);
        // 150 % in a 1280×720 window would leave 853 logical px: capped.
        let big = DisplaySettings {
            ui_scale: Some(1.5),
            ..default()
        };
        let (zoom, capped) = big.applied_ui_scale(1.0, (1280, 720));
        assert!(capped && zoom < 1.1 && 1280.0 / zoom >= MIN_LOGICAL_WIDTH - 0.5);
        assert_eq!(big.applied_ui_scale(1.0, (3840, 2160)), (1.5, false));
    }

    #[test]
    fn settings_roundtrip_and_sanitize() {
        let settings = DisplaySettings {
            mode: DisplayMode::Exclusive,
            resolution: (3840, 2160),
            ui_scale: Some(1.5),
            viewport_render_scale: 0.75,
            vsync: false,
        };
        let text = ron::to_string(&settings).unwrap();
        assert_eq!(ron::from_str::<DisplaySettings>(&text).unwrap(), settings);
        let partial: DisplaySettings = ron::from_str("(mode: Borderless)").unwrap();
        assert_eq!(partial.resolution, (1600, 900));
        let wild = DisplaySettings {
            resolution: (99999, 1),
            viewport_render_scale: 9.0,
            ..default()
        }
        .sanitized();
        assert_eq!(wild.resolution, (7680, 360));
        assert_eq!(wild.viewport_render_scale, 1.0);
    }

    #[test]
    fn fullscreen_toggle_and_ui_scale_steps() {
        let mut settings = DisplaySettings::default();
        settings.toggle_fullscreen();
        assert_eq!(settings.mode, DisplayMode::Borderless);
        settings.toggle_fullscreen();
        assert_eq!(settings.mode, DisplayMode::Windowed);
        settings.step_ui_scale(1.0, 1);
        assert_eq!(settings.ui_scale, Some(1.25));
        settings.step_ui_scale(2.5, 1);
        assert_eq!(settings.ui_scale, Some(2.5));
        settings.step_ui_scale(1.1, -1);
        assert_eq!(settings.ui_scale, Some(1.0));
    }

    #[test]
    fn apply_sets_mode_resolution_and_picks_exclusive_video_mode() {
        let mut app = App::new();
        app.insert_resource(DisplaySettings::default())
            .add_systems(Update, apply_display_settings);
        let mode_4k = VideoMode {
            physical_size: UVec2::new(3840, 2160),
            bit_depth: 32,
            refresh_rate_millihertz: 144_000,
        };
        app.world_mut().spawn((
            Monitor {
                name: None,
                physical_width: 3840,
                physical_height: 2160,
                physical_position: IVec2::ZERO,
                refresh_rate_millihertz: Some(144_000),
                scale_factor: 1.0,
                video_modes: vec![
                    VideoMode {
                        refresh_rate_millihertz: 60_000,
                        ..mode_4k
                    },
                    mode_4k,
                ],
            },
            PrimaryMonitor,
        ));
        let window = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Window>(window).unwrap().physical_width(),
            1600
        );

        app.world_mut().resource_mut::<DisplaySettings>().resolution = (3840, 2160);
        app.world_mut().resource_mut::<DisplaySettings>().mode = DisplayMode::Exclusive;
        app.update();
        let w = app.world().get::<Window>(window).unwrap();
        assert_eq!(
            w.mode,
            WindowMode::Fullscreen(
                MonitorSelection::Current,
                VideoModeSelection::Specific(mode_4k)
            )
        );
        assert_eq!(w.physical_width(), 3840);

        app.world_mut().resource_mut::<DisplaySettings>().mode = DisplayMode::Borderless;
        app.update();
        assert_eq!(
            app.world().get::<Window>(window).unwrap().mode,
            WindowMode::BorderlessFullscreen(MonitorSelection::Current)
        );
    }
}
