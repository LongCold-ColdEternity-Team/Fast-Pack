use crate::motion::Tween;
#[path = "builtin_overlay_controls.rs"]
mod builtin_overlay_controls;
#[path = "color_picker.rs"]
mod color_picker;
#[path = "item_editor.rs"]
mod item_editor;
#[path = "overlay_editor.rs"]
mod overlay_editor;
#[path = "smooth_scroll.rs"]
mod smooth_scroll;
use anyhow::{Context, Result, ensure};
use color_picker::ColorTarget;
use fast_pack::converter::load_extra_renames;
use fast_pack::{
    ConversionOptions, ConversionReport, PackInfo, pack_format_of, probe_pack, suggest_output,
    tables,
};
use iced::widget::{
    self, button, checkbox, column, container, mouse_area, pick_list, progress_bar, row,
    scrollable, space, text, text_input, tooltip,
};
use iced::{
    Alignment, Background, Border, Color, Element, Font, Length, Padding, Size, Subscription, Task,
    Theme, Vector, window,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
enum Message {
    BuiltinOverlay(fast_pack::builtin_overlays::Feature, bool),
    Mode(bool),
    Overlay(overlay_editor::Event),
    PickColor(ColorTarget),
    RainbowColor(bool),
    FishingLine(bool),
    BlockOutline(bool),
    FishingWidthEnabled(bool),
    OutlineWidthEnabled(bool),
    FishingWidth(f32),
    OutlineWidth(f32),
    ResetCrosshairColor,
    Items(item_editor::Event),
    Tick(Instant),
    Frame(Instant),
    WindowEvent(window::Id, window::Event),
    OpenFile,
    OpenFolder,
    InputPicked(Option<PathBuf>),
    InputsPicked(Vec<PathBuf>),
    QueueSelect(u64),
    QueueClear,
    QueueRemoveSelected,
    Target(String),
    AutoSave(bool),
    AutoSavePath(bool),
    HeyPixel(bool),
    AutoNetherite(bool),
    SourceVersion(String),
    Swap,
    Output(String),
    CommitExportDirectory,
    BrowseOutput,
    OutputPicked(Option<PathBuf>),
    Zip(bool),
    Description(String),
    PreserveLang(bool),
    PreserveCase(bool),
    CustomGlint(bool),
    CustomCrosshair(bool),
    ShowAttackIndicator(bool),
    CrosshairImport,
    CrosshairFolder,
    CrosshairFiles(Vec<PathBuf>),
    CrosshairSelected(String),
    CrosshairDrawing(bool),
    CrosshairStroke((u8, u8), (u8, u8), bool, bool),
    CrosshairStrokeEnd,
    CrosshairBrush(bool),
    CrosshairSize(u8),
    CrosshairClear,
    CrosshairUndo,
    CrosshairName(String),
    CrosshairEdit,
    CrosshairRestoreBuiltins,
    CrosshairColor(String),
    CrosshairSave,
    CrosshairDelete,
    CloseGlintPicker,
    GlintPreset([u8; 3]),
    GlintSaturationValue(f32, f32),
    GlintHue(f32),
    GlintHex(String),
    Advanced,
    Details,
    LearnMore,
    CloseLearnMore,
    OpenSource,
    Mapping,
    MappingPicked(Option<PathBuf>),
    Start,
    ConfirmOverwrite,
    CancelOverwrite,
    NewPack,
    SaveReport,
    ReportPicked(Option<PathBuf>),
    OpenOutput,
    ClearLogs,
    Theme,
    ReducedMotion(bool),
    Hover(u8, bool),
    Captured(window::Screenshot),
}

enum WorkEvent {
    OverlayIcon(u64, std::result::Result<Vec<u8>, String>),
    OverlayProgress(String, u8),
    OverlayDone(std::result::Result<fast_pack::overlay::Report, String>),
    Probed(u64, PackInfo),
    Preview(u64, std::result::Result<Option<LoadedPreview>, String>),
    Batch(fast_pack::batch::Event),
    Mapping(std::result::Result<Vec<(String, String)>, String>),
    Preferences(std::result::Result<(), String>),
    Crosshairs(
        std::result::Result<(Vec<fast_pack::crosshair::Preset>, Option<String>, String), String>,
    ),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Empty,
    Identifying,
    Ready,
    Running,
    Done,
    Failed,
}

#[derive(Clone, Copy)]
struct Colors {
    background: Color,
    panel: Color,
    field: Color,
    foreground: Color,
    muted: Color,
    line: Color,
    accent: Color,
    on_accent: Color,
    success: Color,
    danger: Color,
}

fn rgb(value: u32) -> Color {
    Color::from_rgb8((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

fn mix(a: Color, b: Color, fraction: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * fraction,
        g: a.g + (b.g - a.g) * fraction,
        b: a.b + (b.b - a.b) * fraction,
        a: a.a + (b.a - a.a) * fraction,
    }
}

fn rgb_hsv(color: [u8; 3], previous_hue: f32) -> [f32; 3] {
    let [red, green, blue] = color.map(|channel| channel as f32 / 255.0);
    let maximum = red.max(green).max(blue);
    let minimum = red.min(green).min(blue);
    let difference = maximum - minimum;
    let hue = if difference < f32::EPSILON {
        previous_hue
    } else if maximum == red {
        ((green - blue) / difference / 6.0).rem_euclid(1.0)
    } else if maximum == green {
        ((blue - red) / difference + 2.0) / 6.0
    } else {
        ((red - green) / difference + 4.0) / 6.0
    };
    [
        hue,
        if maximum == 0.0 {
            0.0
        } else {
            difference / maximum
        },
        maximum,
    ]
}

fn hsv_rgb([hue, saturation, value]: [f32; 3]) -> [u8; 3] {
    let hue = hue.rem_euclid(1.0) * 6.0;
    let fraction = hue.fract();
    let low = value * (1.0 - saturation);
    let falling = value * (1.0 - fraction * saturation);
    let rising = value * (1.0 - (1.0 - fraction) * saturation);
    let channels = match hue as u32 {
        0 => [value, rising, low],
        1 => [falling, value, low],
        2 => [low, value, rising],
        3 => [low, falling, value],
        4 => [rising, low, value],
        _ => [value, low, falling],
    };
    channels.map(|channel| (channel * 255.0).round().clamp(0.0, 255.0) as u8)
}

impl Colors {
    fn at(dark: f32) -> Self {
        let blend = |light, deep| mix(rgb(light), rgb(deep), dark);
        Self {
            background: blend(0xf3f6fa, 0x10151b),
            panel: blend(0xffffff, 0x18212b),
            field: blend(0xf7f9fc, 0x101820),
            foreground: blend(0x152030, 0xe7eef6),
            muted: blend(0x526477, 0xa0b0c2),
            line: blend(0xdce5ee, 0x2b3949),
            accent: blend(0x086e85, 0x70d9ee),
            on_accent: blend(0xffffff, 0x102027),
            success: blend(0x137652, 0x83dcb4),
            danger: blend(0xbb3544, 0xff94a5),
        }
    }
}

fn hint_style(colors: Colors) -> container::Style {
    container::Style {
        background: Some(colors.panel.into()),
        text_color: Some(colors.foreground),
        border: Border {
            radius: 9.0.into(),
            width: 1.0,
            color: colors.line,
        },
        ..Default::default()
    }
}

struct App {
    overlay_mode: bool,
    overlay: overlay_editor::Composer,
    color_target: ColorTarget,
    fishing_line: Option<[u8; 3]>,
    fishing_line_color: [u8; 3],
    block_outline: Option<[u8; 3]>,
    block_outline_color: [u8; 3],
    line_widths: fast_pack::compat::line_colors::Widths,
    fishing_line_width: f32,
    block_outline_width: f32,
    crosshair_tint: Option<[u8; 3]>,
    rainbow: fast_pack::compat::rainbow::Settings,
    builtin_overlays: fast_pack::builtin_overlays::Settings,
    item_editor: item_editor::Editor,
    profile: Option<crate::ui_profile::Profile>,
    preview_pending: Option<u64>,
    preview_request: Option<(u64, PathBuf)>,
    preview_cache: Vec<(u64, std::result::Result<Option<LoadedPreview>, String>)>,
    queue: Vec<QueueEntry>,
    queue_mode: bool,
    probing: HashSet<u64>,
    next_id: u64,
    batch_pool: Option<fast_pack::batch::Pool>,
    batch_options: Option<ConversionOptions>,
    startup_frame: bool,
    renderer_ready_at: Option<Instant>,
    phase: Phase,
    source: Option<PathBuf>,
    info: Option<PackInfo>,
    sky_preview: Option<widget::image::Handle>,
    sky_day_missing: bool,
    sky_preview_daytime: bool,
    sky_layers: usize,
    source_version: String,
    target: String,
    target_selected: bool,
    auto_save: bool,
    saved_target: Option<String>,
    auto_save_path: bool,
    heypixel_compat: bool,
    auto_netherite: bool,
    saved_export_directory: Option<PathBuf>,
    export_directory_input: String,
    directory_edited: Option<Instant>,
    output: String,
    description: String,
    as_zip: bool,
    preserve_lang: bool,
    preserve_case: bool,
    custom_glint: bool,
    custom_crosshair: bool,
    show_attack_indicator: bool,
    crosshair_drawing: bool,
    crosshair_canvas: fast_pack::crosshair::Canvas,
    crosshair_pen_color: [u8; 3],
    crosshair_name: String,
    crosshair_eraser: bool,
    crosshair_undo: Vec<fast_pack::crosshair::Canvas>,
    crosshair_edited: Option<Instant>,
    crosshair_color: String,
    crosshair_selected: Option<String>,
    crosshair_presets: Vec<fast_pack::crosshair::Preset>,
    crosshair_previews: Vec<widget::image::Handle>,
    crosshair_preview: widget::image::Handle,
    crosshair_pending: bool,
    crosshair_note: String,
    glint_color: [u8; 3],
    glint_hsv: [f32; 3],
    glint_hex: String,
    glint_edited: Option<Instant>,
    glint_picker: bool,
    glint_picker_motion: Tween,
    glint_color_motion: [Tween; 3],
    glint_preview_started: Instant,
    rainbow_preview_started: Instant,
    extra: Vec<(String, String)>,
    report: Option<String>,
    result_note: Option<String>,
    logs: Vec<String>,
    error: Option<String>,
    step: String,
    actual_progress: u8,
    advanced: bool,
    details: bool,
    learn_more: bool,
    dark: bool,
    reduced: bool,
    now: Instant,
    progress: Tween,
    advanced_motion: Tween,
    result_motion: Tween,
    load_motion: Tween,
    theme_motion: Tween,
    auto_save_motion: Tween,
    auto_save_path_motion: Tween,
    heypixel_motion: Tween,
    hover: HashMap<u8, Tween>,
    width: f32,
    generation: u64,
    sender: Sender<WorkEvent>,
    receiver: Receiver<WorkEvent>,
    preferences_sender: Sender<crate::preferences::Preferences>,
    preferences_pending: usize,
    mapping_pending: bool,
    window: Option<window::Id>,
    closing: bool,
    confirm_overwrite: bool,
    smoke: bool,
    smoke_output: Option<PathBuf>,
    capture: Option<PathBuf>,
    capture_stage: String,
    capture_requested: bool,
    settled_at: Instant,
}

#[derive(Clone)]
struct LoadedPreview {
    handle: widget::image::Handle,
    daytime: bool,
    layers: usize,
}

impl From<fast_pack::compat::sky::Preview> for LoadedPreview {
    fn from(preview: fast_pack::compat::sky::Preview) -> Self {
        let image = preview.image;
        Self {
            handle: widget::image::Handle::from_rgba(
                image.width(),
                image.height(),
                image.into_raw(),
            ),
            daytime: preview.daytime,
            layers: preview.names.len(),
        }
    }
}

struct QueueEntry {
    id: u64,
    key: String,
    source: PathBuf,
    info: Option<PackInfo>,
    phase: Phase,
    source_version: String,
    output: Option<PathBuf>,
    progress: u8,
    step: String,
    submitted: bool,
    error: Option<String>,
    report: Option<ConversionReport>,
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let (sender, receiver) = mpsc::channel();
        let now = Instant::now();
        let smoke = std::env::args().any(|value| value == "--smoke-test");
        let capture = std::env::var_os("FAST_PACK_CAPTURE").map(PathBuf::from);
        let capture_stage =
            std::env::var("FAST_PACK_CAPTURE_STAGE").unwrap_or_else(|_| "empty".into());
        let preferences = if !cfg!(test) && !smoke && capture.is_none() {
            crate::preferences::load()
        } else {
            crate::preferences::Preferences::default()
        };
        let dark = match std::env::var("FAST_PACK_CAPTURE_THEME").as_deref() {
            Ok("light") => false,
            Ok("dark") => true,
            _ => preferences.dark,
        };
        let saved_target = preferences.target_version;
        let auto_save = preferences.auto_save;
        let auto_save_path = preferences.auto_save_path;
        let heypixel_compat = preferences.heypixel_compat && fast_pack::client_audio::ENABLED;
        let saved_export_directory = preferences.export_directory;
        let active_glint_color = fast_pack::compat::glint::VANILLA_COLOR;
        let export_directory_input = saved_export_directory
            .as_ref()
            .filter(|_| auto_save_path)
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let target_selected = auto_save && saved_target.is_some();
        let (preferences_sender, preferences_receiver) =
            mpsc::channel::<crate::preferences::Preferences>();
        if !cfg!(test) && !smoke && capture.is_none() {
            let sender = sender.clone();
            std::thread::spawn(move || {
                for preferences in preferences_receiver {
                    let result = crate::preferences::save(&preferences)
                        .map_err(|error| format!("{error:#}"));
                    if sender.send(WorkEvent::Preferences(result)).is_err() {
                        break;
                    }
                }
            });
        }
        let mut app = Self {
            overlay_mode: false,
            overlay: overlay_editor::Composer::from_draft(preferences.overlay),
            color_target: ColorTarget::Glint,
            fishing_line: preferences.fishing_line,
            fishing_line_color: preferences
                .fishing_line
                .unwrap_or(preferences.fishing_line_color),
            block_outline: preferences.block_outline,
            block_outline_color: preferences
                .block_outline
                .unwrap_or(preferences.block_outline_color),
            line_widths: preferences.line_widths,
            fishing_line_width: preferences
                .line_widths
                .fishing
                .unwrap_or(preferences.fishing_line_width),
            block_outline_width: preferences
                .line_widths
                .outline
                .unwrap_or(preferences.block_outline_width),
            crosshair_tint: preferences.crosshair_tint,
            rainbow: preferences.rainbow,
            builtin_overlays: preferences.builtin_overlays,
            profile: crate::ui_profile::Profile::load(),
            preview_pending: None,
            preview_request: None,
            preview_cache: Vec::new(),
            queue: Vec::new(),
            queue_mode: false,
            probing: HashSet::new(),
            next_id: 0,
            batch_pool: None,
            batch_options: None,
            startup_frame: true,
            renderer_ready_at: None,
            phase: Phase::Empty,
            source: None,
            info: None,
            sky_preview: None,
            sky_day_missing: false,
            sky_preview_daytime: true,
            sky_layers: 0,
            source_version: "自动检测".into(),
            target: saved_target
                .as_ref()
                .filter(|_| auto_save)
                .cloned()
                .unwrap_or_else(|| fast_pack::LATEST_VERSION.into()),
            target_selected,
            auto_save,
            saved_target,
            auto_save_path,
            heypixel_compat,
            auto_netherite: true,
            saved_export_directory,
            export_directory_input,
            directory_edited: None,
            output: String::new(),
            description: String::new(),
            as_zip: true,
            preserve_lang: false,
            preserve_case: false,
            custom_glint: false,
            item_editor: item_editor::Editor::new(preferences.custom_item_rules),
            custom_crosshair: false,
            show_attack_indicator: preferences.show_attack_indicator,
            crosshair_drawing: preferences.crosshair_drawing,
            crosshair_color: fast_pack::compat::glint::color_hex(preferences.crosshair_pen_color),
            crosshair_preview: Self::crosshair_handle(
                &preferences
                    .crosshair_canvas
                    .render(&preferences.crosshair_name)
                    .image,
            ),
            crosshair_canvas: preferences.crosshair_canvas,
            crosshair_pen_color: preferences.crosshair_pen_color,
            crosshair_name: preferences.crosshair_name,
            crosshair_eraser: false,
            crosshair_undo: Vec::new(),
            crosshair_edited: None,
            crosshair_selected: preferences.crosshair_selection,
            crosshair_presets: Vec::new(),
            crosshair_previews: Vec::new(),
            crosshair_pending: false,
            crosshair_note: String::new(),
            glint_color: preferences.glint_color,
            glint_hsv: rgb_hsv(active_glint_color, 0.0),
            glint_hex: fast_pack::compat::glint::color_hex(active_glint_color),
            glint_edited: None,
            glint_picker: false,
            glint_picker_motion: Tween::new(0.0, 220),
            glint_color_motion: active_glint_color
                .map(|channel| Tween::new(channel as f32 / 255.0, 180)),
            glint_preview_started: now,
            rainbow_preview_started: now,
            extra: Vec::new(),
            report: None,
            result_note: None,
            logs: Vec::new(),
            error: None,
            step: String::new(),
            actual_progress: 0,
            advanced: false,
            details: false,
            learn_more: false,
            dark,
            reduced: system_reduced_motion(),
            now,
            progress: Tween::new(0.0, 220),
            advanced_motion: Tween::new(0.0, 230),
            result_motion: Tween::new(0.0, 280),
            load_motion: Tween::new(0.0, 80),
            theme_motion: Tween::new(if dark { 1.0 } else { 0.0 }, 180),
            auto_save_motion: Tween::new(if auto_save { 1.0 } else { 0.0 }, 220),
            auto_save_path_motion: Tween::new(if auto_save_path { 1.0 } else { 0.0 }, 220),
            heypixel_motion: Tween::new(if heypixel_compat { 1.0 } else { 0.0 }, 220),
            hover: HashMap::new(),
            width: 1040.0,
            generation: 0,
            sender,
            receiver,
            preferences_sender,
            preferences_pending: 0,
            mapping_pending: false,
            window: None,
            closing: false,
            confirm_overwrite: false,
            smoke,
            smoke_output: std::env::var_os("FAST_PACK_SMOKE_OUTPUT").map(PathBuf::from),
            capture,
            capture_stage,
            capture_requested: false,
            settled_at: now,
        };
        if let Ok(target) = std::env::var("FAST_PACK_SMOKE_TARGET") {
            if tables()
                .pack_formats
                .iter()
                .any(|(version, _)| version == &target)
            {
                app.target = target;
                app.target_selected = true;
            }
        }
        if let Some(path) = std::env::var_os("FAST_PACK_SMOKE_INPUT") {
            app.load(PathBuf::from(path));
        }
        if let Ok(paths) = std::env::var("FAST_PACK_SMOKE_INPUTS") {
            if let Ok(paths) = serde_json::from_str::<Vec<PathBuf>>(&paths) {
                for path in paths {
                    app.load(path);
                }
            }
        }
        if cfg!(test)
            || matches!(
                app.capture_stage.as_str(),
                "advanced" | "crosshair" | "canvas" | "overlay"
            )
        {
            app.set_crosshair_presets(
                fast_pack::crosshair::builtin_presets().unwrap_or_default(),
                None,
            );
        }
        if matches!(
            app.capture_stage.as_str(),
            "advanced" | "crosshair" | "canvas" | "items"
        ) {
            app.advanced = true;
            app.advanced_motion.set(1.0, now, true);
        }
        if app.profile.is_some() {
            app.advanced = true;
            app.advanced_motion.set(1.0, now, true);
            app.custom_crosshair = true;
        }
        if matches!(app.capture_stage.as_str(), "crosshair" | "canvas") {
            app.custom_crosshair = true;
        }
        if app.capture_stage == "overlay" {
            app.overlay_mode = true;
            if let Some(path) = std::env::var_os("FAST_PACK_CAPTURE_OVERLAY_DRAFT")
                && let Ok(bytes) = std::fs::read(path)
                && let Ok(draft) = serde_json::from_slice(&bytes)
            {
                app.overlay = overlay_editor::Composer::from_draft(draft);
            }
        }
        if app.capture_stage == "canvas" {
            app.crosshair_drawing = true;
            if let Some(path) = std::env::var_os("FAST_PACK_CROSSHAIR_CANVAS") {
                if let Ok(canvas) = std::fs::read(path).and_then(|bytes| {
                    serde_json::from_slice::<fast_pack::crosshair::Canvas>(&bytes)
                        .map_err(std::io::Error::other)
                }) {
                    app.crosshair_canvas = fast_pack::crosshair::Canvas::from_image(
                        &canvas.render("自绘准星").image,
                        canvas.size,
                    );
                }
            }
            app.sync_crosshair_preview();
        }
        if !smoke && app.capture.is_none() && !cfg!(test) {
            app.crosshair_pending = true;
            let sender = app.sender.clone();
            std::thread::spawn(move || {
                let result = crate::preferences::crosshair_store()
                    .and_then(|store| store.load())
                    .map(|presets| (presets, None, String::new()))
                    .map_err(|error| format!("{error:#}"));
                let _ = sender.send(WorkEvent::Crosshairs(result));
            });
        } else if let Some(directory) = std::env::var_os("FAST_PACK_CROSSHAIR_PRESETS") {
            if let Ok(presets) = fast_pack::crosshair::Store::new(PathBuf::from(directory)).load() {
                app.set_crosshair_presets(presets, None);
                if !app.crosshair_presets.is_empty() {
                    app.crosshair_drawing = false;
                    app.sync_crosshair_preview();
                }
            }
        }
        let mut task = if app.capture_stage == "items" {
            app.item_editor
                .update(item_editor::Event::Open, &app.target, app.source.clone())
        } else {
            Task::none()
        };
        if app.capture.is_some()
            && let Ok(name) = std::env::var("FAST_PACK_CAPTURE_COLOR")
        {
            let target = match name.as_str() {
                "glint" => Some(ColorTarget::Glint),
                "crosshair" => Some(ColorTarget::Crosshair),
                "item" => Some(ColorTarget::ItemOutline),
                "fishing" => Some(ColorTarget::FishingLine),
                "outline" => Some(ColorTarget::BlockOutline),
                "name-background" => Some(ColorTarget::NameBackground),
                _ => None,
            };
            if let Some(target) = target {
                if target == ColorTarget::ItemOutline {
                    let _ = app.item_editor.update(
                        item_editor::Event::Open,
                        &app.target,
                        app.source.clone(),
                    );
                    task = app.item_editor.update(
                        item_editor::Event::Catalog(
                            app.target.clone(),
                            fast_pack::compat::custom_items::items(&app.target),
                        ),
                        &app.target,
                        app.source.clone(),
                    );
                }
                app.open_color_picker(target);
                if std::env::var("FAST_PACK_CAPTURE_RAINBOW").as_deref() == Ok("1") {
                    app.set_rainbow(true);
                }
                if let Ok(value) = std::env::var("FAST_PACK_CAPTURE_COLOR_VALUE")
                    && let Ok(color) = fast_pack::compat::glint::parse_color(&value)
                {
                    app.set_picker_color(color);
                }
            }
        }
        if app.capture.is_some()
            && let Ok(widths) = std::env::var("FAST_PACK_CAPTURE_LINE_WIDTHS")
            && let Some((fishing, outline)) = widths.split_once(',')
            && let (Ok(fishing), Ok(outline)) = (
                fast_pack::compat::line_colors::parse_width(fishing),
                fast_pack::compat::line_colors::parse_width(outline),
            )
        {
            app.advanced = true;
            app.advanced_motion.set(1.0, now, true);
            app.fishing_line_width = fishing;
            app.block_outline_width = outline;
            app.line_widths = fast_pack::compat::line_colors::Widths {
                fishing: Some(fishing),
                outline: Some(outline),
            };
        }
        if app.capture.is_some()
            && std::env::var("FAST_PACK_CAPTURE_BUILTINS").as_deref() == Ok("1")
        {
            app.advanced = true;
            app.advanced_motion.set(1.0, now, true);
            app.builtin_overlays.features = fast_pack::builtin_overlays::Feature::ALL.to_vec();
        }
        (app, task)
    }

    fn busy(&self) -> bool {
        self.overlay.running || self.batch_options.is_some() || self.phase == Phase::Running
    }
    fn colors(&self) -> Colors {
        Colors::at(self.theme_motion.value(self.now))
    }
    fn animation_active(&self) -> bool {
        (!self.reduced && self.glint_picker && self.rainbow_for(self.color_target))
            || [
                &self.progress,
                &self.advanced_motion,
                &self.result_motion,
                &self.load_motion,
                &self.theme_motion,
                &self.auto_save_motion,
                &self.auto_save_path_motion,
                &self.heypixel_motion,
                &self.glint_picker_motion,
                &self.overlay.progress,
            ]
            .into_iter()
            .any(|value| value.active(self.now))
            || self.hover.values().any(|value| value.active(self.now))
            || self
                .glint_color_motion
                .iter()
                .any(|value| value.active(self.now))
    }
    fn subscription(&self) -> Subscription<Message> {
        let events =
            iced::event::listen_with(|event, _, id| match event {
                iced::Event::Window(event) => Some(Message::WindowEvent(id, event)),
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::CancelOverwrite),
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key, modifiers, ..
                }) if modifiers.control() => match key.as_ref() {
                    iced::keyboard::Key::Character("o") => Some(if modifiers.shift() {
                        Message::OpenFolder
                    } else {
                        Message::OpenFile
                    }),
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter) => {
                        Some(Message::Start)
                    }
                    _ => None,
                },
                _ => None,
            });
        if self.startup_frame
            || self.profile.is_some()
            || self.preview_pending.is_some()
            || self.preview_request.is_some()
            || self
                .queue
                .iter()
                .any(|entry| entry.phase == Phase::Identifying)
            || self.animation_active()
            || self.rainbow_preview_active()
            || self.overlay_text_animation_active()
            || self.phase == Phase::Identifying
            || self.busy()
            || self.mapping_pending
            || self.preferences_pending > 0
            || self.crosshair_pending
            || self.crosshair_edited.is_some()
            || self.directory_edited.is_some()
            || self.glint_edited.is_some()
            || self.overlay.edited.is_some()
            || self.glint_picker && !self.reduced
            || self.capture.is_some()
            || self.smoke
            || self.renderer_ready_at.is_some()
        {
            let mut subscriptions = vec![
                events,
                iced::time::every(Duration::from_millis(40)).map(Message::Tick),
            ];
            if self.startup_frame
                || self.animation_active()
                || self.glint_picker && !self.reduced
                || self.profile.is_some()
            {
                subscriptions.push(window::frames().map(Message::Frame));
            }
            Subscription::batch(subscriptions)
        } else {
            events
        }
    }
    fn theme(&self) -> Theme {
        let c = self.colors();
        Theme::custom(
            "Fast Pack",
            iced::theme::Palette {
                background: c.background,
                text: c.foreground,
                primary: c.accent,
                success: c.success,
                warning: rgb(0xd6a859),
                danger: c.danger,
            },
        )
    }

    fn load(&mut self, path: PathBuf) {
        if self.overlay_mode {
            return;
        }
        let path = path.canonicalize().unwrap_or(path);
        let key = fast_pack::batch::path_key(&path);
        if self.queue.iter().any(|entry| {
            entry.key == key
                || entry
                    .output
                    .as_ref()
                    .is_some_and(|output| fast_pack::batch::path_key(output) == key)
        }) {
            return;
        }
        self.next_id += 1;
        let id = self.next_id;
        let select = self.queue.is_empty()
            || !self.busy()
                && self
                    .queue
                    .iter()
                    .all(|entry| matches!(entry.phase, Phase::Done | Phase::Failed));
        self.queue.push(QueueEntry {
            id,
            key,
            source: path,
            info: None,
            phase: Phase::Identifying,
            source_version: "自动检测".into(),
            output: None,
            progress: 0,
            step: "等待识别".into(),
            submitted: false,
            error: None,
            report: None,
        });
        self.queue_mode |= self.queue.len() > 1;
        if select {
            self.select_queue(id);
        }
        self.refresh_queue_phase();
        self.probe_queued();
    }
    fn probe_queued(&mut self) {
        while self.probing.len() < fast_pack::batch::parallelism() {
            let Some(entry) = self.queue.iter().find(|entry| {
                entry.phase == Phase::Identifying && !self.probing.contains(&entry.id)
            }) else {
                break;
            };
            let id = entry.id;
            let path = entry.source.clone();
            self.probing.insert(id);
            let sender = self.sender.clone();
            std::thread::spawn(move || {
                let info =
                    std::panic::catch_unwind(|| probe_pack(&path)).unwrap_or_else(|_| PackInfo {
                        ok: false,
                        path: path.display().to_string(),
                        name: path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        error: Some("材质包识别意外停止".into()),
                        src_format: None,
                        version_label: None,
                        file_count: 0,
                        texture_count: 0,
                        has_mcmeta: false,
                        has_assets: false,
                        description: String::new(),
                        layout: None,
                        suggested_target: fast_pack::LATEST_VERSION.into(),
                        features: Default::default(),
                    });
                let _ = sender.send(WorkEvent::Probed(id, info));
            });
        }
    }
    fn apply_preview(&mut self, result: std::result::Result<Option<LoadedPreview>, String>) {
        match result {
            Ok(Some(preview)) => {
                self.sky_day_missing = false;
                self.sky_preview_daytime = preview.daytime;
                self.sky_layers = preview.layers;
                self.sky_preview = Some(preview.handle);
                if self.capture.is_some() {
                    self.settled_at = self.now;
                }
            }
            Ok(None) => {
                self.sky_preview = None;
                self.sky_day_missing = self
                    .info
                    .as_ref()
                    .is_some_and(|info| info.features.sky_count() > 0);
            }
            Err(error) => self.log(format!("天空预览：{error}")),
        }
    }
    fn request_preview(&mut self, id: u64, path: PathBuf) {
        if let Some(index) = self.preview_cache.iter().position(|(key, _)| *key == id) {
            let cached = self.preview_cache.remove(index);
            self.apply_preview(cached.1.clone());
            self.preview_cache.push(cached);
            self.preview_request = None;
            return;
        }
        if self.preview_pending == Some(id) {
            self.preview_request = None;
            return;
        }
        self.preview_request = Some((id, path));
        self.dispatch_preview();
    }
    fn dispatch_preview(&mut self) {
        if self.preview_pending.is_some() {
            return;
        }
        let Some((id, path)) = self.preview_request.take() else {
            return;
        };
        self.preview_pending = Some(id);
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(|| fast_pack::compat::sky::preview(&path))
                .map_err(|_| "天空预览读取意外停止".to_string())
                .and_then(|result| {
                    result
                        .map(|preview| preview.map(LoadedPreview::from))
                        .map_err(|error| format!("{error:#}"))
                });
            let _ = sender.send(WorkEvent::Preview(id, result));
        });
    }
    fn select_queue(&mut self, id: u64) {
        let Some(entry) = self.queue.iter().find(|entry| entry.id == id) else {
            return;
        };
        self.generation = id;
        self.source = Some(entry.source.clone());
        self.info = entry.info.clone();
        self.source_version = entry.source_version.clone();
        self.output = entry
            .output
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        self.report = entry.report.as_ref().map(ConversionReport::render);
        self.sky_preview = None;
        self.sky_day_missing = false;
        self.sky_layers = 0;
        self.load_motion.set(
            if self.info.is_some() { 1.0 } else { 0.0 },
            self.now,
            self.reduced,
        );
        let preview = entry
            .info
            .as_ref()
            .filter(|info| info.ok && entry.phase != Phase::Done)
            .map(|_| entry.source.clone())
            .or_else(|| (entry.phase == Phase::Identifying).then(|| entry.source.clone()));
        let completed = entry.phase == Phase::Done;
        if !self.busy() {
            self.error = entry.error.clone();
            self.refresh_queue_phase();
            if !completed {
                self.update_output();
            }
        }
        if let Some(path) = preview {
            self.request_preview(id, path);
        }
    }
    fn can_remove_selected(&self) -> bool {
        self.queue
            .iter()
            .find(|entry| entry.id == self.generation)
            .is_some_and(|entry| {
                !matches!(entry.phase, Phase::Running | Phase::Done)
                    && (!entry.submitted
                        || self
                            .batch_pool
                            .as_ref()
                            .is_some_and(|pool| pool.is_pending(entry.id)))
            })
    }
    fn advance_queue_selection(&mut self) {
        if self
            .queue
            .iter()
            .any(|entry| entry.id == self.generation && entry.phase != Phase::Done)
        {
            return;
        }
        if let Some(entry) = self
            .queue
            .iter()
            .find(|entry| entry.phase != Phase::Done)
            .or_else(|| self.queue.last())
        {
            self.select_queue(entry.id);
        } else {
            self.queue_mode = false;
            self.source = None;
            self.info = None;
            self.sky_preview = None;
            self.sky_day_missing = false;
            self.sky_layers = 0;
            self.output.clear();
            self.report = None;
            self.result_note = None;
            self.error = None;
            self.actual_progress = 0;
            self.progress.set(0.0, self.now, true);
            self.result_motion.set(0.0, self.now, true);
        }
    }
    fn remove_selected(&mut self) {
        let Some(index) = self
            .queue
            .iter()
            .position(|entry| entry.id == self.generation)
        else {
            return;
        };
        let entry = &self.queue[index];
        if matches!(entry.phase, Phase::Running | Phase::Done) {
            return;
        }
        if entry.submitted
            && !self
                .batch_pool
                .as_ref()
                .is_some_and(|pool| pool.cancel_pending(entry.id))
        {
            return;
        }
        let entry = self.queue.remove(index);
        self.preview_cache.retain(|(id, _)| *id != entry.id);
        if self
            .preview_request
            .as_ref()
            .is_some_and(|(id, _)| *id == entry.id)
        {
            self.preview_request = None;
        }
        self.log(format!(
            "已从列表移除：{}",
            entry
                .source
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));
        self.advance_queue_selection();
        if self.queue.is_empty() {
            self.batch_pool.take();
            self.batch_options = None;
        }
        self.refresh_queue_phase();
        if self.batch_options.is_some() {
            self.update_queue_progress();
            self.finish_queue();
        }
    }
    fn refresh_queue_phase(&mut self) {
        self.phase = if self.batch_options.is_some() {
            Phase::Running
        } else if self.queue.is_empty() {
            Phase::Empty
        } else if self.queue.iter().any(|entry| entry.phase == Phase::Ready) {
            Phase::Ready
        } else if self
            .queue
            .iter()
            .any(|entry| entry.phase == Phase::Identifying)
        {
            Phase::Identifying
        } else if self.queue.iter().any(|entry| entry.phase == Phase::Done) {
            Phase::Done
        } else {
            Phase::Failed
        };
    }
    fn plan_queue_outputs(&mut self, avoid_existing: bool) -> Result<()> {
        let directory = self.parsed_export_directory()?;
        let target = self
            .batch_options
            .as_ref()
            .and_then(|options| options.target_version.as_deref())
            .unwrap_or(&self.target);
        let as_zip = self
            .batch_options
            .as_ref()
            .map_or(self.as_zip, |options| options.as_zip);
        let inputs: Vec<_> = self
            .queue
            .iter()
            .map(|entry| entry.source.clone())
            .collect();
        let mut reserved: Vec<_> = self
            .queue
            .iter()
            .filter(|entry| entry.submitted || entry.phase == Phase::Done)
            .filter_map(|entry| entry.output.clone())
            .collect();
        let only_one = self.queue.len() == 1 && !self.queue_mode;
        for entry in &mut self.queue {
            if entry.phase != Phase::Ready || entry.submitted {
                continue;
            }
            let output = if only_one && self.smoke_output.is_some() {
                self.smoke_output.clone().unwrap()
            } else {
                fast_pack::batch::output_path(
                    &entry.source,
                    target,
                    as_zip,
                    directory.as_deref(),
                    &reserved,
                    &inputs,
                    avoid_existing,
                )
            };
            reserved.push(output.clone());
            entry.output = Some(output);
        }
        if let Some(entry) = self.queue.iter().find(|entry| entry.id == self.generation)
            && let Some(output) = &entry.output
        {
            self.output = output.display().to_string();
        }
        Ok(())
    }
    fn dispatch_queue(&mut self, avoid_existing: bool) {
        if let Err(error) = self.plan_queue_outputs(avoid_existing) {
            self.error = Some(error.to_string());
            return;
        }
        let Some(pool) = &self.batch_pool else { return };
        let Some(options) = &self.batch_options else {
            return;
        };
        for entry in &mut self.queue {
            if entry.phase != Phase::Ready || entry.submitted {
                continue;
            }
            let mut options = options.clone();
            options.src_format = if entry.source_version == "自动检测" {
                None
            } else {
                pack_format_of(&entry.source_version).ok()
            };
            let job = fast_pack::batch::Job {
                id: entry.id,
                source: entry.source.clone(),
                output: entry.output.clone().unwrap(),
                options,
            };
            match pool.submit(job) {
                Ok(()) => {
                    entry.submitted = true;
                    entry.step = "等待并行槽位".into();
                }
                Err(error) => {
                    entry.phase = Phase::Failed;
                    entry.error = Some(error.to_string());
                }
            }
        }
        self.update_queue_progress();
    }
    fn update_queue_progress(&mut self) {
        let finished = self
            .queue
            .iter()
            .filter(|entry| matches!(entry.phase, Phase::Done | Phase::Failed))
            .count();
        let running = self
            .queue
            .iter()
            .filter(|entry| entry.phase == Phase::Running)
            .count();
        let total: usize = self
            .queue
            .iter()
            .map(|entry| {
                if matches!(entry.phase, Phase::Done | Phase::Failed) {
                    100
                } else {
                    usize::from(entry.progress)
                }
            })
            .sum();
        self.actual_progress = (total / self.queue.len().max(1)) as u8;
        self.progress
            .set(self.actual_progress as f32, self.now, self.reduced);
        self.step = if self.queue.len() > 1 {
            format!(
                "并行转换 {running} 个 · 已结束 {finished} / {}",
                self.queue.len()
            )
        } else {
            self.queue
                .first()
                .map(|entry| entry.step.clone())
                .unwrap_or_default()
        };
    }
    fn finish_queue(&mut self) -> bool {
        if self.batch_options.is_none()
            || self
                .queue
                .iter()
                .any(|entry| !matches!(entry.phase, Phase::Done | Phase::Failed))
        {
            return false;
        }
        self.batch_pool.take();
        self.batch_options = None;
        self.refresh_queue_phase();
        let failed = self
            .queue
            .iter()
            .filter(|entry| entry.phase == Phase::Failed)
            .count();
        let done = self.queue.len() - failed;
        self.error = if done == 0 {
            Some(format!("{failed} 个材质包未能转换，请查看列表中的原因。"))
        } else {
            None
        };
        self.result_note = (self.queue.len() > 1)
            .then(|| format!("已完成 {done} 个，失败 {failed} 个。原材质包均已保留。"));
        if let Some(entry) = self
            .queue
            .iter()
            .find(|entry| entry.id == self.generation && entry.phase == Phase::Done)
            .or_else(|| {
                self.queue
                    .iter()
                    .rev()
                    .find(|entry| entry.phase == Phase::Done)
            })
        {
            self.report = entry.report.as_ref().map(ConversionReport::render);
            if let Some(output) = &entry.output {
                self.output = output.display().to_string();
            }
        }
        self.actual_progress = 100;
        self.progress.set(100.0, self.now, self.reduced);
        self.result_motion.set(1.0, self.now, self.reduced);
        self.settled_at = self.now;
        self.auto_netherite = true;
        self.custom_crosshair = false;
        self.custom_glint = false;
        self.glint_edited = None;
        self.sync_glint_color();
        self.save_preferences();
        true
    }
    fn update_output(&mut self) {
        if let Some(source) = &self.source {
            let Ok(directory) = self.parsed_export_directory() else {
                self.output.clear();
                return;
            };
            let suggested = suggest_output(source, &self.target, self.as_zip);
            let output = if let Some(directory) = directory {
                let name = suggested.file_name().unwrap_or_default();
                let candidate = directory.join(name);
                if candidate
                    .to_string_lossy()
                    .replace('\\', "/")
                    .eq_ignore_ascii_case(&source.to_string_lossy().replace('\\', "/"))
                {
                    directory.join("Fast Pack").join(name)
                } else {
                    candidate
                }
            } else {
                suggested
            };
            self.output = output.display().to_string();
        }
    }
    fn parsed_export_directory(&self) -> Result<Option<PathBuf>> {
        let input = self.export_directory_input.trim().trim_matches('"');
        if input.is_empty() {
            return Ok(None);
        }
        let path = PathBuf::from(input);
        ensure!(path.is_absolute(), "请选择完整的导出目录");
        ensure!(!path.is_file(), "导出位置应是文件夹，不能是文件");
        Ok(Some(path))
    }
    fn remember_export_directory(&mut self) {
        if self.auto_save_path
            && let Ok(directory) = self.parsed_export_directory()
            && directory != self.saved_export_directory
        {
            self.saved_export_directory = directory;
            self.save_preferences();
        }
    }
    fn remember_target(&mut self) {
        self.target_selected = true;
        if self.auto_save {
            self.saved_target = Some(self.target.clone());
            self.save_preferences();
        }
    }
    fn save_preferences(&mut self) {
        if !cfg!(test) && !self.smoke && self.capture.is_none() {
            if self
                .preferences_sender
                .send(crate::preferences::Preferences {
                    overlay: self.overlay.draft(),
                    dark: self.dark,
                    fishing_line: self.fishing_line,
                    block_outline: self.block_outline,
                    crosshair_tint: self.crosshair_tint,
                    rainbow: self.rainbow,
                    builtin_overlays: self.builtin_overlays.clone(),
                    fishing_line_color: self.fishing_line_color,
                    block_outline_color: self.block_outline_color,
                    line_widths: self.line_widths,
                    fishing_line_width: self.fishing_line_width,
                    block_outline_width: self.block_outline_width,
                    auto_save: self.auto_save,
                    target_version: self.saved_target.clone(),
                    auto_save_path: self.auto_save_path,
                    export_directory: self.saved_export_directory.clone(),
                    custom_glint: self.custom_glint,
                    glint_color: self.glint_color,
                    heypixel_compat: self.heypixel_compat,
                    heypixel_settings_version: 1,
                    crosshair_selection: self.crosshair_selected.clone(),
                    crosshair_drawing: self.crosshair_drawing,
                    crosshair_canvas: self.crosshair_canvas.clone(),
                    crosshair_pen_color: self.crosshair_pen_color,
                    crosshair_name: self.crosshair_name.clone(),
                    show_attack_indicator: self.show_attack_indicator,
                    custom_item_rules: self.item_editor.rules.clone(),
                })
                .is_ok()
            {
                self.preferences_pending += 1;
            } else {
                self.log("偏好设置未能保存：保存任务不可用".into());
            }
        }
    }

    fn log(&mut self, value: String) {
        if self.logs.len() >= 500 {
            self.logs.drain(..100);
        }
        self.logs.push(value);
    }
    fn crosshair_handle(image: &image::RgbaImage) -> widget::image::Handle {
        let image = image.clone();
        let mut min = (image.width(), image.height());
        let mut max = (0, 0);
        let mut visible = false;
        for (x, y, pixel) in image.enumerate_pixels() {
            if pixel[3] > 0 {
                min = (min.0.min(x), min.1.min(y));
                max = (max.0.max(x), max.1.max(y));
                visible = true;
            }
        }
        let mut preview = image::RgbaImage::new(108, 108);
        if visible {
            let glyph = image::imageops::crop_imm(
                &image,
                min.0,
                min.1,
                max.0 - min.0 + 1,
                max.1 - min.1 + 1,
            )
            .to_image();
            let scale = (108.0 / image.width().max(image.height()) as f32)
                .min(100.0 / glyph.width().max(glyph.height()) as f32);
            let width = (glyph.width() as f32 * scale).round().clamp(1.0, 100.0) as u32;
            let height = (glyph.height() as f32 * scale).round().clamp(1.0, 100.0) as u32;
            let glyph = image::imageops::resize(
                &glyph,
                width,
                height,
                image::imageops::FilterType::Nearest,
            );
            image::imageops::replace(
                &mut preview,
                &glyph,
                i64::from((108 - width) / 2),
                i64::from((108 - height) / 2),
            );
        }
        widget::image::Handle::from_rgba(preview.width(), preview.height(), preview.into_raw())
    }
    fn selected_crosshair(&self) -> Option<fast_pack::crosshair::Crosshair> {
        if self.crosshair_drawing {
            (!self.crosshair_canvas.is_empty())
                .then(|| self.crosshair_canvas.render(&self.crosshair_name))
        } else {
            self.crosshair_presets
                .iter()
                .find(|preset| Some(&preset.id) == self.crosshair_selected.as_ref())
                .map(|preset| {
                    let mut crosshair = preset.crosshair.clone();
                    if let Some(color) = self.crosshair_tint {
                        for pixel in crosshair.image.pixels_mut().filter(|pixel| pixel[3] > 0) {
                            pixel.0[..3].copy_from_slice(&color);
                        }
                    }
                    crosshair
                })
        }
    }
    fn sync_crosshair_preview(&mut self) {
        let crosshair = self
            .selected_crosshair()
            .unwrap_or_else(|| self.crosshair_canvas.render(&self.crosshair_name));
        self.crosshair_preview = Self::crosshair_handle(&crosshair.image);
    }
    fn set_crosshair_presets(
        &mut self,
        presets: Vec<fast_pack::crosshair::Preset>,
        selected: Option<String>,
    ) {
        if let Some(selected) = selected {
            self.crosshair_selected = Some(selected);
            self.crosshair_drawing = false;
        }
        if !presets
            .iter()
            .any(|preset| Some(&preset.id) == self.crosshair_selected.as_ref())
        {
            self.crosshair_selected = presets.first().map(|preset| preset.id.clone());
        }
        self.crosshair_previews = presets
            .iter()
            .map(|preset| Self::crosshair_handle(&preset.crosshair.image))
            .collect();
        self.crosshair_presets = presets;
        self.sync_crosshair_preview();
    }
    fn edit_crosshair(&mut self) {
        self.sync_crosshair_preview();
        self.save_preferences();
    }
    fn remember_canvas(&mut self) {
        if self.crosshair_undo.len() >= 32 {
            self.crosshair_undo.remove(0);
        }
        self.crosshair_undo.push(self.crosshair_canvas.clone());
    }
    fn active_glint_color(&self) -> [u8; 3] {
        if self.custom_glint {
            self.glint_color
        } else {
            fast_pack::compat::glint::VANILLA_COLOR
        }
    }
    fn sync_glint_color(&mut self) {
        let color = self.active_glint_color();
        self.glint_hsv = rgb_hsv(color, self.glint_hsv[0]);
        self.glint_hex = fast_pack::compat::glint::color_hex(color);
        for (motion, channel) in self.glint_color_motion.iter_mut().zip(color) {
            motion.set(channel as f32 / 255.0, self.now, self.reduced);
        }
    }
    fn set_glint_color(&mut self, color: [u8; 3]) {
        self.custom_glint = true;
        self.glint_color = color;
        self.sync_glint_color();
        self.glint_edited = Some(self.now);
    }
    fn commit_glint_color(&mut self) {
        if self.glint_edited.take().is_some() {
            if self.color_target == ColorTarget::ItemOutline {
                let _ = self.item_editor.update(
                    item_editor::Event::Save,
                    &self.target,
                    self.customization_source(),
                );
            }
            self.save_preferences();
        }
    }
    fn close_glint_picker(&mut self) {
        self.glint_picker = false;
        self.glint_picker_motion.set(0.0, self.now, self.reduced);
        self.glint_hex = fast_pack::compat::glint::color_hex(self.picker_color());
        self.commit_glint_color();
    }
    fn fail(&mut self, value: String) {
        self.error = Some(value.clone());
        self.phase = Phase::Failed;
        self.log(format!("转换失败：{value}"));
        self.result_motion.set(1.0, self.now, self.reduced);
        self.settled_at = self.now;
    }
    fn start(&mut self) {
        if self.overlay_mode {
            self.start_overlay();
            return;
        }
        if self.busy() || self.phase == Phase::Identifying {
            return;
        }
        self.close_glint_picker();
        if let Err(error) = self.parsed_export_directory() {
            self.error = Some(error.to_string());
            return;
        }
        self.directory_edited = None;
        self.remember_export_directory();
        let Some(_source) = self.source.clone() else {
            self.error = Some("请先导入材质包".into());
            return;
        };
        if !self.queue.iter().any(|entry| entry.phase == Phase::Ready) {
            self.error = Some("请导入有效的材质包".into());
            return;
        }
        if self.output.trim().is_empty() {
            self.error = Some("请选择输出路径".into());
            return;
        }
        if self.as_zip && !self.output.to_lowercase().ends_with(".zip") {
            self.output.push_str(".zip");
        }

        let source_format = if self.source_version == "自动检测" {
            None
        } else {
            pack_format_of(&self.source_version).ok()
        };
        let Ok(target_format) = pack_format_of(&self.target) else {
            self.error = Some("目标版本无效".into());
            return;
        };
        let crosshair = if self.custom_crosshair {
            if self.crosshair_pending {
                self.error = Some("准星预设正在处理，请稍候再转换".into());
                return;
            }
            let Some(crosshair) = self.selected_crosshair() else {
                self.error = Some("请先导入并选择准星预设，或选择自动绘制".into());
                return;
            };
            Some(crosshair)
        } else {
            None
        };
        let options = ConversionOptions {
            src_format: source_format,
            tgt_format: target_format,
            target_version: Some(self.target.clone()),
            description: (!self.description.trim().is_empty())
                .then(|| self.description.trim().into()),
            as_zip: self.as_zip,
            lowercase: !self.preserve_case,
            convert_lang: !self.preserve_lang,
            extra_renames: self.extra.clone(),
            glint_color: self.custom_glint.then_some(self.glint_color),
            restore_vanilla_glint: !self.custom_glint,
            heypixel_compat: self.heypixel_compat,
            auto_client_setup: !self.smoke && self.capture.is_none(),
            auto_netherite: self.auto_netherite,
            auto_netherite_sword: true,
            crosshair,
            show_attack_indicator: self.show_attack_indicator,
            custom_items: self.item_editor.applied(&self.target),
            builtin_overlays: self.builtin_overlays.clone(),
            fishing_line_color: (target_format >= 7).then_some(self.fishing_line).flatten(),
            block_outline_color: (target_format >= 7).then_some(self.block_outline).flatten(),
            line_color_selection: (target_format >= 7).then_some(
                fast_pack::compat::line_colors::ColorSelection {
                    fishing: self.fishing_line.is_some(),
                    outline: self.block_outline.is_some(),
                },
            ),
            line_widths: if target_format >= 7 {
                self.line_widths
            } else {
                Default::default()
            },
            rainbow: fast_pack::compat::rainbow::Settings {
                glint: self.custom_glint && self.rainbow.glint,
                crosshair: self.custom_crosshair && self.rainbow.crosshair,
                fishing_line: target_format >= 7
                    && self.fishing_line.is_some()
                    && self.rainbow.fishing_line,
                block_outline: target_format >= 7
                    && self.block_outline.is_some()
                    && self.rainbow.block_outline,
            },
        };
        self.phase = Phase::Running;
        self.error = None;
        self.report = None;
        self.logs.clear();
        self.result_note = None;
        self.actual_progress = 0;
        self.progress.set(0.0, self.now, true);
        self.result_motion.set(0.0, self.now, true);
        self.step = "正在准备".into();
        if let Err(error) = self.plan_queue_outputs(false) {
            self.fail(error.to_string());
            return;
        }
        let sender = self.sender.clone();
        self.batch_pool = Some(fast_pack::batch::Pool::new(
            fast_pack::batch::parallelism(),
            move |event| {
                let _ = sender.send(WorkEvent::Batch(event));
            },
        ));
        self.batch_options = Some(options);
        self.dispatch_queue(false);
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        if let Message::Overlay(event) = message {
            return self.overlay_update(event);
        }
        if let Message::Mode(overlay) = message {
            if self.busy() {
                return Task::none();
            }
            self.close_glint_picker();
            self.overlay_mode = overlay;
            return Task::batch([
                self.item_editor
                    .sync(&self.target, self.customization_source()),
                widget::operation::snap_to(
                    "fast-pack-content",
                    widget::operation::RelativeOffset {
                        x: Some(0.0),
                        y: Some(0.0),
                    },
                ),
            ]);
        }
        if let Message::Items(event) = message {
            if self.busy()
                && !matches!(
                    event,
                    item_editor::Event::Catalog(..) | item_editor::Event::Preview(..)
                )
            {
                return Task::none();
            }
            let save = matches!(
                event,
                item_editor::Event::Save | item_editor::Event::Reset | item_editor::Event::Clear
            );
            let scroll =
                self.capture_stage == "items" && matches!(event, item_editor::Event::Catalog(..));
            let task = self
                .item_editor
                .update(event, &self.target, self.customization_source());
            if save {
                self.save_preferences();
            }
            return if scroll {
                Task::batch([
                    task,
                    widget::operation::snap_to(
                        "fast-pack-content",
                        widget::operation::RelativeOffset {
                            x: Some(0.0),
                            y: Some(1.0),
                        },
                    ),
                ])
            } else {
                task
            };
        }
        self.now = Instant::now();
        match message {
            Message::Frame(now) => {
                self.now = now;
                if self.startup_frame {
                    self.startup_frame = false;
                    crate::startup::record("first_frame");
                    if crate::renderer_guard::pending() {
                        self.renderer_ready_at = Some(now + Duration::from_millis(500));
                    }
                }
                if let Some(profile) = &mut self.profile {
                    let (delta, action, done) = profile.frame(now);
                    if done {
                        self.profile = None;
                        if let Some(window) = self.window {
                            self.capture_requested = true;
                            return window::screenshot(window).map(Message::Captured);
                        }
                        return iced::exit();
                    }
                    if let Some(action) = action {
                        if action == 0 {
                            self.dark = !self.dark;
                            self.theme_motion.set(
                                if self.dark { 1.0 } else { 0.0 },
                                now,
                                self.reduced,
                            );
                        } else {
                            self.custom_crosshair = !self.custom_crosshair;
                        }
                    }
                    return widget::operation::scroll_by(
                        "fast-pack-content",
                        widget::operation::AbsoluteOffset { x: 0.0, y: delta },
                    );
                }
            }
            Message::Tick(now) => {
                if self
                    .renderer_ready_at
                    .is_some_and(|deadline| now >= deadline)
                {
                    self.renderer_ready_at = None;
                    crate::renderer_guard::ready();
                }
                self.now = now;
                if self.overlay.edited.is_some_and(|edited| {
                    now.saturating_duration_since(edited) >= Duration::from_millis(350)
                }) {
                    self.overlay.edited = None;
                    self.save_preferences();
                }
                if self.crosshair_edited.is_some_and(|edited| {
                    now.saturating_duration_since(edited) >= Duration::from_millis(500)
                }) {
                    self.crosshair_edited = None;
                    self.save_preferences();
                }
                if self.glint_edited.is_some_and(|edited| {
                    now.saturating_duration_since(edited) >= Duration::from_millis(350)
                }) {
                    self.commit_glint_color();
                }
                if self.directory_edited.is_some_and(|edited| {
                    now.saturating_duration_since(edited) >= Duration::from_millis(350)
                }) {
                    self.directory_edited = None;
                    self.remember_export_directory();
                }
                let mut reset_scroll = false;
                let events: Vec<_> = self.receiver.try_iter().collect();
                for event in events {
                    match event {
                        WorkEvent::OverlayIcon(revision, result) => {
                            if revision == self.overlay.icon_revision {
                                self.overlay.icon_pending = false;
                                match result {
                                    Ok(bytes) => {
                                        self.overlay.icon_preview =
                                            widget::image::Handle::from_bytes(bytes.clone());
                                        self.overlay.icon = Some(bytes);
                                        self.overlay.error = None;
                                        self.overlay.edited = Some(now);
                                    }
                                    Err(error) => self.overlay.error = Some(error),
                                }
                            }
                        }
                        WorkEvent::OverlayProgress(step, percent) => {
                            self.overlay.step = step;
                            self.overlay.percent = percent;
                            self.overlay.progress.set(percent as f32, now, self.reduced);
                        }
                        WorkEvent::OverlayDone(result) => {
                            self.overlay.running = false;
                            match result {
                                Ok(report) => {
                                    self.overlay.percent = 100;
                                    self.overlay.result = Some(report);
                                }
                                Err(error) => self.overlay.error = Some(error),
                            }
                            if self.closing
                                && self.preferences_pending == 0
                                && !self.crosshair_pending
                            {
                                return iced::exit();
                            }
                        }
                        WorkEvent::Probed(id, info) => {
                            self.probing.remove(&id);
                            if let Some(entry) = self.queue.iter_mut().find(|entry| entry.id == id)
                            {
                                entry.phase = if info.ok { Phase::Ready } else { Phase::Failed };
                                entry.source_version = info
                                    .version_label
                                    .as_ref()
                                    .filter(|label| {
                                        tables()
                                            .pack_formats
                                            .iter()
                                            .any(|(version, _)| version == *label)
                                    })
                                    .cloned()
                                    .unwrap_or_else(|| "自动检测".into());
                                entry.error = info.error.clone();
                                entry.step = if info.ok {
                                    "等待转换"
                                } else {
                                    "识别失败"
                                }
                                .into();
                                entry.info = Some(info.clone());
                                if id == self.generation {
                                    if info.ok && !self.target_selected && !self.busy() {
                                        self.target = info.suggested_target.clone();
                                    }
                                    self.select_queue(id);
                                    self.settled_at = now;
                                }
                                self.log(format!(
                                    "{}：{}",
                                    info.name,
                                    if info.ok {
                                        "识别完成"
                                    } else {
                                        "无法识别"
                                    }
                                ));
                                self.refresh_queue_phase();
                                if self.batch_options.is_some() {
                                    self.dispatch_queue(true);
                                } else if info.ok && self.smoke_output.is_some() {
                                    self.start();
                                }
                                if self.finish_queue() {
                                    reset_scroll = true;
                                }
                            }
                            self.probe_queued();
                        }
                        WorkEvent::Preview(id, result) => {
                            if self.preview_pending == Some(id) {
                                self.preview_pending = None;
                            }
                            if self.queue.iter().any(|entry| entry.id == id) {
                                self.preview_cache.retain(|(key, _)| *key != id);
                                self.preview_cache.push((id, result.clone()));
                                if self.preview_cache.len() > 12 {
                                    drop(self.preview_cache.remove(0));
                                }
                                if id == self.generation {
                                    self.apply_preview(result);
                                }
                            }
                            self.dispatch_preview();
                        }
                        WorkEvent::Batch(event) => {
                            match event {
                                fast_pack::batch::Event::Started(id) => {
                                    if let Some(entry) =
                                        self.queue.iter_mut().find(|entry| entry.id == id)
                                    {
                                        entry.phase = Phase::Running;
                                        entry.step = "正在准备".into();
                                    }
                                }
                                fast_pack::batch::Event::Progress(id, step, percent) => {
                                    if let Some(entry) =
                                        self.queue.iter_mut().find(|entry| entry.id == id)
                                    {
                                        entry.progress = entry.progress.max(percent.min(100));
                                        entry.step = step.clone();
                                        let name = entry
                                            .source
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy();
                                        let log = format!("[{name}] {percent:3}% {step}");
                                        self.log(log);
                                    }
                                }
                                fast_pack::batch::Event::Finished(id, result) => {
                                    if let Some(entry) =
                                        self.queue.iter_mut().find(|entry| entry.id == id)
                                    {
                                        entry.progress = 100;
                                        match *result {
                                            Ok(report) => {
                                                if let Some(path) = &self.smoke_output {
                                                    let _ = std::fs::write(
                                                        path.with_extension("gui-report.json"),
                                                        serde_json::to_string_pretty(&report)
                                                            .unwrap_or_default(),
                                                    );
                                                }
                                                entry.phase = Phase::Done;
                                                entry.step = "转换完成".into();
                                                entry.report = Some(report);
                                            }
                                            Err(error) => {
                                                entry.phase = Phase::Failed;
                                                entry.step = "转换失败".into();
                                                entry.error = Some(error);
                                            }
                                        }
                                    }
                                }
                            }
                            self.advance_queue_selection();
                            self.update_queue_progress();
                            if self.finish_queue() {
                                reset_scroll = true;
                                if self.closing
                                    && self.preferences_pending == 0
                                    && !self.crosshair_pending
                                    || self.smoke && self.capture.is_none()
                                {
                                    return iced::exit();
                                }
                            }
                        }
                        WorkEvent::Mapping(result) => {
                            self.mapping_pending = false;
                            match result {
                                Ok(mapping) => self.extra = mapping,
                                Err(error) => self.error = Some(error),
                            }
                        }
                        WorkEvent::Preferences(result) => {
                            self.preferences_pending = self.preferences_pending.saturating_sub(1);
                            if let Err(error) = result {
                                self.log(format!("偏好设置未能保存：{error}"));
                            }
                            if self.closing
                                && !self.busy()
                                && self.preferences_pending == 0
                                && !self.crosshair_pending
                            {
                                return iced::exit();
                            }
                        }
                        WorkEvent::Crosshairs(result) => {
                            self.crosshair_pending = false;
                            match result {
                                Ok((presets, selected, note)) => {
                                    self.set_crosshair_presets(presets, selected);
                                    self.crosshair_note = note;
                                    self.save_preferences();
                                }
                                Err(error) => self.crosshair_note = error,
                            }
                            if self.closing && !self.busy() && self.preferences_pending == 0 {
                                return iced::exit();
                            }
                        }
                    }
                }
                if self.smoke
                    && self.smoke_output.is_none()
                    && !self.busy()
                    && self.capture.is_none()
                    && now.saturating_duration_since(self.settled_at) > Duration::from_millis(800)
                {
                    return iced::exit();
                }
                let can_capture = match self.capture_stage.as_str() {
                    "overlay" => self.overlay_mode,
                    "items" => self.item_editor.ready(&self.target),
                    "sky" => self.sky_preview.is_some(),
                    "running" => self.busy(),
                    "done" => self.phase == Phase::Done,
                    "failed" => self.phase == Phase::Failed,
                    "loaded" | "advanced" | "crosshair" | "canvas" => self.phase == Phase::Ready,
                    _ => self.phase == Phase::Empty,
                };
                if self.capture.is_some()
                    && self.profile.is_none()
                    && !self.capture_requested
                    && can_capture
                    && now.saturating_duration_since(self.settled_at) > Duration::from_millis(1000)
                    && let Some(window) = self.window
                {
                    self.capture_requested = true;
                    return window::screenshot(window).map(Message::Captured);
                }
                if reset_scroll && !self.overlay_mode {
                    return widget::operation::snap_to(
                        "fast-pack-content",
                        widget::operation::RelativeOffset {
                            x: Some(0.0),
                            y: Some(0.0),
                        },
                    );
                }
            }
            Message::WindowEvent(id, event) => match event {
                window::Event::Opened { size, .. } => {
                    crate::startup::record("window_opened");
                    self.window = Some(id);
                    self.width = size.width;
                }
                window::Event::Resized(size) => self.width = size.width,
                window::Event::FileDropped(path) => {
                    if self.overlay_mode {
                        if path.extension().is_some_and(|extension| {
                            ["png", "jpg", "jpeg"]
                                .iter()
                                .any(|expected| extension.eq_ignore_ascii_case(expected))
                        }) {
                            return self
                                .overlay_update(overlay_editor::Event::IconPicked(Some(path)));
                        }
                    } else {
                        self.load(path);
                    }
                }
                window::Event::FileHovered(_) => {
                    self.hover
                        .entry(1)
                        .or_insert_with(|| Tween::new(0.0, 150))
                        .set(1.0, self.now, self.reduced);
                }
                window::Event::FilesHoveredLeft => {
                    self.hover
                        .entry(1)
                        .or_insert_with(|| Tween::new(0.0, 150))
                        .set(0.0, self.now, self.reduced);
                }
                window::Event::CloseRequested => {
                    self.commit_glint_color();
                    if self.overlay.edited.take().is_some() {
                        self.save_preferences();
                    }
                    if self.crosshair_edited.take().is_some() {
                        self.save_preferences();
                    }
                    self.directory_edited = None;
                    self.remember_export_directory();
                    if self.busy() || self.preferences_pending > 0 || self.crosshair_pending {
                        self.closing = true;
                        self.log(if self.busy() {
                            "转换完成并清理临时文件后关闭窗口".into()
                        } else {
                            "设置与准星预设保存完成后关闭窗口".into()
                        });
                    } else {
                        return iced::exit();
                    }
                }
                _ => {}
            },
            Message::OpenFile if self.overlay_mode => {
                return self.overlay_update(overlay_editor::Event::ChooseIcon);
            }
            Message::OpenFile => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Fast Pack · 导入材质包")
                            .add_filter("材质包", &["zip", "rar"])
                            .pick_files()
                            .await
                            .unwrap_or_default()
                            .into_iter()
                            .map(|file| file.path().to_owned())
                            .collect()
                    },
                    Message::InputsPicked,
                );
            }
            Message::OpenFolder => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Fast Pack · 选择材质包文件夹")
                            .pick_folder()
                            .await
                            .map(|file| file.path().to_owned())
                    },
                    Message::InputPicked,
                );
            }
            Message::InputPicked(Some(path)) => self.load(path),
            Message::InputsPicked(paths) => {
                for path in paths {
                    self.load(path);
                }
            }
            Message::QueueSelect(id) => self.select_queue(id),
            Message::QueueClear if !self.busy() => return self.update(Message::NewPack),
            Message::QueueRemoveSelected => self.remove_selected(),
            Message::Target(target) if !self.busy() => {
                if !tables()
                    .pack_formats
                    .iter()
                    .any(|(label, _)| label == &target)
                {
                    return Task::none();
                }
                self.target = target;
                self.remember_target();
                self.update_output();
                self.report = None;
                self.error = None;
                if self.info.is_some() {
                    self.phase = Phase::Ready;
                    self.result_motion.set(0.0, self.now, self.reduced);
                }
            }
            Message::SourceVersion(version) if !self.busy() => {
                if let Some(entry) = self
                    .queue
                    .iter_mut()
                    .find(|entry| entry.id == self.generation)
                {
                    entry.source_version = version.clone();
                }
                self.source_version = version;
                self.report = None;
                self.error = None;
            }
            Message::AutoSave(value) if !self.busy() => {
                self.auto_save = value;
                self.auto_save_motion
                    .set(if value { 1.0 } else { 0.0 }, self.now, self.reduced);
                if value {
                    self.remember_target();
                } else {
                    self.save_preferences();
                }
            }
            Message::AutoSavePath(value) if !self.busy() => {
                self.auto_save_path = value;
                self.auto_save_path_motion.set(
                    if value { 1.0 } else { 0.0 },
                    self.now,
                    self.reduced,
                );
                self.directory_edited = None;
                if value {
                    if self.export_directory_input.trim().is_empty()
                        && let Some(directory) = &self.saved_export_directory
                    {
                        self.export_directory_input = directory.display().to_string();
                    }
                    if let Ok(directory) = self.parsed_export_directory() {
                        self.saved_export_directory = directory;
                    }
                    self.update_output();
                }
                self.save_preferences();
            }
            Message::HeyPixel(value) if !self.busy() && fast_pack::client_audio::ENABLED => {
                self.heypixel_compat = value;
                self.heypixel_motion
                    .set(if value { 1.0 } else { 0.0 }, self.now, self.reduced);
                self.save_preferences();
            }
            Message::AutoNetherite(value) if !self.busy() => self.auto_netherite = value,
            Message::CustomGlint(value) if !self.busy() => {
                self.custom_glint = value;
                self.sync_glint_color();
                self.glint_edited = None;
                self.save_preferences();
            }
            Message::CustomCrosshair(value) if !self.busy() => self.custom_crosshair = value,
            Message::ShowAttackIndicator(value) if !self.busy() => {
                self.show_attack_indicator = value;
                self.save_preferences();
            }
            Message::CrosshairImport if !self.busy() && !self.crosshair_pending => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Fast Pack · 导入准星预设")
                            .add_filter("准星包 / 图片", &["zip", "rar", "png"])
                            .pick_files()
                            .await
                            .map(|files| {
                                files
                                    .into_iter()
                                    .map(|file| file.path().to_owned())
                                    .collect()
                            })
                            .unwrap_or_default()
                    },
                    Message::CrosshairFiles,
                );
            }
            Message::CrosshairFolder if !self.busy() && !self.crosshair_pending => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Fast Pack · 选择准星包文件夹")
                            .pick_folder()
                            .await
                            .map(|file| vec![file.path().to_owned()])
                            .unwrap_or_default()
                    },
                    Message::CrosshairFiles,
                );
            }
            Message::CrosshairFiles(paths)
                if !paths.is_empty() && !self.busy() && !self.crosshair_pending =>
            {
                self.crosshair_pending = true;
                self.crosshair_note = "正在导入准星…".into();
                let sender = self.sender.clone();
                std::thread::spawn(move || {
                    let result = (|| -> Result<_> {
                        let store = crate::preferences::crosshair_store()?;
                        let mut selected = None;
                        let mut count = 0;
                        let mut errors = Vec::new();
                        for path in paths {
                            match fast_pack::crosshair::import(&path) {
                                Ok(crosshairs) => {
                                    for crosshair in crosshairs {
                                        match store.save(&crosshair) {
                                            Ok(id) => {
                                                selected = Some(id);
                                                count += 1;
                                            }
                                            Err(error) => errors
                                                .push(format!("{}：{error:#}", crosshair.name)),
                                        }
                                    }
                                }
                                Err(error) => errors.push(format!(
                                    "{}：{error:#}",
                                    path.file_name().unwrap_or_default().to_string_lossy()
                                )),
                            }
                        }
                        let mut note = format!("已导入 {count} 个准星，相同图案自动合并。");
                        if !errors.is_empty() {
                            note.push_str(&format!(" {}", errors.join("；")));
                        }
                        Ok((store.load()?, selected, note))
                    })()
                    .map_err(|error| format!("{error:#}"));
                    let _ = sender.send(WorkEvent::Crosshairs(result));
                });
            }
            Message::CrosshairSelected(id) if !self.busy() && !self.crosshair_pending => {
                self.crosshair_selected = Some(id);
                self.crosshair_drawing = false;
                self.edit_crosshair();
            }
            Message::CrosshairDrawing(value) if !self.busy() => {
                self.crosshair_drawing = value;
                self.edit_crosshair();
            }
            Message::CrosshairStroke(from, to, erase, begin) if !self.busy() => {
                if begin {
                    self.remember_canvas();
                }
                let color = if erase || self.crosshair_eraser {
                    [0; 4]
                } else {
                    [
                        self.crosshair_pen_color[0],
                        self.crosshair_pen_color[1],
                        self.crosshair_pen_color[2],
                        255,
                    ]
                };
                self.crosshair_canvas.line(from, to, color);
                self.crosshair_edited = Some(self.now);
                self.sync_crosshair_preview();
            }
            Message::CrosshairStrokeEnd if !self.busy() => {
                self.crosshair_edited = None;
                self.save_preferences();
            }
            Message::CrosshairBrush(erase) if !self.busy() => self.crosshair_eraser = erase,
            Message::CrosshairSize(size) if !self.busy() => {
                self.remember_canvas();
                self.crosshair_canvas = fast_pack::crosshair::Canvas::from_image(
                    &self.crosshair_canvas.render(&self.crosshair_name).image,
                    size,
                );
                self.edit_crosshair();
            }
            Message::CrosshairClear if !self.busy() => {
                self.remember_canvas();
                self.crosshair_canvas.pixels.fill([0; 4]);
                self.edit_crosshair();
            }
            Message::CrosshairUndo if !self.busy() => {
                if let Some(canvas) = self.crosshair_undo.pop() {
                    self.crosshair_canvas = canvas;
                    self.edit_crosshair();
                }
            }
            Message::CrosshairName(name) if !self.busy() => {
                self.crosshair_name = name;
                self.crosshair_edited = Some(self.now);
            }
            Message::CrosshairEdit if !self.busy() => {
                if let Some(preset) = self.selected_crosshair() {
                    self.remember_canvas();
                    self.crosshair_canvas = fast_pack::crosshair::Canvas::from_image(
                        &preset.image,
                        if preset.image.width() >= 30 { 30 } else { 15 },
                    );
                    self.crosshair_name = format!("{} · 自绘", preset.name);
                    self.crosshair_drawing = true;
                    self.edit_crosshair();
                }
            }
            Message::CrosshairRestoreBuiltins if !self.busy() && !self.crosshair_pending => {
                self.crosshair_pending = true;
                let sender = self.sender.clone();
                std::thread::spawn(move || {
                    let result = (|| -> Result<_> {
                        let store = crate::preferences::crosshair_store()?;
                        store.restore_builtins()?;
                        Ok((store.load()?, None, "已恢复全部内置准星预设。".into()))
                    })()
                    .map_err(|error| format!("{error:#}"));
                    let _ = sender.send(WorkEvent::Crosshairs(result));
                });
            }
            Message::CrosshairColor(value) if !self.busy() => {
                self.crosshair_color = value.clone();
                if let Ok(color) = fast_pack::compat::glint::parse_color(&value) {
                    self.crosshair_pen_color = color;
                    if !self.crosshair_drawing {
                        self.crosshair_tint = Some(color);
                    }
                    self.edit_crosshair();
                }
            }
            Message::CrosshairSave if !self.busy() && !self.crosshair_pending => {
                if self.crosshair_canvas.is_empty() {
                    self.crosshair_note = "画布还是空的，请先点涂一个准星。".into();
                    return Task::none();
                }
                self.crosshair_pending = true;
                let crosshair = self.crosshair_canvas.render(&self.crosshair_name);
                let sender = self.sender.clone();
                std::thread::spawn(move || {
                    let result = (|| -> Result<_> {
                        let store = crate::preferences::crosshair_store()?;
                        let id = store.save(&crosshair)?;
                        Ok((store.load()?, Some(id), "已保存为准星预设。".into()))
                    })()
                    .map_err(|error| format!("{error:#}"));
                    let _ = sender.send(WorkEvent::Crosshairs(result));
                });
            }
            Message::CrosshairDelete if !self.busy() && !self.crosshair_pending => {
                if let Some(id) = self.crosshair_selected.clone() {
                    self.crosshair_pending = true;
                    let sender = self.sender.clone();
                    std::thread::spawn(move || {
                        let result = (|| -> Result<_> {
                            let store = crate::preferences::crosshair_store()?;
                            store.delete(&id)?;
                            Ok((
                                store.load()?,
                                None,
                                "已删除准星预设，原始准星包保留。".into(),
                            ))
                        })()
                        .map_err(|error| format!("{error:#}"));
                        let _ = sender.send(WorkEvent::Crosshairs(result));
                    });
                }
            }
            Message::PickColor(target) if !self.busy() => self.open_color_picker(target),
            Message::BuiltinOverlay(feature, enabled) if !self.busy() => {
                self.builtin_overlays.set(feature, enabled);
                self.save_preferences();
            }
            Message::RainbowColor(enabled) if !self.busy() => self.set_rainbow(enabled),
            Message::FishingLine(enabled) if !self.busy() => {
                self.fishing_line = enabled.then_some(self.fishing_line_color);
                self.save_preferences();
            }
            Message::BlockOutline(enabled) if !self.busy() => {
                self.block_outline = enabled.then_some(self.block_outline_color);
                self.save_preferences();
            }
            Message::FishingWidthEnabled(enabled) if !self.busy() => {
                self.line_widths.fishing = enabled.then_some(self.fishing_line_width);
                self.save_preferences();
            }
            Message::OutlineWidthEnabled(enabled) if !self.busy() => {
                self.line_widths.outline = enabled.then_some(self.block_outline_width);
                self.save_preferences();
            }
            Message::FishingWidth(width)
                if !self.busy()
                    && width.is_finite()
                    && (fast_pack::compat::line_colors::MIN_WIDTH
                        ..=fast_pack::compat::line_colors::MAX_WIDTH)
                        .contains(&width) =>
            {
                self.fishing_line_width = (width * 10.0).round() / 10.0;
                if self.line_widths.fishing.is_some() {
                    self.line_widths.fishing = Some(self.fishing_line_width);
                }
                self.save_preferences();
            }
            Message::OutlineWidth(width)
                if !self.busy()
                    && width.is_finite()
                    && (fast_pack::compat::line_colors::MIN_WIDTH
                        ..=fast_pack::compat::line_colors::MAX_WIDTH)
                        .contains(&width) =>
            {
                self.block_outline_width = (width * 10.0).round() / 10.0;
                if self.line_widths.outline.is_some() {
                    self.line_widths.outline = Some(self.block_outline_width);
                }
                self.save_preferences();
            }
            Message::ResetCrosshairColor if !self.busy() => {
                self.rainbow.crosshair = false;
                self.crosshair_tint = None;
                self.edit_crosshair();
            }
            Message::CloseGlintPicker => self.close_glint_picker(),
            Message::GlintPreset(color) if !self.busy() => self.set_picker_color(color),
            Message::GlintSaturationValue(saturation, value) if !self.busy() => {
                let hsv = [
                    self.glint_hsv[0],
                    saturation.clamp(0.0, 1.0),
                    value.clamp(0.0, 1.0),
                ];
                self.set_picker_color(hsv_rgb(hsv));
                self.glint_hsv = hsv;
            }
            Message::GlintHue(hue) if !self.busy() => {
                let hsv = [hue.clamp(0.0, 1.0), self.glint_hsv[1], self.glint_hsv[2]];
                self.set_picker_color(hsv_rgb(hsv));
                self.glint_hsv = hsv;
            }
            Message::GlintHex(value) if !self.busy() => {
                if let Ok(color) = fast_pack::compat::glint::parse_color(&value) {
                    self.set_picker_color(color);
                }
                self.glint_hex = value;
            }
            Message::Output(output) if !self.busy() => {
                self.export_directory_input = output;
                self.directory_edited = self.auto_save_path.then_some(self.now);
                self.update_output();
                self.error = None;
            }
            Message::CommitExportDirectory if !self.busy() => {
                self.directory_edited = None;
                if let Err(error) = self.parsed_export_directory() {
                    self.error = Some(error.to_string());
                } else {
                    self.remember_export_directory();
                    self.update_output();
                    self.error = None;
                }
            }
            Message::Description(value) if !self.busy() => self.description = value,
            Message::PreserveLang(value) if !self.busy() => self.preserve_lang = value,
            Message::PreserveCase(value) if !self.busy() => self.preserve_case = value,
            Message::Zip(value) if !self.busy() => {
                self.as_zip = value;
                self.update_output();
            }
            Message::Swap if !self.busy() => {
                let source = if self.source_version == "自动检测" {
                    self.info
                        .as_ref()
                        .and_then(|info| info.version_label.clone())
                } else {
                    Some(self.source_version.clone())
                };
                if let Some(source) = source.filter(|label| {
                    tables()
                        .pack_formats
                        .iter()
                        .any(|(value, _)| value == label)
                }) {
                    self.source_version = self.target.clone();
                    self.target = source;
                    self.remember_target();
                    self.update_output();
                } else {
                    self.error = Some("请先指定具体源版本".into());
                }
            }
            Message::BrowseOutput if !self.busy() => {
                let directory = self.parsed_export_directory().ok().flatten().or_else(|| {
                    self.source
                        .as_ref()
                        .and_then(|path| path.parent().map(Path::to_owned))
                });
                return Task::perform(
                    async move {
                        let mut dialog =
                            rfd::AsyncFileDialog::new().set_title("Fast Pack · 选择导出目录");
                        if let Some(directory) = directory {
                            dialog = dialog.set_directory(directory);
                        }
                        dialog
                            .pick_folder()
                            .await
                            .map(|file| file.path().to_owned())
                    },
                    Message::OutputPicked,
                );
            }
            Message::OutputPicked(Some(path)) if !self.busy() => {
                self.export_directory_input = path.display().to_string();
                self.directory_edited = None;
                self.update_output();
                self.remember_export_directory();
                self.error = None;
            }
            Message::Advanced => {
                self.advanced = !self.advanced;
                self.advanced_motion.set(
                    if self.advanced { 1.0 } else { 0.0 },
                    self.now,
                    self.reduced,
                );
            }
            Message::Details => self.details = !self.details,
            Message::Mapping if !self.busy() => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Fast Pack · 自定义映射")
                            .add_filter("JSON", &["json"])
                            .pick_file()
                            .await
                            .map(|file| file.path().to_owned())
                    },
                    Message::MappingPicked,
                );
            }
            Message::MappingPicked(Some(path)) => {
                self.mapping_pending = true;
                let sender = self.sender.clone();
                std::thread::spawn(move || {
                    let result = load_extra_renames(&path).map_err(|error| format!("{error:#}"));
                    let _ = sender.send(WorkEvent::Mapping(result));
                });
            }
            Message::Start if !self.busy() && self.phase != Phase::Identifying => {
                for entry in &mut self.queue {
                    if entry.phase == Phase::Failed
                        && entry.info.as_ref().is_some_and(|info| info.ok)
                    {
                        entry.phase = Phase::Ready;
                        entry.submitted = false;
                        entry.progress = 0;
                        entry.error = None;
                        entry.step = "等待重试".into();
                    }
                }
                self.refresh_queue_phase();
                if let Err(error) = self.plan_queue_outputs(false) {
                    self.error = Some(error.to_string());
                    return Task::none();
                }
                if self
                    .queue
                    .iter()
                    .filter(|entry| entry.phase == Phase::Ready)
                    .any(|entry| entry.output.as_ref().is_some_and(|path| path.exists()))
                {
                    self.confirm_overwrite = true;
                } else {
                    self.start();
                }
            }
            Message::ConfirmOverwrite => {
                self.confirm_overwrite = false;
                self.start();
            }
            Message::CancelOverwrite => {
                self.confirm_overwrite = false;
                self.learn_more = false;
                self.close_glint_picker();
            }
            Message::NewPack if !self.busy() => {
                self.queue.clear();
                self.queue_mode = false;
                self.preview_request = None;
                self.preview_cache.clear();
                self.next_id += 1;
                self.auto_netherite = true;
                self.custom_crosshair = false;
                self.generation = self.next_id;
                self.sky_preview = None;
                self.sky_day_missing = false;
                self.sky_layers = 0;
                self.phase = Phase::Empty;
                self.source = None;
                self.info = None;
                self.report = None;
                self.error = None;
                self.logs.clear();
                self.output.clear();
                self.result_motion.set(0.0, self.now, self.reduced);
                return widget::operation::snap_to(
                    "fast-pack-content",
                    widget::operation::RelativeOffset {
                        x: Some(0.0),
                        y: Some(0.0),
                    },
                );
            }
            Message::SaveReport if self.report.is_some() => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Fast Pack · 保存报告")
                            .set_file_name("Fast Pack 转换报告.txt")
                            .add_filter("文本", &["txt"])
                            .save_file()
                            .await
                            .map(|file| file.path().to_owned())
                    },
                    Message::ReportPicked,
                );
            }
            Message::ReportPicked(Some(path)) => {
                if let Some(report) = &self.report
                    && let Err(error) = std::fs::write(path, report)
                {
                    self.error = Some(error.to_string());
                }
            }
            Message::OpenOutput => {
                let path = PathBuf::from(&self.output);
                let directory = if path.is_dir() {
                    path
                } else {
                    path.parent().unwrap_or(Path::new(".")).to_owned()
                };
                let _ = std::process::Command::new("explorer.exe")
                    .arg(directory)
                    .spawn();
            }
            Message::ClearLogs => self.logs.clear(),
            Message::LearnMore => self.learn_more = true,
            Message::CloseLearnMore => self.learn_more = false,
            Message::OpenSource => {
                let _ = std::process::Command::new("explorer.exe")
                    .arg("https://github.com/LongCold-ColdEternity-Team/Fast-Pack")
                    .spawn();
            }
            Message::Theme => {
                self.dark = !self.dark;
                self.theme_motion
                    .set(if self.dark { 1.0 } else { 0.0 }, self.now, self.reduced);
                self.save_preferences();
            }
            Message::ReducedMotion(value) => {
                self.reduced = value;
                if value {
                    self.progress.snap();
                    self.advanced_motion.snap();
                    self.result_motion.snap();
                    self.load_motion.snap();
                    self.theme_motion.snap();
                    self.auto_save_motion.snap();
                    self.auto_save_path_motion.snap();
                    self.heypixel_motion.snap();
                    self.glint_picker_motion.snap();
                    self.overlay.progress.snap();
                    for motion in &mut self.glint_color_motion {
                        motion.snap();
                    }
                    for value in self.hover.values_mut() {
                        value.snap();
                    }
                }
            }
            Message::Hover(id, value) => self
                .hover
                .entry(id)
                .or_insert_with(|| Tween::new(0.0, 150))
                .set(if value { 1.0 } else { 0.0 }, self.now, self.reduced),
            Message::Captured(screenshot) => {
                if let Some(path) = &self.capture {
                    let _ = save_screenshot(path, &screenshot);
                }
                return iced::exit();
            }
            _ => {}
        }
        self.item_editor
            .sync(&self.target, self.customization_source())
    }

    fn action<'a>(
        &self,
        id: u8,
        label: impl Into<String>,
        message: Option<Message>,
        primary: bool,
    ) -> Element<'a, Message> {
        let c = self.colors();
        let hover = self
            .hover
            .get(&id)
            .map(|value| value.value(self.now))
            .unwrap_or(0.0);
        let widget = button(text(label.into()).size(14))
            .padding([11, 17])
            .on_press_maybe(message)
            .style(move |_, status| {
                let base = if primary { c.accent } else { c.panel };
                let background = mix(
                    base,
                    if primary { c.foreground } else { c.accent },
                    hover * 0.1,
                );
                let disabled = status == button::Status::Disabled;
                button::Style {
                    background: Some(Background::Color(if disabled {
                        mix(background, c.background, 0.55)
                    } else {
                        background
                    })),
                    text_color: if disabled {
                        c.muted
                    } else if primary {
                        c.on_accent
                    } else {
                        c.foreground
                    },
                    border: Border {
                        radius: 10.0.into(),
                        width: if primary { 0.0 } else { 1.0 },
                        color: mix(c.line, c.accent, hover * 0.7),
                    },
                    ..Default::default()
                }
            });
        mouse_area(widget)
            .on_enter(Message::Hover(id, true))
            .on_exit(Message::Hover(id, false))
            .into()
    }

    fn label<'a>(&self, value: impl Into<String>) -> widget::Text<'a> {
        text(value.into()).size(12).color(self.colors().muted)
    }
    fn input_style(&self, status: widget::text_input::Status) -> widget::text_input::Style {
        let c = self.colors();
        let focused = matches!(status, widget::text_input::Status::Focused { .. });
        widget::text_input::Style {
            background: c.field.into(),
            border: Border {
                radius: 10.0.into(),
                width: 1.0,
                color: if focused { c.accent } else { c.line },
            },
            icon: c.muted,
            placeholder: c.muted,
            value: c.foreground,
            selection: mix(c.panel, c.accent, 0.25),
        }
    }
    fn picker_style(&self, status: widget::pick_list::Status, id: u8) -> widget::pick_list::Style {
        let c = self.colors();
        let hover = self
            .hover
            .get(&id)
            .map(|value| value.value(self.now))
            .unwrap_or(0.0);
        let active = matches!(
            status,
            widget::pick_list::Status::Opened { .. } | widget::pick_list::Status::Hovered
        );
        widget::pick_list::Style {
            text_color: c.foreground,
            placeholder_color: c.muted,
            handle_color: mix(c.muted, c.accent, hover * 0.65),
            background: c.field.into(),
            border: Border {
                radius: 10.0.into(),
                width: 1.0,
                color: if active || hover > 0.01 {
                    mix(c.line, c.accent, 0.55 + hover * 0.45)
                } else {
                    c.line
                },
            },
        }
    }
    fn title<'a>(&self, value: impl Into<String>, size: u32) -> widget::Text<'a> {
        text(value.into())
            .size(size)
            .font(Font {
                weight: if size >= 17 {
                    iced::font::Weight::Semibold
                } else {
                    iced::font::Weight::Medium
                },
                ..Font::with_name(crate::windows_compat::font_family())
            })
            .color(self.colors().foreground)
    }

    fn panel<'a>(&self, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
        let c = self.colors();
        container(content)
            .padding(24)
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(c.panel.into()),
                text_color: Some(c.foreground),
                border: Border {
                    color: c.line,
                    width: 1.0,
                    radius: 16.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    fn source_panel(&self) -> Element<'_, Message> {
        let c = self.colors();
        let hover = self
            .hover
            .get(&1)
            .map(|motion| motion.value(self.now))
            .unwrap_or(0.0);
        let name = self
            .source
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|value| value.to_string_lossy().into_owned());
        let caption = match self.phase {
            Phase::Identifying => "正在后台识别…",
            Phase::Empty => "支持 ZIP、RAR 或已解压文件夹",
            Phase::Failed if !self.info.as_ref().is_some_and(|info| info.ok) => {
                "未识别为有效的材质包"
            }
            _ => "原文件保留，转换结果另存",
        };
        let cube = widget::canvas(Artwork {
            color: c.accent,
            kind: ArtworkKind::Package,
        })
        .width(54)
        .height(56);
        let file_body = column![
            cube,
            self.title(
                name.as_deref()
                    .map(|name| shorten(name, 34))
                    .unwrap_or_else(|| "把材质包拖到这里，可一次拖入多个".into()),
                18
            ),
            self.label(caption),
            row![
                self.action(
                    2,
                    if self.source.is_some() {
                        "添加压缩包"
                    } else {
                        "选择压缩包"
                    },
                    Some(Message::OpenFile),
                    false
                ),
                self.action(3, "选择文件夹", Some(Message::OpenFolder), false)
            ]
            .spacing(10)
        ]
        .spacing(14)
        .align_x(Alignment::Center);
        let drop = container(file_body)
            .padding(24)
            .center_x(Length::Fill)
            .style(move |_| container::Style {
                background: Some(mix(c.field, c.accent, 0.03 + 0.07 * hover).into()),
                border: Border {
                    color: mix(c.line, c.accent, hover),
                    width: 1.0,
                    radius: 13.0.into(),
                },
                ..Default::default()
            });
        let mut content = column![
            self.title("源材质包", 14),
            mouse_area(drop)
                .on_enter(Message::Hover(1, true))
                .on_exit(Message::Hover(1, false))
        ]
        .spacing(18);
        if let Some(info) = &self.info {
            let format = info.version_label.as_deref().unwrap_or("自动检测");
            let mut versions = vec!["自动检测".to_string()];
            versions.extend(tables().pack_formats.iter().map(|(label, _)| label.clone()));
            let amount = self.load_motion.value(self.now);
            let mut muted = c.muted;
            muted.a = amount;
            let metadata = column![
                row![
                    text(format!("源版本 {format}")).size(12).color(muted),
                    space().width(Length::Fill),
                    text(format!(
                        "{} 文件 · {} 张纹理",
                        info.file_count, info.texture_count
                    ))
                    .size(12)
                    .color(muted)
                ]
                .spacing(8),
                mouse_area(
                    pick_list(
                        versions,
                        Some(self.source_version.clone()),
                        Message::SourceVersion
                    )
                    .width(Length::Fill)
                    .padding(10)
                    .style(|_, status| self.picker_style(status, 41))
                )
                .on_enter(Message::Hover(41, true))
                .on_exit(Message::Hover(41, false))
            ]
            .spacing(12);
            content = content.push(container(metadata).height(80.0 * amount).clip(true));
        }
        if let Some(path) = &self.source {
            content = content.push(
                tooltip(
                    self.label(shorten(&path.display().to_string(), 48)),
                    container(text(path.display().to_string()).size(12)).padding(10),
                    tooltip::Position::Bottom,
                )
                .style(move |_| hint_style(c)),
            );
        }
        if let Some(preview) = &self.sky_preview {
            content = content.push(
                column![
                    self.label(if self.sky_preview_daytime {
                        "白天天空预览"
                    } else {
                        "其他时段天空预览"
                    }),
                    widget::image(preview.clone())
                        .width(Length::Fill)
                        .height(160)
                        .content_fit(iced::ContentFit::Contain),
                    self.label(if self.sky_preview_daytime {
                        "正午时段 · 随目标版本自动转换"
                    } else {
                        "未找到白天天空 · 显示其他时段"
                    }),
                    self.label("天空显示可能不准确，能显示则是正常。")
                ]
                .spacing(8),
            );
        } else if self.sky_day_missing {
            content = content.push(self.label("未找到可预览的天空"));
        }
        self.panel(content)
    }

    fn queue_view(&self) -> Element<'_, Message> {
        let remaining = self
            .queue
            .iter()
            .filter(|entry| entry.phase != Phase::Done)
            .count();
        if remaining == 0 {
            return space().height(0).into();
        }
        let c = self.colors();
        let done = self
            .queue
            .iter()
            .filter(|entry| entry.phase == Phase::Done)
            .count();
        let failed = self
            .queue
            .iter()
            .filter(|entry| entry.phase == Phase::Failed)
            .count();
        let mut entries = column![].spacing(8);
        for entry in self.queue.iter().filter(|entry| entry.phase != Phase::Done) {
            let selected = entry.id == self.generation;
            let name = entry
                .source
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            let status = match entry.phase {
                Phase::Identifying => "正在识别".into(),
                Phase::Ready if entry.submitted => "等待并行槽位".into(),
                Phase::Ready => "等待转换".into(),
                Phase::Running => format!("{}% · {}", entry.progress, entry.step),
                Phase::Done => "已完成".into(),
                Phase::Failed => format!("失败：{}", entry.error.as_deref().unwrap_or("无法转换")),
                Phase::Empty => "等待导入".into(),
            };
            let tone = match entry.phase {
                Phase::Done => c.success,
                Phase::Failed => c.danger,
                _ => c.muted,
            };
            let label = column![
                text(shorten(&name, 54)).size(14).color(c.foreground),
                text(shorten(&status, 76)).size(12).color(tone)
            ]
            .spacing(5)
            .width(Length::Fill);
            let pick = button(label)
                .padding([2, 0])
                .width(Length::Fill)
                .on_press(Message::QueueSelect(entry.id))
                .style(move |_, _| widget::button::Style {
                    background: None,
                    text_color: c.foreground,
                    ..Default::default()
                });
            let line = row![pick].align_y(Alignment::Center);
            let mut item = column![line].spacing(9);
            if entry.phase == Phase::Running {
                item = item.push(
                    progress_bar(0.0..=100.0, f32::from(entry.progress))
                        .girth(3)
                        .style(move |_| widget::progress_bar::Style {
                            background: c.line.into(),
                            bar: c.accent.into(),
                            border: Border {
                                radius: 2.0.into(),
                                ..Default::default()
                            },
                        }),
                );
            }
            let card = container(item)
                .padding([10, 12])
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(
                        mix(c.field, c.accent, if selected { 0.045 } else { 0.0 }).into(),
                    ),
                    border: Border {
                        radius: 10.0.into(),
                        width: 1.0,
                        color: mix(c.line, c.accent, if selected { 0.50 } else { 0.0 }),
                    },
                    ..Default::default()
                });
            entries = entries.push(card);
        }
        let mut content = column![
            row![
                self.title(format!("材质包列表 · {remaining} 个"), 14),
                space().width(Length::Fill),
                self.label(format!("完成 {done} · 失败 {failed}")),
            ]
            .align_y(Alignment::Center),
            row![
                self.action(44, "添加压缩包", Some(Message::OpenFile), false),
                self.action(46, "添加文件夹", Some(Message::OpenFolder), false),
                self.action(
                    45,
                    "清空",
                    (!self.busy()).then_some(Message::QueueClear),
                    false
                ),
                self.action(
                    47,
                    "删除选中",
                    self.can_remove_selected()
                        .then_some(Message::QueueRemoveSelected),
                    false
                ),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            self.label(format!(
                "同时最多转换 {} 个 · 再次拖入会追加",
                fast_pack::batch::parallelism()
            )),
            scrollable(entries)
                .height((remaining as f32 * 74.0).min(340.0))
                .spacing(10),
        ]
        .spacing(12);
        if let Some(info) = &self.info {
            content = content.push(self.label(format!(
                "当前：{} · {} 张纹理",
                info.version_label.as_deref().unwrap_or("自动检测"),
                info.texture_count
            )));
        }
        if let Some(preview) = &self.sky_preview {
            content = content
                .push(self.label(if self.sky_preview_daytime {
                    "白天天空预览"
                } else {
                    "其他时段天空预览"
                }))
                .push(
                    widget::image(preview.clone())
                        .width(Length::Fill)
                        .height(120)
                        .content_fit(iced::ContentFit::Contain),
                )
                .push(self.label("天空显示可能不准确，能显示则是正常。"));
        }
        self.panel(content)
    }

    fn memory_switch<'a>(
        &self,
        id: u8,
        label: &'a str,
        enabled: bool,
        amount: f32,
        message: Message,
        hint: &'a str,
    ) -> Element<'a, Message> {
        let c = self.colors();
        let hover = self
            .hover
            .get(&id)
            .map(|value| value.value(self.now))
            .unwrap_or(0.0);
        let content = row![
            text(label).size(12).color(c.foreground),
            space().width(Length::Fill),
            text(if enabled { "开" } else { "关" })
                .size(10)
                .color(mix(c.muted, c.accent, amount)),
            widget::canvas(AutoSaveSwitch {
                amount,
                hover,
                colors: c,
                disabled: self.busy(),
            })
            .width(58)
            .height(34)
        ]
        .spacing(5)
        .align_y(Alignment::Center);
        let control = button(content)
            .padding([4, 8])
            .width(Length::Fill)
            .on_press_maybe((!self.busy()).then_some(message))
            .style(move |_, _| button::Style {
                background: Some(mix(c.field, c.accent, amount * 0.035 + hover * 0.045).into()),
                text_color: c.foreground,
                border: Border {
                    color: mix(c.line, c.accent, amount * 0.3 + hover * 0.35),
                    width: 1.0,
                    radius: 12.0.into(),
                },
                ..Default::default()
            });
        tooltip(
            mouse_area(control)
                .on_enter(Message::Hover(id, true))
                .on_exit(Message::Hover(id, false)),
            container(text(hint).size(12)).padding(9).max_width(320),
            tooltip::Position::Bottom,
        )
        .style(move |_| hint_style(c))
        .into()
    }

    fn memory_controls(&self) -> Element<'_, Message> {
        row![
            self.label("自动保存"),
            self.memory_switch(
                34,
                "版本",
                self.auto_save,
                self.auto_save_motion.value(self.now),
                Message::AutoSave(!self.auto_save),
                "记住目标版本。关闭后本次版本保留，重启时恢复自动推荐。",
            ),
            self.memory_switch(
                35,
                "导出目录",
                self.auto_save_path,
                self.auto_save_path_motion.value(self.now),
                Message::AutoSavePath(!self.auto_save_path),
                "只记住导出文件夹，每个包自动生成新文件名。关闭后重启恢复保存在源包旁。",
            ),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    }

    fn target_panel(&self) -> Element<'_, Message> {
        let c = self.colors();
        let versions: Vec<_> = tables()
            .pack_formats
            .iter()
            .map(|(label, _)| label.clone())
            .collect();
        let mut output = text_input("留空时保存在源材质包旁", &self.export_directory_input)
            .padding(10)
            .size(13)
            .style(|_, status| self.input_style(status));
        if !self.busy() {
            output = output
                .on_input(Message::Output)
                .on_submit(Message::CommitExportDirectory);
        }
        let filename = if self.output.is_empty() {
            "文件名按版本和原包名自动生成".into()
        } else {
            format!(
                "将导出 {}",
                shorten(
                    &Path::new(&self.output)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                    28,
                )
            )
        };
        let preview = tooltip(
            self.label(filename),
            container(
                text(if self.output.is_empty() {
                    "导入材质包后自动生成文件名"
                } else {
                    &self.output
                })
                .size(12),
            )
            .padding(9)
            .max_width(420),
            tooltip::Position::Bottom,
        )
        .style(move |_| hint_style(c));
        let mut content = column![
            self.title("转换目标", 14),
            self.label(
                if fast_pack::tables::version_profile(&self.target)
                    .is_some_and(|profile| profile.kind.starts_with("snapshot"))
                {
                    "目标游戏版本 · 26.4 当前为官方快照"
                } else {
                    "目标游戏版本"
                }
            ),
            row![
                pick_list(versions, Some(self.target.clone()), Message::Target)
                    .width(Length::Fill)
                    .padding(11)
                    .style(|_, status| self.picker_style(status, 40)),
                tooltip(
                    self.action(
                        5,
                        "⇄",
                        (!self.busy() && self.info.is_some()).then_some(Message::Swap),
                        false,
                    ),
                    container(text("交换源版本与目标版本").size(12)).padding(9),
                    tooltip::Position::Top,
                )
                .delay(Duration::from_millis(350))
                .style(move |_| hint_style(c)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            self.label("导出目录"),
            row![
                output,
                self.action(
                    6,
                    "选择",
                    (!self.busy()).then_some(Message::BrowseOutput),
                    false,
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            preview,
            self.memory_controls(),
        ]
        .spacing(13);
        if self.target == "1.20.1" && fast_pack::client_audio::ENABLED {
            content = content.push(self.memory_switch(
                36,
                "布吉岛高版本材质包适配",
                self.heypixel_compat,
                self.heypixel_motion.value(self.now),
                Message::HeyPixel(!self.heypixel_compat),
                "自动适配物品、配置官方名称汉化并导入转换包。原声音效仍需服务器或客户端支持。关闭后本次按普通材质包转换。",
            ));
        }
        self.panel(content)
    }

    fn glint_controls(&self) -> Element<'_, Message> {
        row![
            checkbox(self.custom_glint)
                .label("自定义附魔颜色")
                .size(17)
                .text_size(13)
                .on_toggle_maybe((!self.busy()).then_some(Message::CustomGlint)),
            self.palette_button(33, ColorTarget::Glint, !self.busy()),
            space().width(Length::Fill)
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
    }

    fn glint_dialog(&self) -> Element<'_, Message> {
        let c = self.colors();
        let target = self.color_target;
        let dynamic_controls: Element<'_, Message> =
            if matches!(target, ColorTarget::Description | ColorTarget::PackName) {
                self.label(
                    if target == ColorTarget::PackName
                        || !fast_pack::overlay::supports_hex(&self.target)
                    {
                        "直接调色修改整段；选字后只改选中部分。此处匹配游戏支持的 16 色。"
                    } else {
                        "直接调色会修改整段；选中文字时只修改选中部分。"
                    },
                )
                .into()
            } else if target == ColorTarget::NameBackground
                && !self.name_background_rainbow_supported()
            {
                self.label(self.rainbow_hint(target)).into()
            } else {
                column![
                    checkbox(self.rainbow_for(target))
                        .label("动态彩色")
                        .size(17)
                        .text_size(13)
                        .on_toggle_maybe((!self.busy()).then_some(Message::RainbowColor)),
                    self.label(self.rainbow_hint(target))
                ]
                .spacing(12)
                .into()
            };
        let presets = [
            ("白色", [255, 255, 255]),
            ("经典紫", fast_pack::compat::glint::VANILLA_COLOR),
            ("冰蓝", [91, 217, 255]),
            ("薄荷绿", [105, 239, 172]),
            ("金色", [255, 201, 92]),
            ("粉色", [255, 134, 204]),
            ("红色", [255, 86, 100]),
            ("黑色", [0; 3]),
        ];
        let palette = row(presets.into_iter().map(|(name, color)| {
            let selected = self.picker_color() == color;
            let swatch = container(space().width(26).height(26)).style(move |_| container::Style {
                background: Some(Color::from_rgb8(color[0], color[1], color[2]).into()),
                border: Border {
                    radius: 8.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            });
            let choice = button(swatch)
                .padding(5)
                .on_press(Message::GlintPreset(color))
                .style(move |_, status| button::Style {
                    background: Some(
                        if selected {
                            mix(c.field, c.accent, 0.12)
                        } else {
                            c.field
                        }
                        .into(),
                    ),
                    border: Border {
                        radius: 12.0.into(),
                        width: if selected || status == button::Status::Hovered {
                            2.0
                        } else {
                            1.0
                        },
                        color: if selected || status == button::Status::Hovered {
                            c.accent
                        } else {
                            c.line
                        },
                    },
                    ..Default::default()
                });
            tooltip(choice, text(name).size(12), tooltip::Position::Top)
                .style(move |_| hint_style(c))
                .into()
        }))
        .spacing(12);
        let mut controls = column![
            self.label("自定义色值"),
            text_input("#FFFFFF", &self.glint_hex)
                .on_input(Message::GlintHex)
                .padding(10)
                .size(15)
                .style(|_, status| self.input_style(status))
        ]
        .spacing(12);
        controls = controls.push(self.label(format!(
            "RGB  {} · {} · {}",
            self.picker_color()[0],
            self.picker_color()[1],
            self.picker_color()[2]
        )));
        let valid = fast_pack::compat::glint::parse_color(&self.glint_hex).is_ok();
        controls = controls.push(
            text(if valid {
                if target == ColorTarget::ItemOutline {
                    "点击完成后保存此物品设置"
                } else {
                    "颜色自动保存"
                }
            } else {
                "输入 3 位或 6 位 HEX 色值"
            })
            .size(12)
            .color(if valid { c.muted } else { c.danger }),
        );
        let color = if self.rainbow_for(target) {
            let rgb = self.rainbow_preview_color();
            Color::from_rgb8(rgb[0], rgb[1], rgb[2])
        } else {
            Color {
                r: self.glint_color_motion[0].value(self.now),
                g: self.glint_color_motion[1].value(self.now),
                b: self.glint_color_motion[2].value(self.now),
                a: 1.0,
            }
        };
        let preview: Element<'_, Message> = if target == ColorTarget::Glint {
            column![
                widget::canvas(GlintPreview {
                    color,
                    colors: c,
                    elapsed: if self.reduced {
                        0.0
                    } else {
                        self.now
                            .saturating_duration_since(self.glint_preview_started)
                            .as_secs_f32()
                    },
                })
                .width(116)
                .height(108),
                self.label("颜色示意")
            ]
            .spacing(8)
            .align_x(Alignment::Center)
            .into()
        } else if target == ColorTarget::NameBackground {
            column![
                container(text("玩家昵称").size(18).color(Color::WHITE))
                    .padding([18, 8])
                    .width(116)
                    .height(108)
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(iced::alignment::Vertical::Center)
                    .style(move |_| container::Style {
                        background: Some(Color { a: 0.7, ..color }.into()),
                        border: Border {
                            radius: 10.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                self.label("白字配色预览")
            ]
            .spacing(8)
            .align_x(Alignment::Center)
            .into()
        } else {
            column![
                container(space().width(116).height(108)).style(move |_| container::Style {
                    background: Some(color.into()),
                    border: Border {
                        radius: 10.0.into(),
                        width: 1.0,
                        color: c.line
                    },
                    ..Default::default()
                }),
                self.label("颜色示意")
            ]
            .spacing(8)
            .align_x(Alignment::Center)
            .into()
        };
        let apply: Element<'_, Message> = if target == ColorTarget::Glint {
            checkbox(self.custom_glint)
                .label(if self.overlay_mode {
                    "制作时应用自定义颜色"
                } else {
                    "转换时应用自定义颜色"
                })
                .text_size(13)
                .on_toggle(Message::CustomGlint)
                .into()
        } else {
            self.label(
                if target == ColorTarget::Crosshair
                    && self.crosshair_drawing
                    && self.rainbow.crosshair
                {
                    "动态彩色作用于导出的整个准星，画布像素和笔色保留。"
                } else if target == ColorTarget::Crosshair && self.crosshair_drawing {
                    "当前颜色用于画笔，已画好的像素保持原样。"
                } else {
                    "颜色设置只保存在本机。"
                },
            )
            .into()
        };
        container(
            self.panel(
                column![
                    row![
                        self.title(target.title(), 22),
                        space().width(Length::Fill),
                        self.action(34, "完成", Some(Message::CloseGlintPicker), true)
                    ]
                    .align_y(Alignment::Center),
                    self.label(if target == ColorTarget::Glint {
                        "选择喜欢的颜色；取消勾选自定义颜色恢复原版紫色。"
                    } else {
                        "拖动色卡选择颜色与明暗，右侧色相条切换色系。"
                    }),
                    palette,
                    dynamic_controls,
                    row![
                        column![
                            widget::canvas(GlintColorCard {
                                hsv: self.glint_hsv,
                                colors: c
                            })
                            .width(272)
                            .height(228),
                            self.label("左侧选颜色与明暗 · 右侧切换色相")
                        ]
                        .spacing(8),
                        column![preview, controls].spacing(14).width(Length::Fill)
                    ]
                    .spacing(18)
                    .align_y(Alignment::Center),
                    apply
                ]
                .spacing(18),
            ),
        )
        .width(self.width.min(640.0) - 56.0)
        .into()
    }

    fn crosshair_controls(&self) -> Element<'_, Message> {
        let c = self.colors();
        let enabled = !self.busy() && !self.crosshair_pending;
        let attack_supported =
            !self.overlay_mode || pack_format_of(&self.target).is_ok_and(|format| format >= 2);
        let mut content = column![
            row![
                checkbox(self.custom_crosshair)
                    .label("自定义准星")
                    .size(17)
                    .text_size(13)
                    .on_toggle_maybe((!self.busy()).then_some(Message::CustomCrosshair)),
                checkbox(self.show_attack_indicator)
                    .label("显示攻击冷却进度条")
                    .size(17)
                    .text_size(13)
                    .on_toggle_maybe(
                        (!self.busy() && attack_supported).then_some(Message::ShowAttackIndicator)
                    ),
                space().width(Length::Fill),
                self.action(
                    60,
                    "导入预设",
                    enabled.then_some(Message::CrosshairImport),
                    false
                ),
                self.action(
                    61,
                    "导入文件夹",
                    enabled.then_some(Message::CrosshairFolder),
                    false
                )
            ]
            .spacing(10)
            .align_y(Alignment::Center)
        ]
        .spacing(12);
        content = content.push(self.label("攻击冷却指示支持 1.9+，显示位置沿用游戏设置。"));
        if !self.custom_crosshair {
            content = content.push(self.label(format!(
                "{} 个准星预设 · 开启后应用本次准星{}",
                self.crosshair_presets.len(),
                if self.overlay_mode {
                    ""
                } else {
                    "，转换结束自动关闭"
                }
            )));
        } else {
            content = content.push(
                row![
                    self.label(if self.crosshair_drawing {
                        "画笔颜色"
                    } else {
                        "准星颜色"
                    }),
                    self.palette_button(93, ColorTarget::Crosshair, enabled),
                    self.action(
                        97,
                        "原色",
                        (enabled
                            && !self.crosshair_drawing
                            && (self.crosshair_tint.is_some() || self.rainbow.crosshair))
                            .then_some(Message::ResetCrosshairColor),
                        false
                    )
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            );
            content = content.push(
                row![
                    self.action(
                        62,
                        "使用预设",
                        enabled.then_some(Message::CrosshairDrawing(false)),
                        !self.crosshair_drawing
                    ),
                    self.action(
                        63,
                        "自己画",
                        enabled.then_some(Message::CrosshairDrawing(true)),
                        self.crosshair_drawing
                    ),
                    self.action(
                        67,
                        "恢复内置预设",
                        enabled.then_some(Message::CrosshairRestoreBuiltins),
                        false
                    ),
                    space().width(Length::Fill),
                    self.label(if self.crosshair_pending {
                        "正在处理…".into()
                    } else {
                        format!("{} 个预设", self.crosshair_presets.len())
                    })
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            );
            let preview = container(
                widget::image(self.crosshair_preview.clone())
                    .width(96)
                    .height(96)
                    .filter_method(widget::image::FilterMethod::Nearest),
            )
            .padding(12)
            .width(120)
            .height(120)
            .center_x(120)
            .center_y(120)
            .style(move |_| container::Style {
                background: Some(c.field.into()),
                border: Border {
                    color: c.line,
                    width: 1.0,
                    radius: 12.0.into(),
                },
                ..Default::default()
            });
            if self.crosshair_drawing {
                let board = widget::canvas(CrosshairCanvas {
                    canvas: &self.crosshair_canvas,
                    colors: c,
                    disabled: !enabled,
                })
                .width(240)
                .height(240);
                let mut name = text_input("给准星起个名字", &self.crosshair_name)
                    .padding(8)
                    .size(13)
                    .style(|_, status| self.input_style(status));
                let mut color = text_input("#FFFFFF", &self.crosshair_color)
                    .padding(8)
                    .size(13)
                    .width(120)
                    .style(|_, status| self.input_style(status));
                if enabled {
                    name = name.on_input(Message::CrosshairName);
                    color = color.on_input(Message::CrosshairColor);
                }
                let mut palette = row![].spacing(7);
                for rgb in [0xffffff, 0x000000, 0xff5656, 0x6be0f3, 0x7cfa85, 0xffdc65] {
                    let color = crate::gui::rgb(rgb);
                    palette = palette.push(
                        button(space().width(17).height(17))
                            .padding(4)
                            .on_press_maybe(
                                enabled.then(|| Message::CrosshairColor(format!("#{rgb:06X}"))),
                            )
                            .style(move |_, status| button::Style {
                                background: Some(color.into()),
                                border: Border {
                                    color: if status == button::Status::Hovered {
                                        c.accent
                                    } else {
                                        c.line
                                    },
                                    width: 1.0,
                                    radius: 5.0.into(),
                                },
                                ..Default::default()
                            }),
                    );
                }
                let tools = column![
                    row![
                        preview,
                        column![
                            self.label("准星名称"),
                            name,
                            self.label("实时预览 · 透明区域不会显示")
                        ]
                        .spacing(8)
                        .width(Length::Fill)
                    ]
                    .spacing(14)
                    .align_y(Alignment::Center),
                    row![
                        color,
                        self.label("网格"),
                        pick_list(
                            [15u8, 30],
                            Some(self.crosshair_canvas.size),
                            Message::CrosshairSize
                        )
                        .padding(7)
                        .width(70)
                        .style(|_, status| self.picker_style(status, 64))
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                    palette,
                    row![
                        self.action(
                            68,
                            "画笔",
                            enabled.then_some(Message::CrosshairBrush(false)),
                            !self.crosshair_eraser
                        ),
                        self.action(
                            69,
                            "橡皮",
                            enabled.then_some(Message::CrosshairBrush(true)),
                            self.crosshair_eraser
                        ),
                        self.action(
                            70,
                            "撤销",
                            (enabled && !self.crosshair_undo.is_empty())
                                .then_some(Message::CrosshairUndo),
                            false
                        ),
                        self.action(
                            71,
                            "清空",
                            (enabled && !self.crosshair_canvas.is_empty())
                                .then_some(Message::CrosshairClear),
                            false
                        )
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                    self.action(
                        65,
                        "保存为预设",
                        (enabled && !self.crosshair_canvas.is_empty())
                            .then_some(Message::CrosshairSave),
                        false
                    )
                ]
                .spacing(10)
                .width(Length::Fill);
                content = content.push(
                    row![
                        container(board)
                            .padding(1)
                            .style(move |_| container::Style {
                                border: Border {
                                    color: c.line,
                                    width: 1.0,
                                    radius: 2.0.into()
                                },
                                ..Default::default()
                            }),
                        tools
                    ]
                    .spacing(18)
                    .align_y(Alignment::Center),
                );
                content = content.push(self.label("左键逐格涂色，可按住拖动；右键擦除。画布草稿自动保存，选择的预设也能继续编辑。"));
            } else {
                let name = self
                    .crosshair_presets
                    .iter()
                    .find(|preset| Some(&preset.id) == self.crosshair_selected.as_ref())
                    .map(|preset| preset.crosshair.name.as_str())
                    .unwrap_or("尚未选择预设");
                let details = column![
                    self.title(name, 14),
                    self.label("预览供参考，实际颜色随游戏背景变化。"),
                    row![
                        self.action(
                            66,
                            "删除选中预设",
                            (enabled && self.crosshair_selected.is_some())
                                .then_some(Message::CrosshairDelete),
                            false
                        ),
                        self.action(
                            72,
                            "在画布上编辑",
                            (enabled && self.crosshair_selected.is_some())
                                .then_some(Message::CrosshairEdit),
                            false
                        )
                    ]
                    .spacing(10)
                ]
                .spacing(12)
                .width(260);
                content = content.push(
                    row![details, preview]
                        .spacing(16)
                        .align_y(Alignment::Center),
                );
                if self.crosshair_presets.is_empty() {
                    content = content.push(self.label("可以导入准星包、自己画，或恢复内置预设。"));
                } else {
                    let columns = ((self.width - 170.0) / 106.0).floor().clamp(3.0, 8.0) as usize;
                    let mut gallery = column![].spacing(10);
                    for start in (0..self.crosshair_presets.len()).step_by(columns) {
                        let mut tiles = row![].spacing(10);
                        for index in start..(start + columns).min(self.crosshair_presets.len()) {
                            let preset = &self.crosshair_presets[index];
                            let selected = self.crosshair_selected.as_ref() == Some(&preset.id);
                            let card = button(
                                container(
                                    widget::image(self.crosshair_previews[index].clone())
                                        .width(46)
                                        .height(46)
                                        .filter_method(widget::image::FilterMethod::Nearest),
                                )
                                .width(96)
                                .height(96)
                                .center_x(Length::Fill)
                                .center_y(Length::Fill),
                            )
                            .padding(0)
                            .width(96)
                            .height(96)
                            .on_press_maybe(
                                enabled.then(|| Message::CrosshairSelected(preset.id.clone())),
                            )
                            .style(move |_, status| button::Style {
                                background: Some(
                                    mix(
                                        c.field,
                                        c.accent,
                                        if selected {
                                            0.11
                                        } else if status == button::Status::Hovered {
                                            0.06
                                        } else {
                                            0.0
                                        },
                                    )
                                    .into(),
                                ),
                                border: Border {
                                    color: if selected { c.accent } else { c.line },
                                    width: 1.0,
                                    radius: 10.0.into(),
                                },
                                text_color: c.foreground,
                                ..Default::default()
                            });
                            tiles = tiles.push(card);
                        }
                        gallery = gallery.push(tiles);
                    }
                    let height = (self.crosshair_presets.len().div_ceil(columns) as f32 * 106.0
                        - 10.0)
                        .min(212.0);
                    content = content.push(smooth_scroll::scroll(
                        scrollable(gallery)
                            .direction(scrollable::Direction::Vertical(
                                scrollable::Scrollbar::new()
                                    .width(6)
                                    .scroller_width(6)
                                    .spacing(12),
                            ))
                            .height(height)
                            .id("crosshair-gallery"),
                        "crosshair-gallery",
                        self.reduced,
                    ));
                }
            }
            content = content.push(self.label(
                "内置预设随程序提供，导入与自绘预设存在 C 盘。功能每次转换结束和重新打开后关闭。",
            ));
        }
        if !self.crosshair_note.is_empty() {
            content = content.push(self.label(&self.crosshair_note));
        }
        container(content)
            .padding(14)
            .style(move |_| container::Style {
                border: Border {
                    color: c.line,
                    width: 1.0,
                    radius: 12.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    fn advanced_view(&self) -> Element<'_, Message> {
        let amount = self.advanced_motion.value(self.now);
        if amount < 0.001 {
            return space().height(0).into();
        }
        let mut input = text_input("留空则沿用原包", &self.description)
            .padding(12)
            .size(14)
            .style(|_, status| self.input_style(status));
        if !self.busy() {
            input = input.on_input(Message::Description);
        }
        let lang = checkbox(self.preserve_lang)
            .label("保留语言格式")
            .size(17)
            .text_size(13)
            .on_toggle_maybe((!self.busy()).then_some(Message::PreserveLang));
        let case = checkbox(self.preserve_case)
            .label("保留文件名大小写")
            .size(17)
            .text_size(13)
            .on_toggle_maybe((!self.busy()).then_some(Message::PreserveCase));
        let reduced = checkbox(self.reduced)
            .label("减少动态效果")
            .size(17)
            .text_size(13)
            .on_toggle(Message::ReducedMotion);
        let zip = checkbox(self.as_zip)
            .label("输出为 ZIP")
            .size(17)
            .text_size(13)
            .on_toggle_maybe((!self.busy()).then_some(Message::Zip));
        let netherite = checkbox(self.auto_netherite)
            .label("自动制作下界合金材质")
            .size(17)
            .text_size(13)
            .on_toggle_maybe((!self.busy()).then_some(Message::AutoNetherite));
        let options: Element<'_, Message> = if self.width < 940.0 {
            column![
                zip,
                lang,
                case,
                row![reduced, netherite]
                    .spacing(22)
                    .align_y(Alignment::Center)
            ]
            .spacing(12)
            .into()
        } else {
            row![zip, lang, case, reduced, netherite]
                .spacing(22)
                .align_y(Alignment::Center)
                .into()
        };
        let content = column![
            self.title("更多转换选项", 14),
            self.label("材质包描述"),
            input,
            options,
            self.glint_controls(),
            self.line_color_controls(),
            self.crosshair_controls(),
            self.items_view(),
            self.builtin_overlay_controls(),
            row![
                self.action(
                    8,
                    "导入自定义映射",
                    (!self.busy()).then_some(Message::Mapping),
                    false
                ),
                self.label(format!("{} 条映射", self.extra.len()))
            ]
            .spacing(14)
            .align_y(Alignment::Center)
        ]
        .spacing(14);
        container(self.panel(content))
            .height(if amount >= 0.999 {
                Length::Shrink
            } else {
                Length::Fixed(
                    (if self.width < 940.0 { 540.0 } else { 426.0 }
                        + if self.custom_crosshair { 480.0 } else { 0.0 })
                        * amount,
                )
            })
            .clip(true)
            .into()
    }

    fn result_view(&self) -> Element<'_, Message> {
        if self.phase != Phase::Done && self.error.is_none() {
            return space().height(0).into();
        }
        let c = self.colors();
        let amount = self
            .result_motion
            .value(self.now)
            .max(if self.error.is_some() { 1.0 } else { 0.0 });
        let failed = self.error.is_some();
        let tone = if failed { c.danger } else { c.success };
        let caption = if let Some(error) = &self.error {
            error.clone()
        } else if let Some(note) = &self.result_note {
            note.clone()
        } else {
            "原文件已保留，转换详情可在程序内查看。".into()
        };
        let icon = widget::canvas(Artwork {
            color: tone,
            kind: if failed {
                ArtworkKind::Error
            } else {
                ArtworkKind::Check
            },
        })
        .width(36)
        .height(38);
        let mut content = column![
            row![
                icon,
                column![
                    text(if failed {
                        "需要处理"
                    } else {
                        "转换完成"
                    })
                    .size(17)
                    .color(tone),
                    self.label(caption)
                ]
                .spacing(6)
            ]
            .spacing(14)
            .align_y(Alignment::Center)
        ]
        .spacing(16);
        if !failed {
            content = content.push(
                row![
                    self.action(20, "打开输出目录", Some(Message::OpenOutput), false),
                    self.action(21, "保存报告", Some(Message::SaveReport), false)
                ]
                .spacing(10),
            );
        }
        container(self.panel(content))
            .height(if failed {
                Length::Shrink
            } else {
                Length::Fixed(158.0 * amount)
            })
            .clip(true)
            .into()
    }

    fn view(&self) -> Element<'_, Message> {
        let c = self.colors();
        let logo = widget::image(brand_handle())
            .width(38)
            .height(38)
            .content_fit(iced::ContentFit::Contain);
        let navigation = container(
            row![
                self.action(
                    110,
                    "材质包转换",
                    (!self.busy()).then_some(Message::Mode(false)),
                    !self.overlay_mode
                ),
                self.action(
                    111,
                    "覆盖包制作",
                    (!self.busy()).then_some(Message::Mode(true)),
                    self.overlay_mode
                )
            ]
            .spacing(5),
        )
        .padding(4)
        .style(move |_| container::Style {
            background: Some(c.field.into()),
            border: Border {
                radius: 13.0.into(),
                width: 1.0,
                color: c.line,
            },
            ..Default::default()
        });
        let header = container(
            column![
                row![
                    logo,
                    column![
                        self.title("Fast Pack", 18),
                        self.label("Minecraft Java 资源包转换")
                    ]
                    .spacing(3),
                    space().width(Length::Fill),
                    self.action(
                        10,
                        if self.dark { "浅色" } else { "深色" },
                        Some(Message::Theme),
                        false
                    ),
                    self.action(18, "了解详细", Some(Message::LearnMore), false)
                ]
                .spacing(12)
                .align_y(Alignment::Center),
                navigation
            ]
            .spacing(14),
        )
        .padding([18, 28]);
        let mut body = if self.overlay_mode {
            self.overlay_body()
        } else {
            let mut intro = column![self.title(
                if self.phase == Phase::Done {
                    "转换完成"
                } else {
                    "材质包转换"
                },
                28
            )]
            .spacing(9);
            if self.phase == Phase::Done {
                intro = intro.push(self.label("转换结果已保存，原材质包没有改动。"));
            }
            let source_panel = if self.queue_mode || self.queue.len() > 1 {
                self.queue_view()
            } else {
                self.source_panel()
            };
            let panels: Element<'_, Message> = if self.width < 940.0 {
                column![source_panel, self.target_panel()]
                    .spacing(18)
                    .into()
            } else {
                row![
                    container(source_panel).width(Length::FillPortion(1)),
                    container(self.target_panel()).width(Length::FillPortion(1))
                ]
                .spacing(20)
                .into()
            };
            let mut body = column![intro].spacing(20);
            if self.phase == Phase::Done {
                body = body.push(self.result_view()).push(self.queue_view());
                if self.queue.len() < 2 {
                    body = body.push(
                        self.panel(
                            column![
                                self.title("输出材质包", 14),
                                self.title(
                                    Path::new(&self.output)
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy(),
                                    18
                                ),
                                self.label(self.output.clone()),
                                self.label(format!("目标 Minecraft {}", self.target))
                            ]
                            .spacing(12),
                        ),
                    );
                }
            } else {
                body = body
                    .push(panels)
                    .push(self.action(
                        7,
                        if self.advanced {
                            "收起转换选项 ↑"
                        } else {
                            "更多转换选项 ↓"
                        },
                        Some(Message::Advanced),
                        false,
                    ))
                    .push(self.advanced_view());
                if self.error.is_some() {
                    body = body.push(self.result_view());
                }
            }
            body
        };
        let fixed_progress: Element<'_, Message> = if self.busy() {
            let (value, step) = if self.overlay_mode {
                (
                    self.overlay
                        .progress
                        .value(self.now)
                        .min(self.overlay.percent as f32),
                    self.overlay.step.as_str(),
                )
            } else {
                (
                    self.progress
                        .value(self.now)
                        .min(self.actual_progress as f32),
                    self.step.as_str(),
                )
            };
            let progress = progress_bar(0.0..=100.0, value).girth(6).style(move |_| {
                widget::progress_bar::Style {
                    background: c.line.into(),
                    bar: c.accent.into(),
                    border: Border {
                        radius: 4.0.into(),
                        ..Default::default()
                    },
                }
            });
            container(
                column![
                    row![
                        self.label(step),
                        space().width(Length::Fill),
                        self.label(format!("{}%", value.round() as u8))
                    ]
                    .align_y(Alignment::Center),
                    progress
                ]
                .spacing(8)
                .width(Length::Fill),
            )
            .padding([10, 28])
            .width(Length::Fill)
            .into()
        } else {
            space().height(0).into()
        };
        let status = if self.overlay_mode {
            if self.overlay.running {
                "正在本机制作覆盖包"
            } else if self.overlay.result.is_some() {
                "覆盖包已保存 · 放在其他材质包上方使用"
            } else {
                "独立制作覆盖包 · 无需导入材质包"
            }
        } else {
            match self.phase {
                Phase::Empty => "数据仅在本机处理",
                Phase::Identifying => "正在后台识别，你仍可操作界面",
                Phase::Ready => "准备就绪 · 将生成新的材质包",
                Phase::Running => {
                    if self.closing {
                        "完成后关闭窗口"
                    } else {
                        "转换在后台进行 · 原文件保留"
                    }
                }
                Phase::Done => "转换结果已保存",
                Phase::Failed => "请检查输入或输出路径",
            }
        };
        let action = if self.overlay_mode {
            self.action(
                120,
                if self.overlay.running {
                    "正在制作…"
                } else {
                    "制作覆盖包 →"
                },
                (!self.busy() && !self.overlay.icon_pending).then_some(Message::Start),
                true,
            )
        } else if self.phase == Phase::Done {
            self.action(12, "转换另一个包", Some(Message::NewPack), true)
        } else {
            self.action(
                12,
                if self.busy() {
                    "正在转换…"
                } else if self.phase == Phase::Identifying {
                    "正在识别…"
                } else {
                    if self.queue.len() > 1 {
                        "全部转换 →"
                    } else {
                        "开始转换 →"
                    }
                },
                (self.phase == Phase::Ready
                    || self.phase == Phase::Failed
                        && self.info.as_ref().is_some_and(|value| value.ok))
                .then_some(Message::Start),
                true,
            )
        };
        let footer = container(
            row![self.label(status), space().width(Length::Fill), action]
                .align_y(Alignment::Center),
        )
        .padding([18, 28]);
        if !self.overlay_mode && !self.logs.is_empty() {
            body = body.push(self.action(
                13,
                if self.details {
                    "收起转换详情 ↑"
                } else {
                    "转换详情 ↓"
                },
                Some(Message::Details),
                false,
            ));
        }
        if !self.overlay_mode && self.details {
            let log = self.logs.join("\n");
            let mut entries = column![self.label(log)].spacing(12);
            if let Some(report) = &self.report {
                entries = entries.push(self.label(report));
            }
            entries = entries.push(self.action(14, "清空日志", Some(Message::ClearLogs), false));
            body = body.push(self.panel(entries));
        }
        let main = container(smooth_scroll::scroll(
            scrollable(container(body).padding(Padding {
                top: 24.0,
                right: 28.0,
                bottom: 28.0,
                left: 28.0,
            }))
            .id("fast-pack-content"),
            "fast-pack-content",
            self.reduced,
        ))
        .width(Length::Fill)
        .height(Length::Fill);
        let base: Element<'_, Message> = container(column![
            header,
            widget::rule::horizontal(1),
            main,
            widget::rule::horizontal(1),
            fixed_progress,
            footer
        ])
        .height(Length::Fill)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(c.background.into()),
            text_color: Some(c.foreground),
            ..Default::default()
        })
        .into();
        if self.glint_picker || self.glint_picker_motion.value(self.now) > 0.001 {
            let amount = self.glint_picker_motion.value(self.now);
            let overlay = container(self.glint_dialog())
                .padding(Padding {
                    top: (1.0 - amount) * 24.0,
                    ..Padding::ZERO
                })
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.55 * amount).into()),
                    ..Default::default()
                });
            widget::stack![base, widget::opaque(overlay)].into()
        } else if self.learn_more {
            let c = self.colors();
            let names = ["ColdEternity Team", "Long_Cold", "Wind_"];
            let developers = row(names.into_iter().enumerate().map(|(index, name)| {
                container(
                    column![
                        widget::image(developer_handle(index))
                            .width(70)
                            .height(70)
                            .content_fit(iced::ContentFit::Contain),
                        self.title(name, 14)
                    ]
                    .spacing(10)
                    .align_x(Alignment::Center),
                )
                .width(Length::FillPortion(1))
                .into()
            }))
            .spacing(16);
            let dialog=container(self.panel(column![
                row![self.title("了解 Fast Pack",24),space().width(Length::Fill),self.action(19,"关闭",Some(Message::CloseLearnMore),false)].align_y(Alignment::Center),
                self.label(format!("版本 {} · 免费的 Minecraft Java 材质包转换工具",env!("CARGO_PKG_VERSION"))),
                self.title("开发者",14),developers,
                widget::rule::horizontal(1),
                self.title("开发交流群",14),
                text("QQ：663420633").size(17).color(c.foreground),
                text("进群答案 / 密码：快手/B站/抖音").size(15).color(c.foreground),
                self.title("源码地址",14),
                text("https://github.com/LongCold-ColdEternity-Team/Fast-Pack").size(13).color(c.accent),
                self.action(20,"打开源码仓库",Some(Message::OpenSource),true),
                self.label("GNU General Public License v3.0"),
                self.label("允许使用、修改、分发与商业使用；分发程序须遵守 GPL v3，保留许可声明并提供完整对应源码。")
            ].spacing(16))).width(680);
            let overlay = container(dialog)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .style(|_| container::Style {
                    background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.55).into()),
                    ..Default::default()
                });
            widget::stack![base, widget::opaque(overlay)].into()
        } else if self.confirm_overwrite {
            let dialog = container(
                self.panel(
                    column![
                        self.title("替换已有输出？", 20),
                        self.label("已有结果将被本次转换替换，源材质包不会更改。"),
                        self.label(format!(
                            "将替换 {} 个已有结果",
                            self.queue
                                .iter()
                                .filter(|entry| entry.phase == Phase::Ready
                                    && entry.output.as_ref().is_some_and(|path| path.exists()))
                                .count()
                        )),
                        row![
                            self.action(31, "返回", Some(Message::CancelOverwrite), false),
                            self.action(32, "覆盖并转换", Some(Message::ConfirmOverwrite), true)
                        ]
                        .spacing(12)
                    ]
                    .spacing(20),
                ),
            )
            .width(480);
            let overlay = container(dialog)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .style(|_| container::Style {
                    background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.55).into()),
                    ..Default::default()
                });
            widget::stack![base, widget::opaque(overlay)].into()
        } else {
            widget::stack![base].into()
        }
    }
}

