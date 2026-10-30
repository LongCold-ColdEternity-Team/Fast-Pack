use crate::packfs::PathExt;
use crate::packfs::WalkDir;
use crate::packfs::{self as fs, File};
use anyhow::{Context, Result, ensure};
use image::{Rgba, RgbaImage, imageops};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use zip::ZipArchive;

const SPRITE: &str = "assets/minecraft/textures/gui/sprites/hud/crosshair.png";
const ICONS: &str = "assets/minecraft/textures/gui/icons.png";

mod builtin {
    include!("../assets/crosshair/builtins/catalog.rs");
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Canvas {
    pub size: u8,
    pub pixels: Vec<[u8; 4]>,
}

impl Default for Canvas {
    fn default() -> Self {
        Self {
            size: 15,
            pixels: vec![[0; 4]; 225],
        }
    }
}

impl Canvas {
    pub fn color(&self, x: u8, y: u8) -> [u8; 4] {
        if x >= self.size || y >= self.size {
            return [0; 4];
        }
        self.pixels
            .get(usize::from(y) * usize::from(self.size) + usize::from(x))
            .copied()
            .unwrap_or([0; 4])
    }

    pub fn paint(&mut self, x: u8, y: u8, color: [u8; 4]) {
        if x < self.size && y < self.size {
            let length = usize::from(self.size).pow(2);
            self.pixels.resize(length, [0; 4]);
            self.pixels[usize::from(y) * usize::from(self.size) + usize::from(x)] = color;
        }
    }

    pub fn line(&mut self, from: (u8, u8), to: (u8, u8), color: [u8; 4]) {
        let (mut x, mut y) = (i32::from(from.0), i32::from(from.1));
        let (tx, ty) = (i32::from(to.0), i32::from(to.1));
        let dx = (tx - x).abs();
        let dy = -(ty - y).abs();
        let sx = if x < tx { 1 } else { -1 };
        let sy = if y < ty { 1 } else { -1 };
        let mut error = dx + dy;
        loop {
            if x >= 0 && y >= 0 {
                self.paint(x as u8, y as u8, color);
            }
            if x == tx && y == ty {
                break;
            }
            let next = error * 2;
            if next >= dy {
                error += dy;
                x += sx;
            }
            if next <= dx {
                error += dx;
                y += sy;
            }
        }
    }

    pub fn render(&self, name: &str) -> Crosshair {
        let size = if self.size == 30 { 30 } else { 15 };
        let mut image = RgbaImage::new(size, size);
        for y in 0..size {
            for x in 0..size {
                image.put_pixel(x, y, Rgba(self.color(x as u8, y as u8)));
            }
        }
        Crosshair {
            name: if name.trim().is_empty() {
                "自绘准星".into()
            } else {
                name.trim().into()
            },
            image,
        }
    }

