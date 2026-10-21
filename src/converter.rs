use crate::packfs::PathExt;
use crate::packfs::WalkDir;
use crate::packfs::{self as fs, File};
use crate::tables::{
    LATEST_VERSION, LEGACY_VERSION, NEW_META_MIN_FORMAT, flatten_texture_name, legacy_texture_name,
    tables, version_label_of,
};
use anyhow::{Context, Result, bail, ensure};
use chrono::Local;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tempfile::{TempDir, tempdir};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

#[derive(Clone, Debug)]
pub struct ConversionOptions {
    pub src_format: Option<u32>,
    pub tgt_format: u32,
    pub description: Option<String>,
    pub as_zip: bool,
    pub lowercase: bool,
    pub convert_lang: bool,
    pub extra_renames: Vec<(String, String)>,
    pub target_version: Option<String>,
    pub glint_color: Option<[u8; 3]>,
    pub restore_vanilla_glint: bool,
    pub heypixel_compat: bool,
    pub auto_client_setup: bool,
    pub auto_netherite: bool,
    pub auto_netherite_sword: bool,
    pub crosshair: Option<crate::crosshair::Crosshair>,
    pub show_attack_indicator: bool,
    pub custom_items: Vec<crate::compat::custom_items::Rule>,
    pub fishing_line_color: Option<[u8; 3]>,
    pub block_outline_color: Option<[u8; 3]>,
    pub line_color_selection: Option<crate::compat::line_colors::ColorSelection>,
    pub line_widths: crate::compat::line_colors::Widths,
    pub rainbow: crate::compat::rainbow::Settings,
    pub builtin_overlays: crate::builtin_overlays::Settings,
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
            src_format: None,
            tgt_format: 100,
            description: None,
            as_zip: true,
            lowercase: true,
            convert_lang: true,
            extra_renames: Vec::new(),
            target_version: None,
            glint_color: None,
            restore_vanilla_glint: false,
            heypixel_compat: crate::client_audio::ENABLED,
            auto_client_setup: true,
            auto_netherite: true,
            auto_netherite_sword: true,
            crosshair: None,
            show_attack_indicator: true,
            custom_items: Vec::new(),
            fishing_line_color: None,
            block_outline_color: None,
            line_color_selection: None,
            line_widths: Default::default(),
            rainbow: Default::default(),
            builtin_overlays: Default::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversionReport {
    pub src_format: u32,
    pub tgt_format: u32,
    pub direction: String,
    pub files_total: usize,
    pub dirs_moved: Vec<String>,
    pub textures_renamed: Vec<String>,
    pub lowercased: usize,
    pub lang_converted: Vec<String>,
    pub json_updated: usize,
    pub skipped_conflict: Vec<String>,
    pub unmapped: Vec<String>,
    pub manual_todo: Vec<String>,
    pub output: Option<String>,
    pub meta_written: bool,
    #[serde(default)]
    pub target_version: Option<String>,
    #[serde(default)]
    pub target_format_components: Option<[u32; 2]>,
    #[serde(default)]
    pub assets: crate::compat::assets::AssetReport,
    #[serde(default)]
    pub compatibility: crate::compat::CompatibilityReport,
}

impl ConversionReport {
    pub fn render(&self) -> String {
        let separator = "========================================================";
        let mut lines = vec![
            separator.into(),
            "  Fast Pack 材质包转换报告".into(),
            separator.into(),
            format!(
                "  源格式版本 : {} (pack_format {})",
                version_label_of(self.src_format),
                self.src_format
            ),
            format!(
                "  目标版本   : {} (pack_format {})",
                self.target_version
                    .clone()
                    .unwrap_or_else(|| version_label_of(self.tgt_format)),
                self.tgt_format
            ),
            format!("  文件总数   : {}", self.files_total),
            format!("  转换方向   : {}", self.direction),
            String::new(),
            "--- 目录迁移 ---".into(),
        ];
        append_entries(&mut lines, &self.dirs_moved, usize::MAX, true);
        lines.extend([
            String::new(),
            format!("--- 纹理重命名 ({}) ---", self.textures_renamed.len()),
        ]);
        append_entries(&mut lines, &self.textures_renamed, 40, true);
        if self.textures_renamed.len() > 40 {
            lines.push(format!(
                "  ... 其余 {} 项省略",
                self.textures_renamed.len() - 40
            ));
        }
        lines.extend([String::new(), "--- 语言文件 ---".into()]);
        append_entries(&mut lines, &self.lang_converted, usize::MAX, true);
        lines.extend([
            String::new(),
            format!("--- 模型/JSON 引用更新 : {} 个文件", self.json_updated),
        ]);
        if self.lowercased > 0 {
            lines.push(format!("--- 文件名小写规范化 : {} 项", self.lowercased));
        }
        if !self.skipped_conflict.is_empty() {
            lines.push(format!(
                "--- 冲突跳过 ({}) ---",
                self.skipped_conflict.len()
            ));
            append_entries(&mut lines, &self.skipped_conflict, 20, false);
        }
        if !self.unmapped.is_empty() {
            lines.push(if self.direction == "降级" {
                format!(
                    "--- 未能反向映射 ({})：1.13 之后新增的纹理，1.8 中无对应物，已原样保留 ---",
                    self.unmapped.len()
                )
            } else {
                format!("--- 保留的未知纹理 ({}) ---", self.unmapped.len())
            });
            append_entries(&mut lines, &self.unmapped, 30, false);
        }
        lines.extend([String::new(), "--- 自动资源适配 ---".into()]);
        append_entries(&mut lines, &self.assets.converted, usize::MAX, true);
        append_entries(&mut lines, &self.assets.generated, usize::MAX, false);
        if !self.compatibility.is_empty() {
            lines.extend([String::new(), "--- 天空与材质包着色器 ---".into()]);
            append_entries(
                &mut lines,
                &self.compatibility.sky_converted,
                usize::MAX,
                false,
            );
            append_entries(
                &mut lines,
                &self.compatibility.shader_adapted,
                usize::MAX,
                false,
            );
            append_entries(
                &mut lines,
                &self.compatibility.requirements,
                usize::MAX,
                false,
            );
        }
        let notes: Vec<_> = self
            .assets
            .warnings
            .iter()
            .chain(&self.compatibility.warnings)
            .cloned()
            .collect();
        if !notes.is_empty() {
            lines.extend([String::new(), "--- 处理结果与兼容边界 ---".into()]);
            append_entries(&mut lines, &notes, usize::MAX, false);
        }
        lines.extend([
            String::new(),
            format!("输出: {}", self.output.as_deref().unwrap_or("None")),
            separator.into(),
        ]);
        lines.join("\n")
    }
}

fn append_entries(lines: &mut Vec<String>, entries: &[String], limit: usize, show_empty: bool) {
    if entries.is_empty() && show_empty {
        lines.push("  (无)".into());
    } else {
        lines.extend(entries.iter().take(limit).map(|entry| format!("  {entry}")));
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackInfo {
    pub ok: bool,
    pub path: String,
    pub name: String,
    pub src_format: Option<u32>,
    pub version_label: Option<String>,
    pub file_count: usize,
    pub texture_count: usize,
    pub has_mcmeta: bool,
    pub has_assets: bool,
    pub description: String,
    pub layout: Option<String>,
    pub suggested_target: String,
    pub error: Option<String>,
    #[serde(default)]
    pub features: crate::compat::PackFeatures,
}

fn directory_entries(path: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = fs::read_dir(path)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort();
    Ok(entries)
}

fn walk_files(path: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in WalkDir::new(path).min_depth(1) {
        let entry = entry?;
        if entry.file_type().is_file() {
            files.push(entry.into_path());
        }
    }
    files.sort();
    Ok(files)
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in WalkDir::new(source).min_depth(1) {
        let entry = entry?;
        ensure!(
            !entry.file_type().is_symlink(),
            "不支持符号链接: {}",
            entry.path().display()
        );
        let out = destination.join(entry.path().strip_prefix(source)?);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&out)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &out)?;
        }
    }
    Ok(())
}