struct CrosshairCanvas<'a> {
    canvas: &'a fast_pack::crosshair::Canvas,
    colors: Colors,
    disabled: bool,
}

#[derive(Default)]
struct CrosshairCanvasState {
    stroke: Option<bool>,
    last: Option<(u8, u8)>,
}

impl widget::canvas::Program<Message> for CrosshairCanvas<'_> {
    type State = CrosshairCanvasState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &widget::canvas::Event,
        bounds: iced::Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> Option<widget::canvas::Action<Message>> {
        use iced::mouse::{Button, Event};
        if self.disabled {
            return None;
        }
        let begin = match event {
            widget::canvas::Event::Mouse(Event::ButtonPressed(
                button @ (Button::Left | Button::Right),
            )) => {
                cursor.position_in(bounds)?;
                state.stroke = Some(*button == Button::Right);
                state.last = None;
                true
            }
            widget::canvas::Event::Mouse(Event::ButtonReleased(Button::Left | Button::Right)) => {
                if state.stroke.take().is_some() {
                    state.last = None;
                    return Some(
                        widget::canvas::Action::publish(Message::CrosshairStrokeEnd).and_capture(),
                    );
                }
                return None;
            }
            widget::canvas::Event::Mouse(Event::CursorMoved { .. }) if state.stroke.is_some() => {
                false
            }
            _ => return None,
        };
        let point = cursor.position_in(bounds)?;
        let size = self.canvas.size;
        let x = ((point.x / bounds.width * f32::from(size)) as u8).min(size - 1);
        let y = ((point.y / bounds.height * f32::from(size)) as u8).min(size - 1);
        let to = (x, y);
        let from = state.last.unwrap_or(to);
        if !begin && state.last == Some(to) {
            return Some(widget::canvas::Action::capture());
        }
        state.last = Some(to);
        Some(
            widget::canvas::Action::publish(Message::CrosshairStroke(
                from,
                to,
                state.stroke?,
                begin,
            ))
            .and_capture(),
        )
    }

    fn mouse_interaction(
        &self,
        _: &Self::State,
        bounds: iced::Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> iced::mouse::Interaction {
        if !self.disabled && cursor.is_over(bounds) {
            iced::mouse::Interaction::Crosshair
        } else {
            iced::mouse::Interaction::default()
        }
    }

    fn draw(
        &self,
        _: &Self::State,
        renderer: &iced::Renderer,
        _: &Theme,
        bounds: iced::Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> Vec<widget::canvas::Geometry> {
        use widget::canvas::{Frame, Path, Stroke};
        let mut frame = Frame::new(renderer, bounds.size());
        let size = self.canvas.size;
        let cell = bounds.width / f32::from(size);
        for y in 0..size {
            for x in 0..size {
                let point = iced::Point::new(f32::from(x) * cell, f32::from(y) * cell);
                let background = if (x + y) % 2 == 0 {
                    self.colors.field
                } else {
                    mix(self.colors.field, self.colors.foreground, 0.08)
                };
                frame.fill_rectangle(point, Size::new(cell, cell), background);
                let [r, g, b, a] = self.canvas.color(x, y);
                if a > 0 {
                    frame.fill_rectangle(
                        point,
                        Size::new(cell, cell),
                        Color::from_rgba8(r, g, b, f32::from(a) / 255.0),
                    );
                }
            }
        }
        for index in 0..=size {
            let axis = f32::from(index) * cell;
            let stroke = Stroke::default()
                .with_color(mix(self.colors.line, self.colors.foreground, 0.16))
                .with_width(0.65);
            frame.stroke(
                &Path::line(
                    iced::Point::new(axis, 0.0),
                    iced::Point::new(axis, bounds.height),
                ),
                stroke,
            );
            frame.stroke(
                &Path::line(
                    iced::Point::new(0.0, axis),
                    iced::Point::new(bounds.width, axis),
                ),
                stroke,
            );
        }
        if !self.disabled
            && let Some(point) = cursor.position_in(bounds)
        {
            let x = (point.x / cell).floor() * cell;
            let y = (point.y / cell).floor() * cell;
            frame.stroke(
                &Path::rectangle(iced::Point::new(x, y), Size::new(cell, cell)),
                Stroke::default()
                    .with_color(self.colors.accent)
                    .with_width(1.5),
            );
        }
        vec![frame.into_geometry()]
    }
}

struct GlintColorCard {
    hsv: [f32; 3],
    colors: Colors,
}

#[derive(Default)]
struct GlintColorCardState {
    dragging_hue: Option<bool>,
}

impl GlintColorCard {
    fn regions(size: Size) -> (iced::Rectangle, iced::Rectangle) {
        let side = (size.height - 12.0).min(size.width - 52.0).max(1.0);
        (
            iced::Rectangle {
                x: 6.0,
                y: 6.0,
                width: side,
                height: side,
            },
            iced::Rectangle {
                x: side + 20.0,
                y: 6.0,
                width: 24.0,
                height: side,
            },
        )
    }
}

impl widget::canvas::Program<Message> for GlintColorCard {
    type State = GlintColorCardState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &widget::canvas::Event,
        bounds: iced::Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> Option<widget::canvas::Action<Message>> {
        use iced::mouse::{Button, Event};
        let (square, hue_bar) = Self::regions(bounds.size());
        match event {
            widget::canvas::Event::Mouse(Event::ButtonPressed(Button::Left)) => {
                let point = cursor.position_in(bounds)?;
                state.dragging_hue = if square.contains(point) {
                    Some(false)
                } else if hue_bar.contains(point) {
                    Some(true)
                } else {
                    return None;
                };
            }
            widget::canvas::Event::Mouse(Event::ButtonReleased(Button::Left)) => {
                return state
                    .dragging_hue
                    .take()
                    .map(|_| widget::canvas::Action::capture());
            }
            widget::canvas::Event::Mouse(Event::CursorMoved { .. })
                if state.dragging_hue.is_some() => {}
            _ => return None,
        }
        let point = cursor.position()? - Vector::new(bounds.x, bounds.y);
        let message = if state.dragging_hue? {
            Message::GlintHue(1.0 - ((point.y - hue_bar.y) / hue_bar.height).clamp(0.0, 1.0))
        } else {
            Message::GlintSaturationValue(
                ((point.x - square.x) / square.width).clamp(0.0, 1.0),
                1.0 - ((point.y - square.y) / square.height).clamp(0.0, 1.0),
            )
        };
        Some(widget::canvas::Action::publish(message).and_capture())
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: iced::Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> iced::mouse::Interaction {
        let (square, hue_bar) = Self::regions(bounds.size());
        let point = cursor.position_in(bounds);
        if state.dragging_hue.is_some() || point.is_some_and(|point| square.contains(point)) {
            iced::mouse::Interaction::Crosshair
        } else if point.is_some_and(|point| hue_bar.contains(point)) {
            iced::mouse::Interaction::Pointer
        } else {
            iced::mouse::Interaction::default()
        }
    }

    fn draw(
        &self,
        _: &Self::State,
        renderer: &iced::Renderer,
        _: &Theme,
        bounds: iced::Rectangle,
        _: iced::mouse::Cursor,
    ) -> Vec<widget::canvas::Geometry> {
        use widget::canvas::{Frame, Path, Stroke, gradient};
        let mut frame = Frame::new(renderer, bounds.size());
        let (square, hue_bar) = Self::regions(bounds.size());
        let [red, green, blue] = hsv_rgb([self.hsv[0], 1.0, 1.0]);
        let pure = Color::from_rgb8(red, green, blue);
        let square_path = Path::rounded_rectangle(square.position(), square.size(), 3.0.into());
        frame.fill(
            &square_path,
            gradient::Linear::new(
                square.position(),
                iced::Point::new(square.x + square.width, square.y),
            )
            .add_stop(0.0, Color::WHITE)
            .add_stop(1.0, pure),
        );
        frame.fill(
            &square_path,
            gradient::Linear::new(
                square.position(),
                iced::Point::new(square.x, square.y + square.height),
            )
            .add_stop(0.0, Color::TRANSPARENT)
            .add_stop(1.0, Color::BLACK),
        );
        frame.stroke(
            &square_path,
            Stroke::default()
                .with_color(self.colors.line)
                .with_width(1.0),
        );
        let bar_path = Path::rounded_rectangle(hue_bar.position(), hue_bar.size(), 3.0.into());
        let mut spectrum = gradient::Linear::new(
            hue_bar.position(),
            iced::Point::new(hue_bar.x, hue_bar.y + hue_bar.height),
        );
        for index in 0..=6 {
            let amount = index as f32 / 6.0;
            let [red, green, blue] = hsv_rgb([1.0 - amount, 1.0, 1.0]);
            spectrum = spectrum.add_stop(amount, Color::from_rgb8(red, green, blue));
        }
        frame.fill(&bar_path, spectrum);
        frame.stroke(
            &bar_path,
            Stroke::default()
                .with_color(self.colors.line)
                .with_width(1.0),
        );
        let marker = iced::Point::new(
            square.x + self.hsv[1] * square.width,
            square.y + (1.0 - self.hsv[2]) * square.height,
        );
        let marker_path = Path::circle(marker, 5.5);
        let [red, green, blue] = hsv_rgb(self.hsv);
        frame.fill(&marker_path, Color::from_rgb8(red, green, blue));
        frame.stroke(
            &marker_path,
            Stroke::default()
                .with_color(Color::from_rgba(0.0, 0.0, 0.0, 0.7))
                .with_width(4.0),
        );
        frame.stroke(
            &marker_path,
            Stroke::default().with_color(Color::WHITE).with_width(2.0),
        );
        let hue_marker = Path::rectangle(
            iced::Point::new(
                hue_bar.x - 3.0,
                hue_bar.y + (1.0 - self.hsv[0]) * hue_bar.height - 2.0,
            ),
            Size::new(hue_bar.width + 6.0, 4.0),
        );
        frame.stroke(
            &hue_marker,
            Stroke::default()
                .with_color(Color::from_rgba(0.0, 0.0, 0.0, 0.7))
                .with_width(4.0),
        );
        frame.stroke(
            &hue_marker,
            Stroke::default().with_color(Color::WHITE).with_width(2.0),
        );
        vec![frame.into_geometry()]
    }
}

struct GlintPreview {
    color: Color,
    colors: Colors,
    elapsed: f32,
}

impl widget::canvas::Program<Message> for GlintPreview {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &iced::Renderer,
        _: &Theme,
        bounds: iced::Rectangle,
        _: iced::mouse::Cursor,
    ) -> Vec<widget::canvas::Geometry> {
        use widget::canvas::{Frame, Path};
        let mut frame = Frame::new(renderer, bounds.size());
        frame.fill(
            &Path::rounded_rectangle(iced::Point::ORIGIN, bounds.size(), 12.0.into()),
            self.colors.field,
        );
        let unit = bounds.width.min(bounds.height) / 24.0;
        for y in 0_i32..20 {
            for x in 0_i32..20 {
                let diagonal = x + y;
                let blade =
                    (8..=18).contains(&x) && (1..=11).contains(&y) && (18..=20).contains(&diagonal);
                let guard =
                    (5..=11).contains(&x) && (9..=15).contains(&y) && (-5..=-3).contains(&(x - y));
                let handle =
                    (3..=7).contains(&x) && (13..=17).contains(&y) && (19..=20).contains(&diagonal);
                let pommel = (2..=4).contains(&x) && (16..=18).contains(&y);
                if !(blade || guard || handle || pommel) {
                    continue;
                }
                let phase = x as f32 * 0.48 + y as f32 * 0.24 - self.elapsed * 2.8;
                let shine = ((phase.sin() + 1.0) * 0.5).powi(3);
                let base = if handle || pommel {
                    rgb(0x667785)
                } else {
                    rgb(0xa4b9c7)
                };
                let color = mix(base, self.color, 0.35 + shine * 0.65);
                frame.fill_rectangle(
                    iced::Point::new((x + 2) as f32 * unit, (y + 2) as f32 * unit),
                    Size::new(unit + 0.2, unit + 0.2),
                    color,
                );
            }
        }
        vec![frame.into_geometry()]
    }
}

struct AutoSaveSwitch {
    amount: f32,
    hover: f32,
    colors: Colors,
    disabled: bool,
}

impl widget::canvas::Program<Message> for AutoSaveSwitch {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &iced::Renderer,
        _: &Theme,
        bounds: iced::Rectangle,
        _: iced::mouse::Cursor,
    ) -> Vec<widget::canvas::Geometry> {
        use widget::canvas::{Frame, Path, Stroke};
        let mut frame = Frame::new(renderer, bounds.size());
        let track = Path::rounded_rectangle(
            iced::Point::new(4.0, 4.0),
            Size::new(50.0, 26.0),
            13.0.into(),
        );
        let track_color = mix(self.colors.line, self.colors.accent, self.amount);
        frame.fill(
            &track,
            if self.disabled {
                mix(track_color, self.colors.background, 0.5)
            } else {
                track_color
            },
        );
        frame.stroke(
            &track,
            Stroke::default()
                .with_color(mix(self.colors.line, self.colors.accent, self.hover * 0.5))
                .with_width(1.0),
        );
        let center = iced::Point::new(17.0 + self.amount * 24.0, 17.0);
        let radius = 9.5 + if self.disabled { 0.0 } else { self.hover * 0.5 };
        frame.fill(
            &Path::circle(iced::Point::new(center.x, center.y + 1.0), radius + 0.5),
            Color::from_rgba(0.0, 0.0, 0.0, 0.13),
        );
        frame.fill(
            &Path::circle(center, radius),
            mix(
                self.colors.foreground,
                Color::WHITE,
                if self.disabled { 0.4 } else { 0.94 },
            ),
        );
        vec![frame.into_geometry()]
    }
}