    pub fn from_image(image: &RgbaImage, size: u8) -> Self {
        let size = if size == 30 { 30 } else { 15 };
        let image = imageops::resize(
            image,
            u32::from(size),
            u32::from(size),
            imageops::FilterType::Nearest,
        );
        Self {
            size,
            pixels: image.pixels().map(|pixel| pixel.0).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pixels.iter().all(|pixel| pixel[3] == 0)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum Style {
    #[default]
    Cross,
    Dot,
    Ring,
    Diamond,
}

impl Style {
    pub const ALL: [Self; 4] = [Self::Cross, Self::Dot, Self::Ring, Self::Diamond];
}

impl fmt::Display for Style {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Cross => "十字",
            Self::Dot => "圆点",
            Self::Ring => "圆环",
            Self::Diamond => "菱形",
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Generator {
    pub style: Style,
    pub length: u8,
    pub thickness: u8,
    pub gap: u8,
    pub outline: bool,
    pub color: [u8; 3],
}

impl Default for Generator {
    fn default() -> Self {
        Self {
            style: Style::Cross,
            length: 4,
            thickness: 1,
            gap: 1,
            outline: true,
            color: [255; 3],
        }
    }
}

#[derive(Clone, Debug)]
pub struct Crosshair {
    pub name: String,
    pub image: RgbaImage,
}

impl Generator {
    pub fn render(&self) -> Crosshair {
        let radius = i32::from(self.length.clamp(1, 7));
        let width = i32::from(self.thickness.clamp(1, 3));
        let gap = i32::from(self.gap.min(4));
        let mut image = RgbaImage::new(15, 15);
        for y in 0..15 {
            for x in 0..15 {
                let dx = x as i32 - 7;
                let dy = y as i32 - 7;
                let inside = match self.style {
                    Style::Cross => {
                        let thin = |v: i32| v >= -(width / 2) && v < (width + 1) / 2;
                        thin(dx) && dy.abs() >= gap && dy.abs() < gap + radius
                            || thin(dy) && dx.abs() >= gap && dx.abs() < gap + radius
                    }
                    Style::Dot => dx * dx + dy * dy <= (radius - 1).pow(2),
                    Style::Ring => {
                        let distance = ((dx * dx + dy * dy) as f32).sqrt();
                        distance <= radius as f32 && distance >= (radius - width).max(0) as f32
                    }
                    Style::Diamond => {
                        let distance = dx.abs() + dy.abs();
                        distance <= radius && distance > radius - width
                    }
                };
                if inside {
                    image.put_pixel(
                        x,
                        y,
                        Rgba([self.color[0], self.color[1], self.color[2], 255]),
                    );
                }
            }
        }
        if self.outline {
            let shape = image.clone();
            for y in 0..15 {
                for x in 0..15 {
                    if shape.get_pixel(x, y)[3] > 0 {
                        continue;
                    }
                    if [-1, 0, 1].into_iter().any(|dy| {
                        [-1, 0, 1].into_iter().any(|dx| {
                            let px = x as i32 + dx;
                            let py = y as i32 + dy;
                            (0..15).contains(&px)
                                && (0..15).contains(&py)
                                && shape.get_pixel(px as u32, py as u32)[3] > 0
                        })
                    }) {
                        image.put_pixel(x, y, Rgba([0, 0, 0, 255]));
                    }
                }
            }
        }
        Crosshair {
            name: format!(
                "自动绘制 · {} {}-{}-{}",
                self.style, self.length, self.thickness, self.gap
            ),
            image,
        }
    }
}

fn decode(bytes: &[u8]) -> Result<RgbaImage> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader.decode()?.to_rgba8())
}

pub fn transparent_background(image: &RgbaImage) -> RgbaImage {
    let mut image = image.clone();
    let mut border = 0u64;
    let mut black = 0u64;
    for (x, y, pixel) in image.enumerate_pixels() {
        if x == 0 || y == 0 || x + 1 == image.width() || y + 1 == image.height() {
            border += 1;
            if pixel[3] > 0 && pixel[0].max(pixel[1]).max(pixel[2]) <= 2 {
                black += 1;
            }
        }
    }
    if border > 0 && black * 4 >= border * 3 {
        for pixel in image.pixels_mut() {
            if pixel[0].max(pixel[1]).max(pixel[2]) <= 2 {
                *pixel = Rgba([0; 4]);
            }
        }
    }
    image
}

fn normalized(image: RgbaImage, atlas: bool, remove_background: bool) -> Result<RgbaImage> {
    let image = if atlas {
        ensure!(
            image.width() >= 256
                && image.width().is_multiple_of(256)
                && image.height() >= image.width(),
            "旧版 icons.png 需要使用 256 基准方形图集"
        );
        let size = 15 * (image.width() / 256);
        imageops::crop_imm(&image, 0, 0, size, size).to_image()
    } else if image.height() > image.width() && image.height().is_multiple_of(image.width()) {
        imageops::crop_imm(&image, 0, 0, image.width(), image.width()).to_image()
    } else {
        image
    };
    ensure!(image.width() > 0 && image.height() > 0, "准星图片为空");
    let image = if remove_background {
        transparent_background(&image)
    } else {
        image
    };
    ensure!(image.pixels().any(|pixel| pixel[3] > 0), "准星区域完全透明");
    let side = image.width().max(image.height()).div_ceil(15).clamp(1, 64) * 15;
    let mut out = RgbaImage::new(side, side);
    let largest = image.width().max(image.height());
    let width = ((u64::from(image.width()) * u64::from(side)) / u64::from(largest)).max(1) as u32;
    let height = ((u64::from(image.height()) * u64::from(side)) / u64::from(largest)).max(1) as u32;
    let tile = imageops::resize(&image, width, height, imageops::FilterType::Nearest);
    imageops::replace(
        &mut out,
        &tile,
        i64::from((side - width) / 2),
        i64::from((side - height) / 2),
    );
    Ok(out)
}

fn candidate(name: &str) -> Option<(String, u8, bool)> {
    let name = name.replace('\\', "/");
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("fastpack/")
        || lower.contains("/fastpack/")
        || lower.split('/').any(|part| matches!(part, "." | ".."))
    {
        return None;
    }
    for (suffix, priority, atlas) in [
        (SPRITE, 0, false),
        (ICONS, 1, true),
        ("gui/icons.png", 2, true),
    ] {
        if lower == suffix || lower.ends_with(&format!("/{suffix}")) {
            let prefix = name[..name.len() - suffix.len()].trim_end_matches('/');
            return Some((prefix.into(), priority, atlas));
        }
    }
    if lower.ends_with(".png")
        && (lower
            .rsplit('/')
            .next()
            .is_some_and(|name| name.starts_with("crosshair"))
            || lower
                .split('/')
                .any(|part| matches!(part, "crosshair" | "crosshairs")))
    {
        return Some((name.trim_end_matches(".png").into(), 3, false));
    }
    None
}

pub fn import(source: &Path) -> Result<Vec<Crosshair>> {
    let name = source
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    if source.pack_is_file()
        && source
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
    {
        ensure!(
            fs::metadata(source)?.len() <= 32 * 1024 * 1024,
            "图片文件过大"
        );
        let atlas = source
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("icons.png"));
        return Ok(vec![Crosshair {
            name,
            image: normalized(decode(&fs::read(source)?)?, atlas, true)?,
        }]);
    }
    let unpacked = if !source.pack_is_dir() && crate::archive::is_rar(source)? {
        Some(crate::converter::load_pack(source)?)
    } else {
        None
    };
    let source = unpacked
        .as_ref()
        .map_or(source, |(_, root)| root.as_path());
    let mut entries: Vec<(String, String, u8, bool, Vec<u8>)> = Vec::new();
    let mut total_bytes = 0u64;
    if source.pack_is_dir() {
        for entry in WalkDir::new(source).min_depth(1).follow_links(false) {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            let relative = entry.path().strip_prefix(source)?.to_string_lossy();
            if let Some((group, priority, atlas)) = candidate(&relative) {
                ensure!(entry.metadata()?.len() <= 32 * 1024 * 1024, "准星资源过大");
                total_bytes += entry.metadata()?.len();
                ensure!(total_bytes <= 128 * 1024 * 1024, "单个包的准星资源总量过大");
                entries.push((
                    group,
                    relative.into_owned(),
                    priority,
                    atlas,
                    fs::read(entry.path())?,
                ));
                ensure!(entries.len() <= 512, "单个包的准星资源超过 512 个");
            }
        }
    } else {
        let mut zip = ZipArchive::new(File::open(source).context("无法打开准星包")?)?;
        let mut candidates = Vec::new();
        for index in 0..zip.len() {
            let entry = zip.by_index(index)?;
            if entry.is_dir() {
                continue;
            }
            if let Some((group, priority, atlas)) = candidate(entry.name()) {
                ensure!(entry.size() <= 32 * 1024 * 1024, "准星资源过大");
                total_bytes += entry.size();
                ensure!(total_bytes <= 128 * 1024 * 1024, "单个包的准星资源总量过大");
                candidates.push((index, group, entry.name().to_string(), priority, atlas));
                ensure!(candidates.len() <= 512, "单个包的准星资源超过 512 个");
            }
        }
        for (index, group, path, priority, atlas) in candidates {
            let mut bytes = Vec::new();
            zip.by_index(index)?
                .take(32 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= 32 * 1024 * 1024, "准星资源过大");
            entries.push((group, path, priority, atlas, bytes));
        }
    }
    entries.sort_by(|a, b| (&a.0, a.2, &a.1).cmp(&(&b.0, b.2, &b.1)));
    let mut seen = std::collections::BTreeSet::new();
    let mut result = Vec::new();
    let mut errors = Vec::new();
    for (group, path, _, atlas, bytes) in entries {
        if seen.contains(&group) {
            continue;
        }
        let image = match decode(&bytes)
            .with_context(|| format!("无法读取 {path}"))
            .and_then(|image| normalized(image, atlas, true))
        {
            Ok(image) => image,
            Err(error) => {
                errors.push(format!("{path}：{error:#}"));
                continue;
            }
        };
        let title = if group.is_empty() {
            name.clone()
        } else {
            format!("{name} · {group}")
        };
        result.push(Crosshair { name: title, image });
        seen.insert(group);
    }
    ensure!(
        !result.is_empty(),
        "没有找到有效准星，请导入准星包、PNG 图片或包含准星的文件夹。{}",
        errors.first().cloned().unwrap_or_default()
    );
    Ok(result)
}

#[derive(Clone, Debug)]
pub struct Preset {
    pub id: String,
    pub crosshair: Crosshair,
    pub builtin: bool,
}

pub fn builtin_presets() -> Result<Vec<Preset>> {
    builtin::BUILTINS
        .iter()
        .map(|(id, name, bytes)| {
            Ok(Preset {
                id: (*id).into(),
                crosshair: Crosshair {
                    name: (*name).into(),
                    image: transparent_background(&decode(bytes)?),
                },
                builtin: true,
            })
        })
        .collect()
}

#[derive(Deserialize, Serialize)]
struct Metadata {
    name: String,
}

#[derive(Clone)]
pub struct Store {
    directory: PathBuf,
}

impl Store {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    fn path(&self, id: &str) -> Result<PathBuf> {
        ensure!(
            id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "预设编号无效"
        );
        Ok(self.directory.join(id))
    }