pub(crate) fn find_pack_root(base: &Path) -> Result<PathBuf> {
    if base.join("pack.mcmeta").pack_exists() {
        return Ok(base.into());
    }
    let children = directory_entries(base)?;
    for name in ["pack.mcmeta", "assets"] {
        for child in &children {
            if child.pack_is_dir() && child.join(name).pack_exists() {
                return Ok(child.clone());
            }
        }
    }
    Ok(base.into())
}

pub fn load_pack(source: &Path) -> Result<(TempDir, PathBuf)> {
    let work = tempdir().context("无法创建临时工作目录")?;
    let root = load_pack_into(source, &work.path().join("in"))?;
    Ok((work, root))
}

fn load_pack_into(source: &Path, base: &Path) -> Result<PathBuf> {
    if source.pack_is_dir() && fs::mounted(base) {
        fs::create_dir_all(base)?;
        let mut files = Vec::new();
        for entry in WalkDir::new(source).min_depth(1) {
            let entry = entry?;
            ensure!(
                !entry.file_type().is_symlink(),
                "不支持符号链接: {}",
                entry.path().display()
            );
            let target = base.join(entry.path().strip_prefix(source)?);
            if entry.file_type().is_dir() {
                fs::create_dir_all(target)?;
            } else if entry.file_type().is_file() {
                files.push((entry.into_path(), target));
            }
        }
        let loaded = crate::parallel::map(&files, |(path, target)| {
            Ok((target.clone(), Arc::<[u8]>::from(std::fs::read(path)?)))
        })?;
        for (target, bytes) in loaded {
            fs::write_shared(&target, bytes)?;
        }
    } else if source.pack_is_dir() {
        copy_tree(source, &base)?;
    } else if !fs::mounted(base) {
        fs::create_dir_all(base)?;
        let mut archive = ZipArchive::new(crate::archive::ArchiveReader::new(
            std::fs::File::open(source)?,
        )?)?;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            let name = entry.name().replace('\\', "/");
            let relative = Path::new(&name);
            if name.contains(':')
                || relative.is_absolute()
                || relative.components().any(|part| {
                    matches!(
                        part,
                        Component::ParentDir | Component::RootDir | Component::Prefix(_)
                    )
                })
                || entry
                    .unix_mode()
                    .is_some_and(|mode| mode & 0o170000 == 0o120000)
            {
                continue;
            }
            let path = base.join(relative);
            if entry.is_dir() {
                fs::create_dir_all(path)?;
            } else {
                fs::create_dir_all(path.parent().unwrap())?;
                std::io::copy(&mut entry, &mut File::create(path)?)?;
            }
        }
    } else {
        fs::create_dir_all(&base)?;
        let bytes: Arc<[u8]> = std::fs::read(source)
            .with_context(|| format!("无法读取 {}", source.display()))?
            .into();
        let mut archive = ZipArchive::new(Cursor::new(bytes)).context("不是有效的 ZIP 材质包")?;
        let mut files = Vec::new();
        for index in 0..archive.len() {
            let entry = archive.by_index_raw(index)?;
            let name = entry.name().replace('\\', "/");
            let relative = Path::new(&name);
            if name.contains(':')
                || relative.is_absolute()
                || relative.components().any(|part| {
                    matches!(
                        part,
                        Component::ParentDir | Component::RootDir | Component::Prefix(_)
                    )
                })
            {
                continue;
            }
            if entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
            {
                continue;
            }
            let target = base.join(relative);
            if entry.is_dir() {
                fs::create_dir_all(&target)?;
            } else {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                files.push((index, target));
            }
        }
        let chunks: Vec<_> = files
            .chunks(files.len().div_ceil(crate::parallel::threads()).max(1))
            .collect();
        let loaded = crate::parallel::map(&chunks, |chunk| {
            let mut archive = archive.clone();
            chunk
                .iter()
                .map(|(index, target)| {
                    let mut entry = archive.by_index(*index)?;
                    let mut bytes = Vec::new();
                    bytes
                        .try_reserve_exact(usize::try_from(entry.size())?)
                        .context("材质包资源过大，内存不足")?;
                    entry.read_to_end(&mut bytes)?;
                    Ok((*index, target.clone(), Arc::<[u8]>::from(bytes)))
                })
                .collect::<Result<Vec<_>>>()
        })?;
        fs::set_source(base, archive);
        for chunk in loaded {
            for (index, target, bytes) in chunk {
                if target
                    .extension()
                    .is_some_and(|ext| matches!(ext.to_str(), Some("png" | "jpg" | "jpeg" | "ogg")))
                {
                    fs::retain_original(&target, index, bytes.clone());
                }
                fs::write_shared(&target, bytes)?;
            }
        }
    }
    find_pack_root(base)
}