#[derive(Clone, Copy)]
enum ArtworkKind {
    Package,
    Check,
    Error,
}
struct Artwork {
    color: Color,
    kind: ArtworkKind,
}

impl widget::canvas::Program<Message> for Artwork {
    type State = ();
    fn draw(
        &self,
        _: &(),
        renderer: &iced::Renderer,
        _: &Theme,
        bounds: iced::Rectangle,
        _: iced::mouse::Cursor,
    ) -> Vec<widget::canvas::Geometry> {
        use widget::canvas::{Frame, Path, Stroke};
        let mut frame = Frame::new(renderer, bounds.size());
        let w = bounds.width;
        let h = bounds.height;
        let path = Path::new(|builder| match self.kind {
            ArtworkKind::Package => {
                builder.move_to(iced::Point::new(w * 0.5, h * 0.08));
                builder.line_to(iced::Point::new(w * 0.91, h * 0.29));
                builder.line_to(iced::Point::new(w * 0.91, h * 0.71));
                builder.line_to(iced::Point::new(w * 0.5, h * 0.94));
                builder.line_to(iced::Point::new(w * 0.09, h * 0.71));
                builder.line_to(iced::Point::new(w * 0.09, h * 0.29));
                builder.close();
                builder.move_to(iced::Point::new(w * 0.09, h * 0.29));
                builder.line_to(iced::Point::new(w * 0.5, h * 0.51));
                builder.line_to(iced::Point::new(w * 0.91, h * 0.29));
                builder.move_to(iced::Point::new(w * 0.5, h * 0.51));
                builder.line_to(iced::Point::new(w * 0.5, h * 0.94));
                builder.move_to(iced::Point::new(w * 0.3, h * 0.18));
                builder.line_to(iced::Point::new(w * 0.7, h * 0.41));
            }
            ArtworkKind::Check => {
                builder.move_to(iced::Point::new(w * 0.16, h * 0.52));
                builder.line_to(iced::Point::new(w * 0.42, h * 0.77));
                builder.line_to(iced::Point::new(w * 0.88, h * 0.21));
            }
            ArtworkKind::Error => {
                builder.move_to(iced::Point::new(w * 0.5, h * 0.15));
                builder.line_to(iced::Point::new(w * 0.5, h * 0.65));
                builder.move_to(iced::Point::new(w * 0.5, h * 0.78));
                builder.line_to(iced::Point::new(w * 0.5, h * 0.85));
            }
        });
        frame.stroke(
            &path,
            Stroke::default().with_color(self.color).with_width(2.5),
        );
        vec![frame.into_geometry()]
    }
}