    pub fn load(&self) -> Result<Vec<Preset>> {
        let mut builtins: Vec<_> = builtin_presets()?
            .into_iter()
            .filter(|preset| {
                !self
                    .directory
                    .join(&preset.id)
                    .join("hidden")
                    .pack_is_file()
            })
            .collect();
        if !self.directory.pack_exists() {
            return Ok(builtins);
        }
        let mut result = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            let Ok(path) = self.path(&id) else {
                continue;
            };
            let preset = (|| -> Result<Preset> {
                let meta: Metadata = serde_json::from_slice(&fs::read(path.join("name.json"))?)?;
                let image = normalized(
                    decode(&fs::read(path.join("crosshair.png"))?)?,
                    false,
                    false,
                )?;
                Ok(Preset {
                    id,
                    crosshair: Crosshair {
                        name: meta.name,
                        image,
                    },
                    builtin: false,
                })
            })();
            if let Ok(preset) = preset {
                result.push(preset);
            }
        }
        result.sort_by(|a, b| (&a.crosshair.name, &a.id).cmp(&(&b.crosshair.name, &b.id)));
        builtins.extend(result);
        Ok(builtins)
    }

    pub fn save(&self, crosshair: &Crosshair) -> Result<String> {
        let crosshair = Crosshair {
            name: crosshair.name.clone(),
            image: crosshair.image.clone(),
        };
        if let Some(preset) = builtin_presets()?
            .into_iter()
            .find(|preset| preset.crosshair.image == crosshair.image)
        {
            let hidden = self.path(&preset.id)?.join("hidden");
            if hidden.pack_is_file() {
                fs::remove_file(hidden)?;
            }
            return Ok(preset.id);
        }
        if let Some(preset) = self
            .load()?
            .into_iter()
            .find(|preset| !preset.builtin && preset.crosshair.image == crosshair.image)
        {
            return Ok(preset.id);
        }
        let mut digest = Sha256::new();
        digest.update(crosshair.image.width().to_le_bytes());
        digest.update(crosshair.image.height().to_le_bytes());
        digest.update(crosshair.image.as_raw());
        let id = format!("{:x}", digest.finalize());
        let destination = self.path(&id)?;
        fs::create_dir_all(&self.directory)?;
        if !destination.pack_exists() {
            let temporary = tempfile::Builder::new()
                .prefix("import-")
                .tempdir_in(&self.directory)?;
            crosshair
                .image
                .save(temporary.path().join("crosshair.png"))?;
            fs::write(
                temporary.path().join("name.json"),
                serde_json::to_vec_pretty(&Metadata {
                    name: crosshair.name.clone(),
                })?,
            )?;
            fs::rename(temporary.path(), &destination)?;
        }
        Ok(id)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let path = self.path(id)?;
        if builtin::BUILTINS
            .iter()
            .any(|(builtin, _, _)| *builtin == id)
        {
            fs::create_dir_all(&path)?;
            let temporary = tempfile::NamedTempFile::new_in(&path)?;
            fs::write(temporary.path(), b"hidden")?;
            temporary.persist(path.join("hidden"))?;
            return Ok(());
        }
        if !path.pack_exists() {
            return Ok(());
        }
        ensure!(
            !fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "预设目录不能是链接"
        );
        for name in ["crosshair.png", "name.json"] {
            let file = path.join(name);
            if file.pack_is_file() {
                fs::remove_file(file)?;
            }
        }
        fs::remove_dir(path).context("无法删除准星预设")
    }

