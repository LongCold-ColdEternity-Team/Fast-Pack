//! Standalone overlay packs: only explicitly requested features are generated.
use crate::compat::assets::AssetReport;
use crate::packfs::{self as fs, WalkDir};
use anyhow::{Context, Result, ensure};
use image::{ImageFormat, Rgba, RgbaImage, imageops};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use zip::ZipWriter;

pub const DESCRIPTION_LIMIT: usize = 2048;
#[path = "overlay_text.rs"]
mod text;
pub use text::{Format, Run, TextStyle, formatted_filename};
pub const LEGACY_COLORS: [[u8; 3]; 16] = [
    [0, 0, 0],
    [0, 0, 170],
    [0, 170, 0],
    [0, 170, 170],
    [170, 0, 0],
    [170, 0, 170],
    [255, 170, 0],
    [170, 170, 170],
    [85, 85, 85],
    [85, 85, 255],
    [85, 255, 85],
    [85, 255, 255],
    [255, 85, 85],
    [255, 85, 255],
    [255, 255, 85],
    [255, 255, 255],
];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Description {
    pub text: String,
    /// One optional color per Unicode scalar, independent of UTF-8 byte offsets.
    pub colors: Vec<Option<[u8; 3]>>,
    pub styles: Vec<TextStyle>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Draft {
    pub name: String,
    pub description: Description,
    pub icon: Option<Vec<u8>>,
    pub ink: [u8; 3],
    pub as_zip: bool,
    pub name_colors: Vec<Option<[u8; 3]>>,
    pub name_styles: Vec<TextStyle>,
    pub name_ink: [u8; 3],
    pub name_typing_style: TextStyle,
    pub typing_style: TextStyle,
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            name: "我的覆盖包".into(),
            description: Description::plain("我的自定义覆盖包"),
            icon: None,
            ink: [170; 3],
            as_zip: true,
            name_colors: Vec::new(),
            name_styles: Vec::new(),
            name_ink: [255; 3],
            name_typing_style: TextStyle::default(),
            typing_style: TextStyle::default(),
        }
    }
}

pub fn supports_hex(version: &str) -> bool {
    !version.starts_with("1.")
        || version
            .split('.')
            .nth(1)
            .and_then(|part| part.parse::<u32>().ok())
            .is_some_and(|minor| minor >= 16)
}

fn legacy_index(color: [u8; 3]) -> usize {
    LEGACY_COLORS
        .iter()
        .enumerate()
        .min_by_key(|(_, candidate)| {
            candidate
                .iter()
                .zip(color)
                .map(|(a, b)| (i32::from(*a) - i32::from(b)).pow(2))
                .sum::<i32>()
        })
        .unwrap()
        .0
}

pub fn display_color(color: [u8; 3], version: &str) -> [u8; 3] {
    if supports_hex(version) {
        color
    } else {
        LEGACY_COLORS[legacy_index(color)]
    }
}

impl Description {
    pub fn plain(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            colors: vec![None; text.chars().count()],
            styles: vec![TextStyle::default(); text.chars().count()],
            text,
        }
    }

    pub fn paint(&mut self, range: Range<usize>, color: Option<[u8; 3]>) {
        let length = self.text.chars().count();
        self.colors.resize(length, None);
        for current in &mut self.colors
            [range.start.min(length)..range.end.min(length).max(range.start.min(length))]
        {
            *current = color;
        }
    }

    /// Reconcile an edit at its actual cursor anchor (including repeated text).
    pub fn edit(&mut self, text: String, anchor: usize, ink: [u8; 3]) -> Result<()> {
        self.edit_styled(text, anchor, ink, TextStyle::default())
    }

    pub fn runs(&self, version: &str) -> Vec<(String, Option<[u8; 3]>)> {
        self.formatted_runs(version)
            .into_iter()
            .map(|run| (run.text, run.color))
            .collect()
    }

    pub fn component(&self, version: &str) -> Value {
        self.styled_component(version)
    }
}