fn numeric_format(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        .or_else(|| {
            value
                .as_f64()
                .filter(|value| value.is_finite() && *value >= 0.0 && *value <= u32::MAX as f64)
                .map(|value| value as u32)
        })
}

pub fn detect_format(root: &Path) -> Option<u32> {
    if let Ok(text) = fs::read_to_string(root.join("pack.mcmeta"))
        && let Ok(data) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}'))
        && let Some(format) = metadata_format(&data)
    {
        return Some(format);
    }
    root.join("assets/minecraft/textures/blocks")
        .pack_exists()
        .then_some(1)
}

pub(crate) fn metadata_format(value: &Value) -> Option<u32> {
    let pack = value.get("pack")?;
    pack.get("pack_format")
        .and_then(numeric_format)
        .or_else(|| {
            let format = pack.get("min_format")?;
            numeric_format(
                format
                    .as_array()
                    .and_then(|array| array.first())
                    .unwrap_or(format),
            )
        })
}

pub fn compact_json(value: &Value) -> String {
    let raw = value.to_string();
    let mut out = String::with_capacity(raw.len());
    let mut quoted = false;
    let mut escaped = false;
    for c in raw.chars() {
        out.push(c);
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && c == '\\' {
            escaped = true;
        } else if c == '"' {
            quoted = !quoted;
        } else if !quoted && matches!(c, ',' | ':') {
            out.push(' ');
        }
    }
    out
}

pub fn suggest_target(format: Option<u32>) -> &'static str {
    if format.is_some_and(|format| format >= crate::pack_format_of(LATEST_VERSION).unwrap_or(100)) {
        LEGACY_VERSION
    } else {
        LATEST_VERSION
    }
}

pub fn suggest_output(source: &Path, target: &str, as_zip: bool) -> PathBuf {
    let name = if source.pack_is_dir() {
        source.file_name()
    } else {
        source.file_stem()
    }
    .unwrap_or_default()
    .to_string_lossy();
    let mut stem = name.as_ref();
    while let Some((version, remainder)) = stem
        .strip_prefix('[')
        .and_then(|value| value.split_once(']'))
    {
        if !tables()
            .pack_formats
            .iter()
            .any(|(label, _)| label == version)
        {
            break;
        }
        stem = remainder;
    }
    let stem = if stem.is_empty() { "pack" } else { stem };
    let filename = format!("[{target}]{stem}{}", if as_zip { ".zip" } else { "" });
    if source
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(&filename))
    {
        source
            .parent()
            .unwrap_or(Path::new("."))
            .join("Fast Pack")
            .join(filename)
    } else {
        source.with_file_name(filename)
    }
}

pub fn probe_pack(path: &Path) -> PackInfo {
    crate::probe::probe_pack(path)
}

