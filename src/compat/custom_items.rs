use super::assets::{AssetReport, backup};
use crate::packfs::{self as fs, PathExt};
use anyhow::{Context, Result, ensure};
use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Rule {
    pub version: String,
    pub item: String,
    pub name: Option<String>,
    pub inventory_scale: u16,
    pub held_scale: u16,
    pub dropped_scale: u16,
    pub outline: Option<Outline>,
}

impl Default for Rule {
    fn default() -> Self {
        Self {
            version: String::new(),
            item: String::new(),
            name: None,
            inventory_scale: 100,
            held_scale: 100,
            dropped_scale: 100,
            outline: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Outline {
    pub color: [u8; 3],
    pub width: u8,
    #[serde(default)]
    pub rainbow: bool,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub id: String,
    pub english: String,
    pub chinese: String,
    pub translation: String,
    pub model: String,
}

impl Item {
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.split_whitespace().all(|word| {
            self.id.contains(word)
                || self.english.to_lowercase().contains(word)
                || self.chinese.contains(word)
        })
    }
}

#[derive(Deserialize)]
struct Catalog {
    entries: Vec<[String; 5]>,
    profiles: BTreeMap<String, Vec<usize>>,
    versions: BTreeMap<String, String>,
}

static CATALOG: LazyLock<BTreeMap<String, Arc<Vec<Item>>>> = LazyLock::new(|| {
    let data: Catalog = serde_json::from_str(&crate::embedded::text(include_bytes!(concat!(
        env!("OUT_DIR"),
        "/custom-items.deflate"
    ))))
    .expect("物品列表无效");
    let profiles: BTreeMap<_, _> = data
        .profiles
        .into_iter()
        .map(|(id, indexes)| {
            let entries: Vec<Item> = indexes
                .into_iter()
                .map(|index| {
                    let [id, english, chinese, translation, model] = data.entries[index].clone();
                    Item {
                        id,
                        english,
                        chinese,
                        translation,
                        model,
                    }
                })
                .collect();
            (id, Arc::new(entries))
        })
        .collect();
    data.versions
        .into_iter()
        .map(|(id, profile)| (id, profiles[&profile].clone()))
        .collect()
});

pub fn items(version: &str) -> Arc<Vec<Item>> {
    CATALOG
        .get(version)
        .cloned()
        .unwrap_or_else(|| Arc::new(Vec::new()))
}

pub fn validate(rule: &Rule) -> Result<()> {
    ensure!(
        items(&rule.version).iter().any(|item| item.id == rule.item),
        "目标版本没有该物品"
    );
    ensure!(
        [rule.inventory_scale, rule.held_scale, rule.dropped_scale]
            .iter()
            .all(|scale| (25..=400).contains(scale)),
        "物品大小须在 25%–400% 之间"
    );
    ensure!(
        rule.name
            .as_ref()
            .is_none_or(|name| name.chars().count() <= 128 && !name.contains(['\r', '\n', '\0'])),
        "物品名字不能超过 128 字或包含换行"
    );
    ensure!(
        rule.outline
            .as_ref()
            .is_none_or(|outline| (1..=4).contains(&outline.width)),
        "描边宽度须在 1–4 之间"
    );
    Ok(())
}

fn resource(reference: &str) -> Result<(String, String)> {
    let (namespace, name) = reference
        .split_once(':')
        .unwrap_or(("minecraft", reference));
    ensure!(
        !namespace.is_empty()
            && !name.is_empty()
            && namespace
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c))
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-/".contains(c))
            && !name.split('/').any(|part| matches!(part, "" | "." | "..")),
        "物品资源路径无效"
    );
    Ok((namespace.into(), name.into()))
}

fn model_path(root: &Path, reference: &str) -> Result<PathBuf> {
    let (namespace, name) = resource(reference)?;
    Ok(root.join(format!("assets/{namespace}/models/{name}.json")))
}