fn shorten(value: &str, maximum: usize) -> String {
    let chars: Vec<_> = value.chars().collect();
    if chars.len() <= maximum {
        return value.into();
    }
    format!(
        "{}…{}",
        chars[..maximum * 2 / 3].iter().collect::<String>(),
        chars[chars.len() - maximum / 3..]
            .iter()
            .collect::<String>()
    )
}

fn system_reduced_motion() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
    };
    let mut enabled = 1i32;
    unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            (&mut enabled as *mut i32).cast(),
            0,
        );
    }
    enabled == 0
}

fn save_screenshot(path: &Path, screenshot: &window::Screenshot) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(file, screenshot.size.width, screenshot.size.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&screenshot.rgba)?;
    Ok(())
}

fn app_icon() -> Option<window::Icon> {
    let bitmap = image::load_from_memory(include_bytes!("../assets/app.png"))
        .ok()?
        .to_rgba8();
    let bitmap = image::imageops::resize(&bitmap, 64, 64, image::imageops::FilterType::Lanczos3);
    window::icon::from_rgba(bitmap.into_raw(), 64, 64).ok()
}

fn brand_handle() -> widget::image::Handle {
    static BRAND: std::sync::LazyLock<widget::image::Handle> = std::sync::LazyLock::new(|| {
        let bitmap = image::load_from_memory(include_bytes!("../assets/app.png"))
            .expect("内置 logo 无效")
            .to_rgba8();
        widget::image::Handle::from_rgba(bitmap.width(), bitmap.height(), bitmap.into_raw())
    });
    BRAND.clone()
}