pub fn prepare_icon(bytes: &[u8]) -> Result<Vec<u8>> {
    ensure!(bytes.len() <= 16 * 1024 * 1024, "图标文件不能超过 16 MB");
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(80 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .context("请选择 PNG 或 JPG 图片")?
        .to_rgba8();
    let side = image.width().max(image.height()).max(1);
    let image = imageops::resize(
        &image,
        (image.width() * 128 / side).max(1),
        (image.height() * 128 / side).max(1),
        imageops::FilterType::Lanczos3,
    );
    let mut square = RgbaImage::from_pixel(128, 128, Rgba([0; 4]));
    imageops::replace(
        &mut square,
        &image,
        i64::from((128 - image.width()) / 2),
        i64::from((128 - image.height()) / 2),
    );
    let mut output = Cursor::new(Vec::new());
    square.write_to(&mut output, ImageFormat::Png)?;
    Ok(output.into_inner())
}

#[derive(Clone, Debug)]
pub struct Options {
    pub version: String,
    pub description: Description,
    pub icon: Option<Vec<u8>>,
    pub as_zip: bool,
    pub glint_color: Option<[u8; 3]>,
    pub fishing_line_color: Option<[u8; 3]>,
    pub block_outline_color: Option<[u8; 3]>,
    pub line_widths: crate::compat::line_colors::Widths,
    pub crosshair: Option<crate::crosshair::Crosshair>,
    pub show_attack_indicator: bool,
    pub custom_items: Vec<crate::compat::custom_items::Rule>,
    pub rainbow: crate::compat::rainbow::Settings,
    pub builtin_overlays: crate::builtin_overlays::Settings,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            version: "1.20.1".into(),
            description: Description::default(),
            icon: None,
            as_zip: true,
            glint_color: None,
            fishing_line_color: None,
            block_outline_color: None,
            line_widths: Default::default(),
            crosshair: None,
            show_attack_indicator: true,
            custom_items: Vec::new(),
            rainbow: Default::default(),
            builtin_overlays: Default::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Report {
    pub output: PathBuf,
    pub files: usize,
    pub assets: AssetReport,
}

pub fn filename(name: &str, version: &str, as_zip: bool) -> String {
    formatted_filename(&Description::plain(name), version, as_zip)
}

pub fn build(
    destination: &Path,
    options: &Options,
    progress: impl FnMut(&str, u8),
) -> Result<Report> {
    crate::tasks::run("制作覆盖包", || {
        build_inner(destination, options, progress)
    })
}

fn build_inner(
    destination: &Path,
    options: &Options,
    mut progress: impl FnMut(&str, u8),
) -> Result<Report> {
    let format = crate::pack_format_of(&options.version)?;
    ensure!(!destination.exists(), "输出已存在，请更换文件名后重试");
    ensure!(
        options.description.text.chars().count() <= DESCRIPTION_LIMIT,
        "简介过长"
    );
    ensure!(
        options
            .custom_items
            .iter()
            .all(|rule| rule.version == options.version),
        "物品设置与覆盖包版本不一致"
    );
    for rule in &options.custom_items {
        crate::compat::custom_items::validate(rule)?;
    }
    let work = tempfile::tempdir()?;
    let root = work.path().join("overlay");
    let _memory = fs::Mount::new(work.path());
    fs::create_dir_all(&root)?;
    let mut assets = AssetReport::default();
    progress("制作内置覆盖内容", 8);
    crate::builtin_overlays::apply(
        &root,
        &options.version,
        &options.builtin_overlays,
        &mut assets,
    )?;
    progress("制作附魔光效", 12);
    if options.rainbow.glint {
        crate::compat::glint::rainbow(&root, format, format, &mut assets)?;
    } else if options.glint_color.is_some() {
        crate::compat::glint::migrate(&root, format, format, options.glint_color, &mut assets)?;
    }
    progress("制作线条外观", 28);
    let builtin_outline = options
        .builtin_overlays
        .enabled(crate::builtin_overlays::Feature::RainbowOutline)
        && format >= 7;
    crate::compat::line_colors::apply_selected(
        &root,
        &options.version,
        options.fishing_line_color,
        options
            .block_outline_color
            .or(builtin_outline.then_some([0; 3])),
        options.rainbow.fishing_line,
        options.rainbow.block_outline || (builtin_outline && options.block_outline_color.is_none()),
        options.line_widths,
        Some(crate::compat::line_colors::ColorSelection {
            fishing: options.fishing_line_color.is_some() || options.rainbow.fishing_line,
            outline: options.block_outline_color.is_some()
                || options.rainbow.block_outline
                || builtin_outline,
        }),
        &mut assets,
    )?;
    progress("制作物品设置", 45);
    if !options.custom_items.is_empty() {
        crate::compat::custom_items::apply(
            &root,
            &options.version,
            &options.custom_items,
            &mut assets,
        )?;
    }
    progress("制作准星", 68);
    if let Some(crosshair) = &options.crosshair {
        crate::crosshair::apply(&root, format, crosshair, &mut assets)?;
        if options.rainbow.crosshair {
            crate::compat::rainbow::crosshair(&root, format, &mut assets)?;
        }
    }
    // Leaving the default on must not replace the pack underneath with vanilla HUD sprites.
    if !options.show_attack_indicator {
        crate::compat::attack_indicator::apply(&root, format, false, &mut assets)?;
    }
    progress("写入图标与简介", 82);
    let icon = prepare_icon(
        options
            .icon
            .as_deref()
            .unwrap_or(include_bytes!("../assets/app.png")),
    )?;
    fs::write(root.join("pack.png"), icon)?;
    let mut pack = json!({"pack_format": format, "description": options.description.component(&options.version)});
    if format >= crate::NEW_META_MIN_FORMAT {
        let components = crate::tables::format_components(Some(&options.version), format);
        pack["min_format"] = json!(components);
        pack["max_format"] = json!(components);
    }
    fs::write(
        root.join("pack.mcmeta"),
        serde_json::to_vec_pretty(&json!({"pack": pack}))?,
    )?;
    let mut files = Vec::new();
    for entry in WalkDir::new(&root) {
        let entry = entry?;
        if entry.file_type().is_file() {
            files.push(entry.into_path());
        }
    }
    files.sort();
    progress("保存覆盖包", 92);
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    if options.as_zip {
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        {
            let mut zip = ZipWriter::new(temporary.as_file_mut());
            for file in &files {
                let name = file
                    .strip_prefix(&root)?
                    .to_string_lossy()
                    .replace('\\', "/");
                zip.start_file(name, crate::converter::archive_options(file))?;
                zip.write_all(&fs::read(file)?)?;
            }
            zip.finish()?.flush()?;
        }
        temporary
            .persist_noclobber(destination)
            .map_err(|error| error.error)?;
    } else {
        let staging = tempfile::tempdir_in(parent)?;
        for file in &files {
            let path = staging.path().join(file.strip_prefix(&root)?);
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, fs::read(file)?)?;
        }
        ensure!(!destination.exists(), "输出已存在，请更换文件名后重试");
        std::fs::rename(staging.path(), destination)?;
    }
    progress("覆盖包制作完成", 100);
    Ok(Report {
        output: destination.into(),
        files: files.len(),
        assets,
    })
}

#[cfg(test)]
#[path = "overlay_tests.rs"]
mod tests;