fn reference_model(format: u32, reference: &str) -> Option<Value> {
    let (namespace, name) = resource(reference).ok()?;
    if namespace != "minecraft" {
        return None;
    }
    if format >= 46 {
        if let Some(value) =
            super::modern_items::model_reference(format, &format!("minecraft:{name}"))
        {
            return Some(value);
        }
    }
    let preferred = era(format);
    if let Some(bytes) =
        default_resource(&format!("assets/minecraft/models/{name}.json"), preferred)
    {
        if let Ok(value) = serde_json::from_slice(&bytes) {
            return Some(value);
        }
    }
    if format >= 4 {
        for candidate in ["middle", "transition", "lastclassic"] {
            if let Some(bytes) =
                default_resource(&format!("assets/minecraft/models/{name}.json"), candidate)
            {
                if let Ok(value) = serde_json::from_slice(&bytes) {
                    return Some(value);
                }
            }
        }
    }
    if format < 4 {
        let legacy: &Value = &LEGACY_MODELS;
        if let Some(value) = legacy["references"]
            .get(reference)
            .or_else(|| legacy["references"].get(&name))
        {
            return Some(value.clone());
        }
    }
    super::modern_items::model_reference(format, &format!("minecraft:{name}"))
        .or_else(|| super::item_transitions::model_reference(format, name.strip_prefix("item/")?))
        .or_else(|| super::item_bindings::reference(name.strip_prefix("item/")?).cloned())
        .or_else(|| super::model_integrity::native_model(format, reference))
}

static LEGACY_MODELS: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../assets/legacy-models.json")).unwrap()
});
static DISPLAYS: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_str(include_str!("../../assets/item-display.json")).unwrap());

fn read_model(root: &Path, format: u32, reference: &str) -> Result<Value> {
    let path = model_path(root, reference)?;
    if path.pack_is_file() {
        let model: Value = serde_json::from_slice(&fs::read(path)?)?;
        ensure!(model.is_object(), "物品模型 {reference} 不是 JSON 对象");
        return Ok(model);
    }
    reference_model(format, reference).with_context(|| format!("未找到物品模型参考 {reference}"))
}

pub(crate) fn cit_model(root: &Path, format: u32, reference: &str) -> Result<Value> {
    let mut model = read_model(root, format, reference)?;
    model["textures"] = inherited(root, format, &model, "textures", &mut BTreeSet::new());
    model["display"] = inherited(root, format, &model, "display", &mut BTreeSet::new());
    let mut current = model.clone();
    let mut seen = BTreeSet::new();
    loop {
        if current.get("elements").is_some() {
            model["elements"] = current["elements"].clone();
            break;
        }
        let Some(parent) = current["parent"].as_str() else {
            break;
        };
        if matches!(
            parent,
            "item/generated"
                | "minecraft:item/generated"
                | "builtin/generated"
                | "minecraft:builtin/generated"
                | "item/handheld"
                | "minecraft:item/handheld"
                | "item/handheld_rod"
                | "minecraft:item/handheld_rod"
        ) {
            model["parent"] = json!(parent);
            break;
        }
        ensure!(
            seen.len() < 48 && seen.insert(parent.to_string()),
            "CIT 模型父链循环或过深"
        );
        current = read_model(root, format, parent)?;
    }
    Ok(model)
}

fn inherited(
    root: &Path,
    format: u32,
    model: &Value,
    field: &str,
    seen: &mut BTreeSet<String>,
) -> Value {
    let mut values = Map::new();
    if let Some(parent) = model["parent"].as_str() {
        if seen.len() < 48 && seen.insert(parent.into()) {
            let parent_value = read_model(root, format, parent).ok();
            if let Some(parent) = parent_value {
                if let Some(map) = inherited(root, format, &parent, field, seen).as_object() {
                    values.extend(map.clone());
                }
            } else if field == "display" {
                let kind = if parent.contains("handheld_rod") {
                    "profile_5"
                } else if parent.contains("handheld") {
                    "profile_2"
                } else {
                    "profile_0"
                };
                let era = if format <= 1 { "legacy" } else { "modern" };
                if let Some(map) = DISPLAYS["profiles"][kind][era].as_object() {
                    values.extend(map.clone());
                }
            }
        }
    }
    if let Some(map) = model[field].as_object() {
        values.extend(map.clone());
    }
    Value::Object(values)
}