    pub fn restore_builtins(&self) -> Result<()> {
        for (id, _, _) in builtin::BUILTINS {
            let path = self.path(id)?.join("hidden");
            if path.pack_is_file() {
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }
}

pub(crate) fn apply(
    root: &Path,
    target: u32,
    crosshair: &Crosshair,
    report: &mut crate::compat::assets::AssetReport,
) -> Result<()> {
    let tile = normalized(crosshair.image.clone(), false, false)?;
    crate::compat::rainbow::clear_crosshair_animation(root)?;
    let output = root.join(if target >= 18 { SPRITE } else { ICONS });
    if output.pack_is_file() {
        crate::compat::assets::backup(root, &output)?;
    }
    let image = if target >= 18 {
        tile
    } else {
        let mut atlas = if output.pack_is_file() {
            decode(&fs::read(&output)?)?
        } else {
            decode(include_bytes!("../assets/crosshair/legacy-icons.png"))?
        };
        ensure!(
            atlas.width().is_multiple_of(256) && atlas.height() >= atlas.width(),
            "目标 HUD 图集比例无效"
        );
        let scale = (atlas.width() / 256).max((tile.width() / 15).min(16));
        if atlas.width() != 256 * scale {
            atlas = imageops::resize(
                &atlas,
                256 * scale,
                atlas.height() * 256 * scale / atlas.width(),
                imageops::FilterType::Nearest,
            );
        }
        let tile = imageops::resize(&tile, 15 * scale, 15 * scale, imageops::FilterType::Nearest);
        for y in 0..16 * scale {
            for x in 0..16 * scale {
                atlas.put_pixel(x, y, Rgba([0; 4]));
            }
        }
        imageops::replace(&mut atlas, &tile, 0, 0);
        atlas
    };
    crate::compat::assets::save(root, &output, &image)?;
    let metadata = output.with_extension("png.mcmeta");
    if target >= 18 && metadata.pack_is_file() {
        crate::compat::assets::backup(root, &metadata)?;
        fs::remove_file(metadata)?;
    }
    report.converted.push(format!(
        "自定义准星：{}，自动适配目标 {} 布局",
        crosshair.name,
        if target >= 18 {
            "独立精灵"
        } else {
            "HUD 图集"
        }
    ));
    Ok(())
}

#[cfg(test)]
mod color_tests {
    use super::*;
    #[test]
    fn explicit_black_pixels_survive_preset_save_load_and_conversion() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::new(temp.path().join("presets"));
        let crosshair = Crosshair {
            name: "Black square".into(),
            image: RgbaImage::from_pixel(15, 15, Rgba([0, 0, 0, 255])),
        };
        let id = store.save(&crosshair).unwrap();
        let loaded = store
            .load()
            .unwrap()
            .into_iter()
            .find(|preset| preset.id == id)
            .unwrap();
        assert_eq!(loaded.crosshair.image, crosshair.image);
        for target in [1, 15, 18, 100] {
            let root = temp.path().join(target.to_string());
            let mut report = crate::compat::assets::AssetReport::default();
            apply(&root, target, &loaded.crosshair, &mut report).unwrap();
            let path = root.join(if target >= 18 { SPRITE } else { ICONS });
            assert_eq!(
                *image::open(path).unwrap().to_rgba8().get_pixel(0, 0),
                Rgba([0, 0, 0, 255])
            );
        }
    }
    #[test]
    fn raw_imports_still_remove_black_backgrounds() {
        let mut image = RgbaImage::from_pixel(15, 15, Rgba([0, 0, 0, 255]));
        image.put_pixel(7, 7, Rgba([255; 4]));
        let imported = normalized(image, false, true).unwrap();
        assert_eq!(*imported.get_pixel(0, 0), Rgba([0; 4]));
        assert_eq!(*imported.get_pixel(7, 7), Rgba([255; 4]));
    }
}