fn normalize_lowercase(root: &Path) -> Result<usize> {
    let mut entries = WalkDir::new(root)
        .min_depth(1)
        .into_iter()
        .map(|entry| entry.map(|entry| entry.into_path()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    let mut count = 0;
    for path in entries {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let lower = name.to_lowercase();
        if lower == name {
            continue;
        }
        let target = path.with_file_name(&lower);
        // A pure case change resolves to the same entry both in the
        // in-memory store (lowercased keys) and on case-insensitive
        // filesystems, so only skip when the target resolves to a
        // genuinely different entry.
        let conflict = match target.pack_canonicalize() {
            Ok(resolved) => {
                let self_resolved = path.pack_canonicalize().unwrap_or_else(|_| path.clone());
                resolved != path && resolved != self_resolved
            }
            Err(_) => false,
        };
        if conflict {
            continue;
        }
        if fs::rename(&path, &target).is_ok() {
            count += 1;
        }
    }
    Ok(count)
}

fn texture_references(value: &Value, namespace: &str, references: &mut HashSet<String>) {
    match value {
        Value::String(text) => {
            let (namespace, path) = text.split_once(':').unwrap_or((namespace, text));
            if ["block/", "blocks/", "item/", "items/"]
                .iter()
                .any(|prefix| path.starts_with(prefix))
            {
                references.insert(format!("{namespace}:{}", path.trim_end_matches(".png")));
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                texture_references(value, namespace, references);
            }
        }
        Value::Array(values) => {
            for value in values {
                texture_references(value, namespace, references);
            }
        }
        _ => {}
    }
}

fn prefer_legacy_texture(
    relative: &Path,
    destination: &Path,
    namespace: &str,
    kind: &str,
    source_format: u32,
    references: &HashSet<String>,
) -> bool {
    if namespace != "minecraft"
        || source_format >= 4
        || relative
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
    {
        return false;
    }
    let name = relative.file_name().unwrap_or_default().to_string_lossy();
    let Some(stem) = name
        .strip_suffix(".png.mcmeta")
        .or_else(|| name.strip_suffix(".png"))
    else {
        return false;
    };
    let known = if kind == "block" {
        crate::tables::legacy_blocks()
            .source_names
            .iter()
            .any(|name| name == stem)
    } else {
        crate::tables::legacy_items().items.contains_key(stem)
    };
    if !known {
        return false;
    }
    let name = destination
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let stem = name
        .strip_suffix(".png.mcmeta")
        .or_else(|| name.strip_suffix(".png"))
        .unwrap_or(&name);
    !references.contains(&format!("{namespace}:{kind}/{stem}"))
        && !references.contains(&format!("{namespace}:{kind}s/{stem}"))
}

fn backup_texture(root: &Path, path: &Path) -> Result<()> {
    let destination = root
        .join("fastpack/original-assets")
        .join(path.strip_prefix(root)?);
    if !destination.pack_exists() {
        fs::create_dir_all(destination.parent().unwrap())?;
        fs::copy(path, destination)?;
    }
    Ok(())
}

fn migrate_textures(
    root: &Path,
    reverse: bool,
    options: &ConversionOptions,
    report: &mut ConversionReport,
) -> Result<HashMap<String, String>> {
    let assets = root.join("assets");
    if !assets.pack_is_dir() {
        return Ok(HashMap::new());
    }
    let mut references = HashSet::new();
    for path in walk_files(&assets)? {
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let namespace = path
            .strip_prefix(&assets)?
            .components()
            .next()
            .unwrap()
            .as_os_str()
            .to_string_lossy();
        if let Ok(value) = serde_json::from_slice::<Value>(&fs::read(&path)?) {
            texture_references(&value, &namespace, &mut references);
        }
    }
    let pairs = if reverse {
        [("block", "blocks"), ("item", "items")]
    } else {
        [("blocks", "block"), ("items", "item")]
    };
    let mut renames = HashMap::new();
    for namespace in directory_entries(&assets)? {
        if !namespace.pack_is_dir() {
            continue;
        }
        let namespace_name = namespace.file_name().unwrap_or_default().to_string_lossy();
        let texture = namespace.join("textures");
        for (old, new) in pairs {
            let source = texture.join(old);
            let target = texture.join(new);
            if !source.pack_is_dir() {
                continue;
            }
            fs::create_dir_all(&target)?;
            for path in walk_files(&source)? {
                let relative = path.strip_prefix(&source)?;
                let destination = target.join(relative);
                if destination.pack_exists() {
                    if !reverse
                        && prefer_legacy_texture(
                            relative,
                            &destination,
                            &namespace_name,
                            new,
                            report.src_format,
                            &references,
                        )
                    {
                        backup_texture(root, &destination)?;
                        fs::remove_file(&destination)?;
                    } else {
                        report
                            .skipped_conflict
                            .push(destination.display().to_string());
                        continue;
                    }
                }
                fs::create_dir_all(destination.parent().unwrap())?;
                fs::rename(path, destination)?;
            }
            let _ = fs::remove_dir(source);
            report
                .dirs_moved
                .push(format!("textures/{old} -> textures/{new}"));
        }
        let folders = if reverse {
            [("blocks", "block"), ("items", "item")]
        } else {
            [("block", "block"), ("item", "item")]
        };
        for (folder, kind) in folders {
            let directory = texture.join(folder);
            if !directory.pack_exists() {
                continue;
            }
            for path in walk_files(&directory)? {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let (stem, suffix) = if let Some(stem) = name.strip_suffix(".png.mcmeta") {
                    (stem, ".png.mcmeta")
                } else if let Some(stem) = name.strip_suffix(".png") {
                    (stem, ".png")
                } else {
                    continue;
                };
                let renamed = if namespace_name != "minecraft" {
                    Some(stem.into())
                } else if reverse {
                    options
                        .extra_renames
                        .iter()
                        .rev()
                        .find(|(old, new)| new == stem && !old.is_empty())
                        .map(|(old, _)| old.clone())
                        .or_else(|| legacy_texture_name(stem, kind))
                } else {
                    options
                        .extra_renames
                        .iter()
                        .find(|(old, new)| old == stem && !new.is_empty())
                        .map(|(_, new)| new.clone())
                        .or_else(|| Some(flatten_texture_name(stem, kind)))
                };
                if let Some(new) = renamed.filter(|new| !new.is_empty()) {
                    if new == stem {
                        continue;
                    }
                    ensure!(
                        !new.contains(['/', '\\', ':']) && new != "..",
                        "自定义纹理名称不合法: {new}"
                    );
                    let target = path.with_file_name(format!("{new}{suffix}"));
                    if target.pack_exists() {
                        if !reverse
                            && prefer_legacy_texture(
                                path.strip_prefix(&directory)?,
                                &target,
                                &namespace_name,
                                kind,
                                report.src_format,
                                &references,
                            )
                        {
                            backup_texture(root, &target)?;
                            fs::remove_file(&target)?;
                        } else {
                            report.skipped_conflict.push(target.display().to_string());
                            continue;
                        }
                    }
                    let relative = path
                        .strip_prefix(&directory)?
                        .to_string_lossy()
                        .replace('\\', "/");
                    let old_path = relative.strip_suffix(suffix).unwrap_or(&relative);
                    let parent = Path::new(old_path).parent().unwrap_or(Path::new(""));
                    let new_path = parent.join(&new).to_string_lossy().replace('\\', "/");
                    fs::rename(&path, target)?;
                    renames.insert(format!("{namespace_name}:{kind}/{old_path}"), new_path);
                    report
                        .textures_renamed
                        .push(format!("textures/{folder}/{name} -> {new}{suffix}"));
                } else if !reverse || !tables().deferred_unmapped.iter().any(|item| item == stem) {
                    report.unmapped.push(format!("textures/{folder}/{name}"));
                }
            }
        }
    }
    Ok(renames)
}

fn update_json_refs(
    root: &Path,
    reverse: bool,
    renames: &HashMap<String, String>,
) -> Result<usize> {
    let sources = ["blocks?", "items?"];
    let targets = if reverse {
        ["blocks", "items"]
    } else {
        ["block", "item"]
    };
    let patterns = sources.map(|source| {
        Regex::new(&format!(
            r"^([A-Za-z0-9_.-]+:)?({source})/([A-Za-z0-9_./-]+)$"
        ))
        .unwrap()
    });
    let mut count = 0;
    for path in walk_files(root)? {
        if path.strip_prefix(root)?.starts_with("fastpack") {
            continue;
        }
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let Ok(mut value) = serde_json::from_slice::<Value>(&fs::read(&path)?) else {
            continue;
        };
        let original = value.clone();
        rewrite_texture_values(&mut value, &patterns, &targets, reverse, renames, false);
        if value != original {
            fs::write(path, serde_json::to_vec_pretty(&value)?)?;
            count += 1;
        }
    }
    Ok(count)
}

fn canonical_texture_aliases(
    root: &Path,
    target: u32,
    report: &mut ConversionReport,
) -> Result<HashMap<String, String>> {
    let mut renames = HashMap::new();
    if target < 4 {
        return Ok(renames);
    }
    let directory = root.join("assets/minecraft/textures/block");
    if !directory.pack_is_dir() {
        return Ok(renames);
    }
    for path in walk_files(&directory)? {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let (stem, suffix) = if let Some(stem) = name.strip_suffix(".png.mcmeta") {
            (stem, ".png.mcmeta")
        } else if let Some(stem) = name.strip_suffix(".png") {
            (stem, ".png")
        } else {
            continue;
        };
        let Some(mapped) = crate::tables::legacy_blocks().aliases.get(stem) else {
            continue;
        };
        let destination = path.with_file_name(format!("{mapped}{suffix}"));
        if destination.pack_exists() {
            continue;
        }
        let relative = path.strip_prefix(root)?;
        let backup = root.join("fastpack/original-assets").join(relative);
        if !backup.pack_exists() {
            fs::create_dir_all(backup.parent().unwrap())?;
            fs::copy(&path, backup)?;
        }
        let old_path = path
            .strip_prefix(&directory)?
            .to_string_lossy()
            .replace('\\', "/");
        let old_path = old_path.strip_suffix(suffix).unwrap_or(&old_path);
        let new_path = Path::new(old_path)
            .with_file_name(mapped)
            .to_string_lossy()
            .replace('\\', "/");
        fs::rename(&path, &destination)?;
        renames.insert(format!("minecraft:block/{old_path}"), new_path);
        report.textures_renamed.push(format!(
            "textures/block/{old_path}{suffix} → {mapped}{suffix}"
        ));
    }
    Ok(renames)
}

fn rewrite_texture_values(
    value: &mut Value,
    patterns: &[Regex; 2],
    targets: &[&str; 2],
    reverse: bool,
    renames: &HashMap<String, String>,
    texture_dictionary: bool,
) {
    match value {
        Value::String(text) => {
            for ((pattern, target), kind) in patterns.iter().zip(targets).zip(["block", "item"]) {
                if let Some(capture) = pattern.captures(text) {
                    let namespace = capture.get(1).map(|value| value.as_str()).unwrap_or("");
                    let namespace_name = namespace.trim_end_matches(':');
                    let domain = if namespace_name.is_empty() {
                        "minecraft"
                    } else {
                        namespace_name
                    };
                    let folder = &capture[2];
                    let name = &capture[3];
                    let (resource, suffix) = if let Some(value) = name.strip_suffix(".png") {
                        (value, ".png")
                    } else {
                        (name, "")
                    };
                    let key = format!("{domain}:{kind}/{}", resource.to_lowercase());
                    let mapped = renames.get(&key).cloned().unwrap_or_else(|| {
                        if domain != "minecraft" {
                            return resource.into();
                        }
                        if reverse == folder.ends_with('s') {
                            return resource.into();
                        }
                        let path = Path::new(resource);
                        let leaf = path.file_name().unwrap_or_default().to_string_lossy();
                        let name = leaf.as_ref();
                        let mapped = if reverse {
                            legacy_texture_name(
                                match name {
                                    "dirt_path" => "grass_path",
                                    "short_grass" => "grass",
                                    _ => name,
                                },
                                kind,
                            )
                            .unwrap_or_else(|| name.into())
                        } else {
                            flatten_texture_name(name, kind)
                        };
                        path.with_file_name(mapped)
                            .to_string_lossy()
                            .replace('\\', "/")
                    });
                    *text = format!("{namespace}{target}/{mapped}{suffix}");
                    break;
                }
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                if texture_dictionary || !matches!(key.as_str(), "parent" | "model") {
                    rewrite_texture_values(
                        value,
                        patterns,
                        targets,
                        reverse,
                        renames,
                        texture_dictionary || key == "textures",
                    );
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                rewrite_texture_values(
                    value,
                    patterns,
                    targets,
                    reverse,
                    renames,
                    texture_dictionary,
                );
            }
        }
        _ => {}
    }
}

fn convert_languages(root: &Path, reverse: bool, report: &mut ConversionReport) -> Result<()> {
    let directory = root.join("assets/minecraft/lang");
    if !directory.pack_exists() {
        return Ok(());
    }
    for path in directory_entries(&directory)? {
        if !path.pack_is_file() {
            continue;
        }
        let extension = path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        if extension != if reverse { "json" } else { "lang" } {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let (name, output, count) = if reverse {
            let Ok(text) = String::from_utf8(bytes) else {
                continue;
            };
            let Ok(Value::Object(values)) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let mut parts = stem.split('_');
            let base = parts.next().unwrap_or_default().to_lowercase();
            let region = parts.next().unwrap_or_default().to_uppercase();
            let name = if region.is_empty() {
                format!("{base}.lang")
            } else {
                format!("{base}_{region}.lang")
            };
            let mut lines = vec![
                "# Converted by Fast Pack (1.13+ JSON -> 1.8 .lang)".into(),
                format!("# {}", Local::now().format("%Y-%m-%d %H:%M:%S")),
                String::new(),
            ];
            for (key, value) in &values {
                lines.push(format!(
                    "{key}={}",
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| compact_json(value))
                ));
            }
            (name, lines.join("\n") + "\n", values.len())
        } else {
            let mut values = Map::new();
            for line in String::from_utf8_lossy(&bytes).lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once('=') {
                    values.insert(key.trim().into(), Value::String(value.trim().into()));
                }
            }
            let count = values.len();
            (
                format!("{}.json", stem.to_lowercase()),
                serde_json::to_string_pretty(&Value::Object(values))?,
                count,
            )
        };
        fs::write(directory.join(&name), output)?;
        fs::remove_file(&path)?;
        report.lang_converted.push(format!(
            "lang/{} -> {name} ({count} 条)",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    Ok(())
}

fn post_renames(root: &Path, report: &mut ConversionReport) -> Result<()> {
    let texture = root.join("assets/minecraft/textures");
    for (boundary, old, new, label) in [
        (7, "grass_path", "dirt_path", "1.17"),
        (22, "grass", "short_grass", "1.20.3"),
    ] {
        if report.src_format < boundary && boundary <= report.tgt_format {
            for folder in ["block", "item"] {
                let path = texture.join(folder).join(format!("{old}.png"));
                if path.pack_exists() {
                    fs::rename(path, texture.join(folder).join(format!("{new}.png")))?;
                    report.textures_renamed.push(format!(
                        "textures/{folder}/{old}.png -> {new}.png ({label})"
                    ));
                }
            }
        }
    }
    for (boundary, old, new, label) in [
        (7, "grass_path", "dirt_path", "1.17"),
        (22, "grass", "short_grass", "1.20.3"),
    ] {
        if report.tgt_format < boundary && boundary <= report.src_format {
            for folder in ["blocks", "items"] {
                let path = texture.join(folder).join(format!("{new}.png"));
                if path.pack_exists() {
                    fs::rename(path, texture.join(folder).join(format!("{old}.png")))?;
                    report.textures_renamed.push(format!(
                        "textures/{folder}/{new}.png -> {old}.png (降级 {label})"
                    ));
                }
            }
        }
    }
    Ok(())
}

fn write_mcmeta(
    root: &Path,
    options: &ConversionOptions,
    report: &mut ConversionReport,
) -> Result<()> {
    let path = root.join("pack.mcmeta");
    let mut description = options.description.clone().unwrap_or_else(|| {
        fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|data| {
                data.get("pack")
                    .and_then(|pack| pack.get("description"))
                    .cloned()
            })
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| compact_json(&value))
            })
            .unwrap_or_default()
    });
    if description.is_empty() {
        description = format!(
            "Converted for Minecraft {}",
            options
                .target_version
                .clone()
                .unwrap_or_else(|| version_label_of(report.tgt_format))
        );
    }
    let mut pack = Map::new();
    pack.insert("description".into(), Value::String(description));
    if report.tgt_format >= NEW_META_MIN_FORMAT {
        let format =
            crate::tables::format_components(options.target_version.as_deref(), report.tgt_format);
        pack.insert("min_format".into(), json!(format));
        pack.insert("max_format".into(), json!(format));
    }
    pack.insert("pack_format".into(), json!(report.tgt_format));
    fs::write(path, serde_json::to_string_pretty(&json!({"pack": pack}))?)?;
    report.meta_written = true;
    Ok(())
}

fn absolute_normalized(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.into()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut result = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    let mut missing = Vec::new();
    let mut ancestor = result.clone();
    while !ancestor.pack_exists() {
        if let Some(name) = ancestor.file_name() {
            missing.push(name.to_owned());
        }
        if !ancestor.pop() {
            break;
        }
    }
    if ancestor.pack_exists() {
        result = ancestor.pack_canonicalize()?;
        for part in missing.into_iter().rev() {
            result.push(part);
        }
    }
    Ok(result)
}

fn validate_output(source: &Path, destination: &Path) -> Result<()> {
    let source = absolute_normalized(source)?;
    let destination = absolute_normalized(destination)?;
    ensure!(source != destination, "输出路径不能与源材质包相同");
    ensure!(
        !source.starts_with(&destination),
        "输出路径不能是源材质包的上级目录"
    );
    if source.pack_is_dir() {
        ensure!(
            !destination.starts_with(&source),
            "输出路径不能位于源材质包内部"
        );
    }
    Ok(())
}

pub(crate) fn archive_options(_: &Path) -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(9))
}