fn write_json(root: &Path, path: &Path, value: &Value) -> Result<()> {
    if path.pack_is_file() {
        backup(root, path)?;
    }
    fs::create_dir_all(path.parent().context("缺少资源目录")?)?;
    fs::write_atomic(path, &serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn default_texture(path: &str, legacy: bool) -> Option<Vec<u8>> {
    [
        if legacy { "legacy" } else { "modern" },
        "preflatten",
        "early",
        "middle",
        "transition",
        "lastclassic",
    ]
    .iter()
    .find_map(|era| default_resource(path, era))
}

fn era(format: u32) -> &'static str {
    if format <= 1 {
        "legacy"
    } else if format < 4 {
        "preflatten"
    } else if format == 4 {
        "early"
    } else if format == 13 {
        "pottery"
    } else if format == 42 {
        "lastclassic"
    } else if format < 46 {
        "middle"
    } else {
        "modern"
    }
}

struct Resources {
    index: BTreeMap<String, [usize; 2]>,
    objects: Vec<u8>,
}

static RESOURCES: LazyLock<Resources> = LazyLock::new(|| {
    let mut archive = zip::ZipArchive::new(Cursor::new(
        include_bytes!("../../assets/custom-item-resources.zip").as_slice(),
    ))
    .expect("物品参考资源无效");
    let mut bytes = Vec::new();
    archive
        .by_name("index.json")
        .expect("缺少物品资源索引")
        .read_to_end(&mut bytes)
        .expect("物品资源索引不可读");
    let index = serde_json::from_slice(&bytes).expect("物品资源索引无效");
    let mut objects = Vec::new();
    archive
        .by_name("objects.bin")
        .expect("缺少物品参考内容")
        .read_to_end(&mut objects)
        .expect("物品参考内容不可读");
    Resources { index, objects }
});

fn default_resource(path: &str, prefix: &str) -> Option<Vec<u8>> {
    let [start, length] = RESOURCES.index.get(&format!("{prefix}/{path}"))?;
    Some(
        RESOURCES
            .objects
            .get(*start..start.checked_add(*length)?)?
            .to_vec(),
    )
}

fn texture_bytes(root: &Path, reference: &str, format: u32) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
    let (namespace, name) = resource(reference)?;
    let relative = format!("assets/{namespace}/textures/{name}.png");
    let path = root.join(&relative);
    let era = era(format);
    let find = |name: &str| {
        [
            era,
            "middle",
            "transition",
            "lastclassic",
            "modern",
            "preflatten",
            "legacy",
        ]
        .iter()
        .find_map(|era| default_resource(name, era))
    };
    let bytes = if path.pack_is_file() {
        fs::read(&path)?
    } else {
        find(&relative).context("未找到该物品贴图")?
    };
    let metadata = if path.with_extension("png.mcmeta").pack_is_file() {
        Some(fs::read(path.with_extension("png.mcmeta"))?)
    } else if !path.pack_is_file() {
        find(&(relative.clone() + ".mcmeta"))
    } else {
        None
    };
    Ok((bytes, metadata))
}

pub fn outlined(image: &RgbaImage, outline: &Outline) -> RgbaImage {
    let mut result = image.clone();
    let radius = i32::from(outline.width).min(4);
    let frame_height =
        if image.height() > image.width() && image.height().is_multiple_of(image.width()) {
            image.width()
        } else {
            image.height()
        };
    if image.pixels().all(|pixel| pixel[3] == 255) {
        let width = u32::from(outline.width);
        for (x, y, pixel) in result.enumerate_pixels_mut() {
            if x < width
                || x.saturating_add(width) >= image.width()
                || y % frame_height < width
                || (y % frame_height).saturating_add(width) >= frame_height
            {
                *pixel = Rgba([outline.color[0], outline.color[1], outline.color[2], 255]);
            }
        }
        return result;
    }
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] != 0 {
            continue;
        }
        let top = y / frame_height * frame_height;
        let visible = (-radius..=radius).any(|dy| {
            (-radius..=radius).any(|dx| {
                let px = i64::from(x) + i64::from(dx);
                let py = i64::from(y) + i64::from(dy);
                dx * dx + dy * dy <= radius * radius
                    && px >= 0
                    && px < i64::from(image.width())
                    && py >= i64::from(top)
                    && py < i64::from(top + frame_height)
                    && image.get_pixel(px as u32, py as u32)[3] > 16
            })
        });
        if visible {
            result.put_pixel(
                x,
                y,
                Rgba([outline.color[0], outline.color[1], outline.color[2], 255]),
            );
        }
    }
    result
}

fn outline_image(
    image: &RgbaImage,
    metadata: Option<Vec<u8>>,
    outline: &Outline,
) -> Result<(RgbaImage, Option<Vec<u8>>)> {
    if outline.rainbow {
        let (image, metadata) =
            super::rainbow::animate(image, metadata.as_deref(), |image, color| {
                outlined(
                    image,
                    &Outline {
                        color,
                        width: outline.width,
                        rainbow: false,
                    },
                )
            })?;
        Ok((image, Some(metadata)))
    } else {
        Ok((outlined(image, outline), metadata))
    }
}