fn developer_handle(index: usize) -> widget::image::Handle {
    static DEVELOPERS: std::sync::LazyLock<[widget::image::Handle; 3]> =
        std::sync::LazyLock::new(|| {
            [
                include_bytes!("../assets/developers/coldeternity-team.png").as_slice(),
                include_bytes!("../assets/developers/long-cold.png").as_slice(),
                include_bytes!("../assets/developers/wind.png").as_slice(),
            ]
            .map(|bytes| {
                let bitmap = image::load_from_memory(bytes)
                    .expect("开发者头像无效")
                    .to_rgba8();
                widget::image::Handle::from_rgba(bitmap.width(), bitmap.height(), bitmap.into_raw())
            })
        });
    DEVELOPERS[index].clone()
}

pub fn run() -> Result<()> {
    let smoke = std::env::args().any(|value| value == "--smoke-test");
    let size = std::env::var("FAST_PACK_CAPTURE_SIZE")
        .ok()
        .and_then(|value| {
            value
                .split_once('x')
                .map(|(w, h)| (w.parse::<f32>().ok(), h.parse::<f32>().ok()))
        })
        .and_then(|(w, h)| Some(Size::new(w?, h?)))
        .unwrap_or_else(|| Size::new(1040.0, 780.0));
    let settings = window::Settings {
        size,
        min_size: Some(Size::new(800.0, 680.0)),
        position: window::Position::Centered,
        visible: !smoke,
        icon: app_icon(),
        exit_on_close_request: false,
        ..Default::default()
    };
    iced::application(App::boot, App::update, App::view)
        .title("Fast Pack")
        .theme(App::theme)
        .subscription(App::subscription)
        .window(settings)
        .default_font(Font::with_name(crate::windows_compat::font_family()))
        .antialiasing(true)
        .run()
        .context("无法启动 Fast Pack 界面")
}

pub fn show_error(value: &str) {
    rfd::MessageDialog::new()
        .set_title("Fast Pack")
        .set_description(value)
        .set_level(rfd::MessageLevel::Error)
        .show();
}

#[cfg(test)]
#[path = "gui_tests.rs"]
mod tests;