fn save_output(root: &Path, destination: &Path, as_zip: bool) -> Result<()> {
    if destination.pack_exists() {
        if destination.pack_is_dir() {
            ensure!(!as_zip, "ZIP 输出路径是一个文件夹");
            fs::remove_dir_all(destination)?;
        } else {
            ensure!(as_zip, "文件夹输出路径是一个文件");
        }
    }
    if let Some(parent) = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    if as_zip {
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        let mut archive =
            ZipWriter::new(crate::archive::ArchiveWriter::new(temporary.as_file_mut())?);
        if let Some(input) = fs::output_entries(root) {
            let chunks: Vec<_> = input
                .chunks(input.len().div_ceil(crate::parallel::threads()).max(1))
                .collect();
            let source = fs::source_archive(root);
            let compressed = crate::parallel::map(&chunks, |chunk| {
                let mut source = source.clone();
                let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
                for (name, bytes, index) in *chunk {
                    if let Some((source, index)) = source.as_mut().zip(*index) {
                        archive.raw_copy_file_rename(source.by_index(index)?, name)?;
                    } else {
                        archive.start_file(name, archive_options(Path::new(name)))?;
                        archive.write_all(bytes)?;
                    }
                }
                Ok(archive.finish()?.into_inner())
            })?;
            for bytes in compressed {
                archive.merge_archive(ZipArchive::new(Cursor::new(bytes))?)?;
            }
        } else {
            for path in walk_files(root)? {
                let name = path
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/");
                archive.start_file(name, archive_options(&path))?;
                std::io::copy(&mut File::open(path)?, &mut archive)?;
            }
        }
        archive.finish()?.flush()?;
        temporary
            .persist(destination)
            .map_err(|error| error.error)?;
    } else {
        copy_tree(root, destination)?;
    }
    Ok(())
}