fn baseline(root: &Path, version: &str, path: &Path) -> Result<()> {
    let relative = path.strip_prefix(root)?;
    let saved = root
        .join("fastpack/custom-item-originals")
        .join(version)
        .join(relative);
    let missing = saved.with_extension("json.missing");
    if saved.pack_is_file() {
        fs::create_dir_all(path.parent().unwrap())?;
        fs::copy(&saved, path)?;
    } else if missing.pack_is_file() {
        if path.pack_is_file() {
            fs::remove_file(path)?;
        }
    } else {
        fs::create_dir_all(saved.parent().unwrap())?;
        if path.pack_is_file() {
            fs::copy(path, &saved)?;
        } else {
            fs::write(&missing, b"")?;
        }
    }
    Ok(())
}

fn rename(root: &Path, format: u32, item: &Item, name: &str) -> Result<()> {
    for locale in ["zh_cn", "en_us"] {
        let path = root.join(format!("assets/minecraft/lang/{locale}.json"));
        if format >= 4 {
            let mut value = if path.pack_is_file() {
                serde_json::from_slice(&fs::read(&path)?)?
            } else {
                json!({})
            };
            value
                .as_object_mut()
                .context("语言文件不是对象")?
                .insert(item.translation.clone(), json!(name));
            write_json(root, &path, &value)?;
        } else {
            for code in [
                locale.to_string(),
                if locale == "zh_cn" {
                    "zh_CN".into()
                } else {
                    "en_US".into()
                },
            ] {
                let path = root.join(format!("assets/minecraft/lang/{code}.lang"));
                let text = if path.pack_is_file() {
                    String::from_utf8_lossy(&fs::read(&path)?).into_owned()
                } else {
                    String::new()
                };
                if path.pack_is_file() {
                    backup(root, &path)?;
                }
                let mut lines: Vec<_> = text
                    .lines()
                    .filter(|line| {
                        line.split_once('=')
                            .is_none_or(|(key, _)| key != item.translation)
                    })
                    .map(str::to_string)
                    .collect();
                lines.push(format!("{}={name}", item.translation));
                fs::create_dir_all(path.parent().unwrap())?;
                fs::write_atomic(&path, &(lines.join("\n") + "\n").into_bytes())?;
            }
        }
    }
    Ok(())
}

fn scale_model(root: &Path, format: u32, model: &mut Value, rule: &Rule) {
    let mut display = inherited(root, format, model, "display", &mut BTreeSet::new());
    let original = display.clone();
    let contexts: &[(&str, u16)] = if format <= 1 {
        &[
            ("gui", rule.inventory_scale),
            ("firstperson", rule.held_scale),
            ("thirdperson", rule.held_scale),
            ("ground", rule.dropped_scale),
        ]
    } else {
        &[
            ("gui", rule.inventory_scale),
            ("firstperson_righthand", rule.held_scale),
            ("firstperson_lefthand", rule.held_scale),
            ("thirdperson_righthand", rule.held_scale),
            ("thirdperson_lefthand", rule.held_scale),
            ("ground", rule.dropped_scale),
        ]
    };
    for &(context, percent) in contexts {
        if percent == 100 {
            continue;
        }
        if display.get(context).is_none() && context.contains("lefthand") {
            if let Some(right) = original
                .get(context.replace("lefthand", "righthand"))
                .cloned()
            {
                display[context] = right;
            }
        }
        let transform = display
            .as_object_mut()
            .unwrap()
            .entry(context)
            .or_insert_with(|| json!({}));
        let old = transform["scale"]
            .as_array()
            .map(|scale| {
                scale
                    .iter()
                    .map(|value| value.as_f64().unwrap_or(1.0))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![1.0; 3]);
        transform["scale"] = json!(
            old.iter()
                .map(|value| (value * f64::from(percent) / 100.0).clamp(0.0, 4.0))
                .collect::<Vec<_>>()
        );
    }
    model["display"] = display;
}

struct Editor<'a> {
    root: &'a Path,
    format: u32,
    rule: &'a Rule,
    prefix: String,
    models: BTreeMap<String, String>,
    textures: BTreeMap<String, String>,
}

impl Editor<'_> {
    fn model(&mut self, reference: &str) -> Result<String> {
        let (namespace, name) = resource(reference)?;
        let reference = format!("{namespace}:{name}");
        if let Some(private) = self.models.get(&reference) {
            return Ok(private.clone());
        }
        ensure!(self.models.len() < 256, "物品模型引用过多");
        let private = format!(
            "minecraft:item/fastpack_custom/{}/m{}",
            self.prefix,
            self.models.len()
        );
        self.models.insert(reference.clone(), private.clone());
        let mut model = read_model(self.root, self.format, &reference)?;
        ensure!(
            !model
                .get("slots")
                .is_some_and(|_| model.get("parent").is_none()),
            "该物品缺少可保留的模型几何，未修改大小或描边"
        );
        if [
            self.rule.inventory_scale,
            self.rule.held_scale,
            self.rule.dropped_scale,
        ]
        .iter()
        .any(|value| *value != 100)
        {
            scale_model(self.root, self.format, &mut model, self.rule);
        }
        if self.rule.outline.is_some() {
            let textures = inherited(
                self.root,
                self.format,
                &model,
                "textures",
                &mut BTreeSet::new(),
            );
            let mut values = textures.as_object().cloned().unwrap_or_default();
            for value in values.values_mut() {
                if let Some(reference) = value
                    .as_str()
                    .filter(|reference| !reference.starts_with('#'))
                {
                    *value = json!(self.texture(reference)?);
                }
            }
            model["textures"] = Value::Object(values);
        }
        if let Some(overrides) = model.get_mut("overrides").and_then(Value::as_array_mut) {
            for entry in overrides {
                if let Some(reference) = entry["model"].as_str() {
                    entry["model"] = json!(self.model(reference)?);
                }
            }
        }
        if let Some(object) = model.as_object_mut() {
            object.remove("slots");
        }
        write_json(self.root, &model_path(self.root, &private)?, &model)?;
        Ok(private)
    }
    fn texture(&mut self, reference: &str) -> Result<String> {
        if let Some(private) = self.textures.get(reference) {
            return Ok(private.clone());
        }
        let private = format!(
            "minecraft:{}/fastpack_custom/{}/t{}",
            if self.format < 4 { "items" } else { "item" },
            self.prefix,
            self.textures.len()
        );
        let (bytes, metadata) = texture_bytes(self.root, reference, self.format)?;
        let image = image::load_from_memory(&bytes)?.to_rgba8();
        let outline = self.rule.outline.as_ref().unwrap();
        let (image, metadata) = outline_image(&image, metadata, outline)?;
        let name = private.strip_prefix("minecraft:").unwrap();
        let path = self
            .root
            .join(format!("assets/minecraft/textures/{name}.png"));
        fs::create_dir_all(path.parent().unwrap())?;
        fs::save_image(&image, &path)?;
        if let Some(metadata) = metadata {
            fs::write(path.with_extension("png.mcmeta"), metadata)?;
        } else if path.with_extension("png.mcmeta").pack_is_file() {
            fs::remove_file(path.with_extension("png.mcmeta"))?;
        }
        self.textures.insert(reference.into(), private.clone());
        Ok(private)
    }
    fn definition(&mut self, node: &mut Value) -> Result<()> {
        match node {
            Value::Object(values) => {
                for (key, value) in values {
                    if matches!(key.as_str(), "model" | "base") && value.is_string() {
                        *value = json!(self.model(value.as_str().unwrap())?);
                    } else {
                        self.definition(value)?;
                    }
                }
            }
            Value::Array(values) => {
                for value in values {
                    self.definition(value)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

pub fn apply(root: &Path, version: &str, rules: &[Rule], report: &mut AssetReport) -> Result<()> {
    let format = crate::pack_format_of(version)?;
    let catalog = items(version);
    for rule in rules.iter().filter(|rule| rule.version == version) {
        validate(rule)?;
        let item = catalog.iter().find(|item| item.id == rule.item).unwrap();
        let main_path = if format >= 46 {
            root.join(format!("assets/minecraft/items/{}.json", item.model))
        } else {
            model_path(root, &format!("minecraft:item/{}", item.model))?
        };
        if version != "1.7.10" {
            baseline(root, version, &main_path)?;
        }
        if let Some(name) = &rule.name {
            rename(root, format, item, name)?;
        }
        let geometry = [rule.inventory_scale, rule.held_scale, rule.dropped_scale]
            .iter()
            .any(|scale| *scale != 100)
            || rule.outline.is_some();
        if geometry {
            if version == "1.7.10" {
                if let Some(outline) = &rule.outline {
                    let name = crate::legacy_texture_name(&item.model, "item")
                        .unwrap_or_else(|| item.model.clone());
                    let reference = format!("minecraft:items/{name}");
                    let path = root.join(format!("assets/minecraft/textures/items/{name}.png"));
                    baseline(root, version, &path)?;
                    baseline(root, version, &path.with_extension("png.mcmeta"))?;
                    match texture_bytes(root, &reference, format).and_then(|(bytes, metadata)| {
                        outline_image(
                            &image::load_from_memory(&bytes)?.to_rgba8(),
                            metadata,
                            outline,
                        )
                    }) {
                        Ok((image, metadata)) => {
                            let path =
                                root.join(format!("assets/minecraft/textures/items/{name}.png"));
                            super::assets::save(root, &path, &image)?;
                            if let Some(metadata) = metadata {
                                fs::write_atomic(&path.with_extension("png.mcmeta"), &metadata)?;
                            }
                        }
                        Err(error) => report
                            .warnings
                            .push(format!("{} 描边未应用：{error:#}", item.chinese)),
                    }
                }
                if [rule.inventory_scale, rule.held_scale, rule.dropped_scale]
                    .iter()
                    .any(|scale| *scale != 100)
                {
                    report
                        .warnings
                        .push("1.7.10 的原版物品渲染不读取资源包模型，大小参数未应用".into());
                }
            } else {
                let mut editor = Editor {
                    root,
                    format,
                    rule,
                    prefix: item.model.replace('/', "_"),
                    models: BTreeMap::new(),
                    textures: BTreeMap::new(),
                };
                let result = (|| -> Result<()> {
                    if format >= 46 {
                        let path = root.join(format!("assets/minecraft/items/{}.json", item.model));
                        let mut definition = if path.pack_is_file() {
                            serde_json::from_slice(&fs::read(&path)?)?
                        } else {
                            super::modern_items::definition_reference(format,&item.model).or_else(||default_resource(&format!("assets/minecraft/items/{}.json",item.model),era(format)).and_then(|bytes|serde_json::from_slice(&bytes).ok())).or_else(|| {
                            ["item", "block"].iter().find_map(|folder| {
                                let reference = format!("minecraft:{folder}/{}",item.model);
                                read_model(root,format,&reference).ok().map(|_| json!({"model":{"type":"minecraft:model","model":reference}}))
                            })
                        }).context("缺少目标版本物品定义")?
                        };
                        editor.definition(&mut definition)?;
                        write_json(root, &path, &definition)?;
                    } else {
                        let original = format!("minecraft:item/{}", item.model);
                        let private = editor.model(&original)?;
                        let model = read_model(root, format, &private)?;
                        write_json(root, &model_path(root, &original)?, &model)?;
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    report
                        .warnings
                        .push(format!("{} 的大小/描边未全部应用：{error:#}", item.chinese));
                }
            }
        }
        report
            .converted
            .push(format!("物品自定义：{} ({})", item.chinese, item.id));
    }
    Ok(())
}

pub fn preview(source: &Path, item: &Item) -> Result<RgbaImage> {
    let name = item.model.as_str();
    let old = crate::legacy_texture_name(name, "item").unwrap_or_else(|| name.into());
    let old_block = crate::legacy_texture_name(name, "block").unwrap_or_else(|| name.into());
    let candidates = [
        format!("assets/minecraft/textures/item/{name}.png"),
        format!("assets/minecraft/textures/items/{old}.png"),
        format!("assets/minecraft/textures/block/{name}.png"),
        format!("assets/minecraft/textures/blocks/{old_block}.png"),
    ];
    if source.is_dir() {
        let root = crate::converter::find_pack_root(source)?;
        for name in &candidates {
            let path = root.join(name);
            if path.is_file() {
                return Ok(image::open(path)?.to_rgba8());
            }
        }
    } else if source.is_file() {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(source)?)?;
        let names: Vec<_> = zip.file_names().map(str::to_string).collect();
        for candidate in &candidates {
            if let Some(name) = names.iter().find(|name| {
                name.to_lowercase() == *candidate
                    || name.to_lowercase().ends_with(&format!("/{candidate}"))
            }) {
                let mut entry = zip.by_name(name)?;
                ensure!(entry.size() <= 32 * 1024 * 1024, "物品预览文件过大");
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes)?;
                return Ok(image::load_from_memory(&bytes)?.to_rgba8());
            }
        }
    }
    for name in &candidates {
        if let Some(bytes) =
            default_texture(name, name.contains("/items/") || name.contains("/blocks/"))
        {
            return Ok(image::load_from_memory(&bytes)?.to_rgba8());
        }
    }
    anyhow::bail!("该物品使用特殊渲染，暂无平面预览")
}

#[cfg(test)]
mod tests;