pub fn convert_pack(
    source: &Path,
    destination: &Path,
    options: &ConversionOptions,
    mut progress: impl FnMut(&str, u8),
) -> Result<ConversionReport> {
    let heypixel_compat = options.heypixel_compat && crate::client_audio::ENABLED;
    validate_output(source, destination)?;
    progress("载入源包", 3);
    let work = tempdir().context("无法创建临时工作目录")?;
    let _memory = fs::Mount::for_pack(source, work.path())?;
    let root = load_pack_into(source, &work.path().join("in"))?;
    let src_format = options.src_format.unwrap_or_else(|| {
        detect_format(&root)
            .filter(|value| *value != 0)
            .unwrap_or(1)
    });
    let mut report = ConversionReport {
        src_format,
        tgt_format: options.tgt_format,
        target_version: options.target_version.clone(),
        target_format_components: Some(crate::tables::format_components(
            options.target_version.as_deref(),
            options.tgt_format,
        )),
        direction: if options.tgt_format > src_format {
            "升级"
        } else if options.tgt_format < src_format {
            "降级"
        } else {
            "同版本"
        }
        .into(),
        ..Default::default()
    };
    progress("扫描文件", 10);
    report.files_total = walk_files(&root)?.len();
    let model_json_notes = crate::model_json::normalize(&root)?;
    let builtin_settings = crate::builtin_overlays::effective(&root, &options.builtin_overlays)?;
    let builtin_notes = crate::builtin_overlays::prepare(&root)?;
    let native_cit_notes =
        crate::compat::native_cit::prepare(&root, src_format, options.tgt_format)?;
    let sky_notes = crate::compat::sky::prepare(&root)?;
    if src_format < 3 && 3 <= options.tgt_format {
        progress("小写规范化", 15);
        if options.lowercase {
            report.lowercased = normalize_lowercase(&root)?;
        }
    }
    let default_notes =
        crate::compat::legacy_defaults::prepare(&root, src_format, options.tgt_format)?;
    let upgrade = src_format < 4 && 4 <= options.tgt_format;
    let downgrade = options.tgt_format < 4 && 4 <= src_format;
    if upgrade || downgrade {
        progress(
            if downgrade {
                "反向扁平化与重命名"
            } else {
                "扁平化与重命名"
            },
            30,
        );
        let renames = migrate_textures(&root, downgrade, options, &mut report)?;
        progress("更新模型引用", 60);
        report.json_updated = update_json_refs(&root, downgrade, &renames)?;
        progress(
            if downgrade {
                "还原语言文件"
            } else {
                "转换语言文件"
            },
            70,
        );
        if options.convert_lang {
            convert_languages(&root, downgrade, &mut report)?;
        }
    }
    let aliases = canonical_texture_aliases(&root, options.tgt_format, &mut report)?;
    if !aliases.is_empty() {
        report.json_updated += update_json_refs(&root, false, &aliases)?;
    }
    progress("后续版本微调", 78);
    post_renames(&root, &mut report)?;
    progress("自动适配实体、模型与界面", 80);
    report.assets = crate::compat::assets::migrate_with_netherite(
        &root,
        src_format,
        options.tgt_format,
        options.auto_netherite,
        options.auto_netherite_sword,
    )?;
    report.assets.converted.extend(default_notes);
    report.assets.converted.extend(model_json_notes);
    report.assets.converted.extend(native_cit_notes);
    report.assets.warnings.extend(builtin_notes);
    crate::compat::migrate_versioned_assets(
        &root,
        src_format,
        options.tgt_format,
        options
            .target_version
            .as_deref()
            .unwrap_or(&version_label_of(options.tgt_format)),
        &mut report.assets,
        heypixel_compat,
    )?;
    progress("自动适配天空与材质包着色器", 82);
    report.compatibility = crate::compat::migrate(
        &root,
        src_format,
        options.tgt_format,
        options
            .target_version
            .as_deref()
            .unwrap_or(&version_label_of(options.tgt_format)),
        crate::compat::SkyRuntime::Auto,
        crate::compat::ShaderPolicy::Adaptive,
    )?;
    if options.rainbow.glint {
        crate::compat::glint::rainbow(&root, src_format, options.tgt_format, &mut report.assets)?;
    } else if options.restore_vanilla_glint && options.glint_color.is_none() {
        crate::compat::glint::restore_vanilla(&root, options.tgt_format, &mut report.assets)?;
    } else if options.glint_color.is_some() || report.compatibility.selected_variant.is_some() {
        crate::compat::glint::migrate(
            &root,
            options.tgt_format,
            options.tgt_format,
            options.glint_color,
            &mut report.assets,
        )?;
    }
    report.compatibility.warnings.extend(sky_notes);
    crate::builtin_overlays::apply(
        &root,
        options
            .target_version
            .as_deref()
            .unwrap_or(&version_label_of(options.tgt_format)),
        &builtin_settings,
        &mut report.assets,
    )?;
    // Explicit line-color choices must not revive an inherited rainbow outline.
    let outline_settings = if options.line_color_selection.is_some() {
        &options.builtin_overlays
    } else {
        &builtin_settings
    };
    let builtin_outline = outline_settings
        .enabled(crate::builtin_overlays::Feature::RainbowOutline)
        && options.tgt_format >= 7;
    crate::compat::line_colors::apply_selected(
        &root,
        options
            .target_version
            .as_deref()
            .unwrap_or(&version_label_of(options.tgt_format)),
        options.fishing_line_color,
        options
            .block_outline_color
            .or(builtin_outline.then_some([0; 3])),
        options.rainbow.fishing_line,
        options.rainbow.block_outline || (builtin_outline && options.block_outline_color.is_none()),
        options.line_widths,
        options.line_color_selection.map(|mut selection| {
            selection.outline |= builtin_outline;
            selection
        }),
        &mut report.assets,
    )?;
    if !options.custom_items.is_empty() {
        progress("应用物品自定义", 83);
        crate::compat::custom_items::apply(
            &root,
            options
                .target_version
                .as_deref()
                .unwrap_or(&version_label_of(options.tgt_format)),
            &options.custom_items,
            &mut report.assets,
        )?;
    }
    if let Some(crosshair) = &options.crosshair {
        crate::crosshair::apply(&root, options.tgt_format, crosshair, &mut report.assets)?;
        if options.rainbow.crosshair {
            crate::compat::rainbow::crosshair(&root, options.tgt_format, &mut report.assets)?;
        }
    }
    crate::compat::attack_indicator::apply(
        &root,
        options.tgt_format,
        options.show_attack_indicator,
        &mut report.assets,
    )?;
    report.compatibility.warnings.sort();
    report.compatibility.warnings.dedup();
    if !report.compatibility.is_empty() {
        fs::create_dir_all(root.join("fastpack"))?;
        fs::write(
            root.join("fastpack/compatibility.json"),
            serde_json::to_vec_pretty(&report.compatibility)?,
        )?;
    }
    progress("写入 pack.mcmeta", 84);
    write_mcmeta(&root, options, &mut report)?;
    progress("生成报告", 90);
    report.output = Some(destination.display().to_string());
    let embedded_report = root.join("转换报告.txt");
    if embedded_report.pack_is_file() {
        fs::remove_file(embedded_report)?;
    }
    progress("写入输出", 95);
    save_output(&root, destination, options.as_zip)?;
    report.output = Some(destination.display().to_string());
    if heypixel_compat
        && options.auto_client_setup
        && options.target_version.as_deref() == Some("1.20.1")
    {
        static CLIENT_SETUP: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _client_setup = CLIENT_SETUP
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        progress("自动适配布吉岛客户端", 97);
        let client_root = if options.as_zip {
            root.as_path()
        } else {
            destination
        };
        crate::client_audio::automatic(
            source,
            destination,
            client_root,
            &mut report.assets,
            |step| progress(step, 98),
        );
    }
    progress("完成", 100);
    Ok(report)
}

pub fn load_extra_renames(path: &Path) -> Result<Vec<(String, String)>> {
    let value: Value = serde_json::from_slice(&fs::read(path)?)?;
    let Some(values) = value.as_object() else {
        bail!("自定义映射必须是 JSON 对象");
    };
    values
        .iter()
        .map(|(key, value)| {
            value
                .as_str()
                .map(|value| (key.clone(), value.into()))
                .with_context(|| format!("自定义映射 {key} 的值必须是字符串"))
        })
        .collect()
}

#[cfg(test)]
mod speed_tests {
    use super::*;
    use std::io::{Seek, SeekFrom};

    #[test]
    fn corrupt_source_does_not_replace_existing_output() {
        let work = tempdir().unwrap();
        let source = work.path().join("source.zip");
        let output = work.path().join("output.zip");
        let mut zip = ZipWriter::new(std::fs::File::create(&source).unwrap());
        zip.start_file("pack.mcmeta", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(br#"{"pack":{"pack_format":15}}"#).unwrap();
        zip.start_file("assets/test/data.bin", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"payload").unwrap();
        zip.finish().unwrap();
        let offset = {
            let mut zip = ZipArchive::new(std::fs::File::open(&source).unwrap()).unwrap();
            zip.by_index_raw(1).unwrap().data_start()
        };
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .unwrap();
        file.seek(SeekFrom::Start(offset)).unwrap();
        file.write_all(b"!").unwrap();
        drop(file);
        std::fs::write(&output, b"previous output").unwrap();
        let options = ConversionOptions {
            auto_client_setup: false,
            ..Default::default()
        };
        assert!(convert_pack(&source, &output, &options, |_, _| {}).is_err());
        assert_eq!(std::fs::read(output).unwrap(), b"previous output");
    }

    #[test]
    fn parallel_output_preserves_payloads_for_java_compatible_zip_methods() {
        let work = tempdir().unwrap();
        let guard = fs::Mount::new(work.path());
        let root = work.path().join("pack");
        for index in 0..120 {
            fs::write(
                root.join(format!("assets/test/{index}.bin")),
                vec![index as u8; 2048 + index],
            )
            .unwrap();
        }
        let output_work = tempdir().unwrap();
        let output = output_work.path().join("result.zip");
        save_output(&root, &output, true).unwrap();
        drop(guard);
        let mut zip = ZipArchive::new(std::fs::File::open(output).unwrap()).unwrap();
        assert_eq!(zip.len(), 120);
        for index in 0..120 {
            let mut entry = zip.by_name(&format!("assets/test/{index}.bin")).unwrap();
            assert_eq!(entry.compression(), CompressionMethod::Deflated);
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, vec![index as u8; 2048 + index]);
        }
    }
}

#[cfg(test)]
mod lowercase_tests {
    use super::*;
    use image::ImageEncoder;

    fn write_png(path: &Path) {
        let image = image::RgbaImage::from_pixel(16, 16, image::Rgba([12, 34, 56, 255]));
        image::codecs::png::PngEncoder::new(std::fs::File::create(path).unwrap())
            .write_image(image.as_raw(), 16, 16, image::ExtendedColorType::Rgba8)
            .unwrap();
    }

    #[test]
    fn convert_pack_lowercases_mixed_case_names_in_store_mode() {
        let work = tempdir().unwrap();
        let source = work.path().join("source");
        std::fs::create_dir_all(source.join("assets/minecraft/textures/blocks/CustomDir")).unwrap();
        std::fs::write(
            source.join("pack.mcmeta"),
            br#"{"pack":{"pack_format":1,"description":"t"}}"#,
        )
        .unwrap();
        write_png(&source.join("assets/minecraft/textures/blocks/MyCustom.PNG"));
        write_png(&source.join("assets/minecraft/textures/blocks/CustomDir/Inner.PNG"));
        let output = tempdir().unwrap();
        let destination = output.path().join("out");
        let options = ConversionOptions {
            tgt_format: 88,
            lowercase: true,
            as_zip: false,
            ..Default::default()
        };
        let report = convert_pack(&source, &destination, &options, |_, _| {}).unwrap();
        let block = destination.join("assets/minecraft/textures/block");
        assert!(block.join("mycustom.png").exists());
        assert!(block.join("customdir/inner.png").exists());
        assert!(report.lowercased >= 2);
    }

    #[test]
    fn normalize_lowercase_renames_on_disk_and_skips_genuine_collisions() {
        let work = tempdir().unwrap();
        let root = work.path();
        std::fs::write(root.join("stone.png"), b"A").unwrap();
        std::fs::write(root.join("Stone.PNG"), b"B").unwrap();
        std::fs::create_dir_all(root.join("customdir")).unwrap();
        std::fs::write(root.join("customdir/Inner.PNG"), b"C").unwrap();
        std::fs::write(root.join("Upper.PNG"), b"D").unwrap();
        let count = normalize_lowercase(root).unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            std::fs::read(root.join("customdir/inner.png")).unwrap(),
            b"C"
        );
        assert_eq!(std::fs::read(root.join("upper.png")).unwrap(), b"D");
        if cfg!(windows) {
            // NTFS merged the case-variant writes into one entry at creation
            // time; normalize must leave that entry's content untouched.
            assert_eq!(std::fs::read(root.join("stone.png")).unwrap(), b"B");
        } else {
            assert_eq!(std::fs::read(root.join("stone.png")).unwrap(), b"A");
            assert_eq!(std::fs::read(root.join("Stone.PNG")).unwrap(), b"B");
        }
    }
}
