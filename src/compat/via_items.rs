use super::assets::{AssetReport, backup};
use crate::packfs as fs;
use crate::packfs::PathExt;
use anyhow::{ensure, Result};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

mod images {
    include!("../../assets/via-items/images.rs");
}

#[derive(Clone, Deserialize)]
struct Rule {
    item: String,
    carrier: String,
    cmd: u32,
    key: String,
    id: u32,
}

#[derive(Deserialize)]
struct Profile {
    version: String,
    rules: Vec<Rule>,
    carriers: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
struct Item {
    model: String,
}

#[derive(Deserialize)]
struct Catalog {
    profiles: BTreeMap<u32, Profile>,
    items: BTreeMap<String, Item>,
    models: BTreeMap<String, Value>,
    languages: BTreeMap<String, BTreeMap<String, Value>>,
}

static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    serde_json::from_str(&crate::embedded::text(include_bytes!(concat!(
        env!("OUT_DIR"),
        "/via-catalog.deflate"
    ))))
    .expect("协议物品资源表无效")
});

fn resource(reference: &str) -> Option<(&str, &str)> {
    let (namespace, name) = reference
        .split_once(':')
        .unwrap_or(("minecraft", reference));
    if namespace.is_empty()
        || name.is_empty()
        || !namespace
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c))
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-/".contains(c))
        || name
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return None;
    }
    Some((namespace, name))
}

fn model_path(root: &Path, reference: &str) -> Option<PathBuf> {
    let (namespace, name) = resource(reference)?;
    Some(root.join(format!("assets/{namespace}/models/{name}.json")))
}

fn write(root: &Path, path: &Path, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    if path.pack_is_file() {
        if fs::read(path)? == bytes {
            return Ok(());
        }
        backup(root, path)?;
    }
    fs::create_dir_all(path.parent().unwrap())?;
    let temporary = path.with_extension("json.fastpack-new");
    fs::write(&temporary, bytes)?;
    if path.pack_is_file() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)?;
    Ok(())
}

struct Installer<'a> {
    root: &'a Path,
    target: u32,
    installed: BTreeMap<String, String>,
}

impl Installer<'_> {
    fn resolved(&self, reference: &str, seen: &mut BTreeSet<String>) -> Value {
        if seen.len() >= 64 || !seen.insert(reference.into()) {
            return json!({});
        }
        let Some((namespace, name)) = resource(reference) else {
            return json!({});
        };
        let value = model_path(self.root, reference)
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .or_else(|| {
                (namespace == "minecraft")
                    .then(|| CATALOG.models.get(name).cloned())
                    .flatten()
            });
        let Some(value) = value else {
            return json!({});
        };
        let mut inherited = value
            .get("parent")
            .and_then(Value::as_str)
            .map(|parent| self.resolved(parent, seen))
            .unwrap_or_else(|| json!({}));
        for field in ["display", "textures"] {
            if let Some(values) = value.get(field).and_then(Value::as_object) {
                if !inherited.get(field).is_some_and(Value::is_object) {
                    inherited[field] = json!({});
                }
                inherited[field]
                    .as_object_mut()
                    .unwrap()
                    .extend(values.clone());
            }
        }
        for field in ["elements", "ambientocclusion", "gui_light"] {
            if let Some(value) = value.get(field) {
                inherited[field] = value.clone();
            }
        }
        seen.remove(reference);
        inherited
    }

    fn untinted(&self, reference: &str) -> Result<()> {
        let mut model = self.resolved(reference, &mut BTreeSet::new());
        if let Some(elements) = model.get_mut("elements").and_then(Value::as_array_mut) {
            for element in elements {
                if let Some(faces) = element.get_mut("faces").and_then(Value::as_object_mut) {
                    for face in faces.values_mut() {
                        if let Some(face) = face.as_object_mut() {
                            face.remove("tintindex");
                        }
                    }
                }
            }
        } else {
            let Some(mut texture) = model
                .get("textures")
                .and_then(|v| v.get("layer0"))
                .and_then(Value::as_str)
            else {
                return Ok(());
            };
            let mut seen = BTreeSet::new();
            while let Some(key) = texture.strip_prefix('#') {
                if !seen.insert(key) {
                    return Ok(());
                }
                let Some(value) = model["textures"].get(key).and_then(Value::as_str) else {
                    return Ok(());
                };
                texture = value;
            }
            let Some((namespace, name)) = resource(texture) else {
                return Ok(());
            };
            let path = self
                .root
                .join(format!("assets/{namespace}/textures/{name}.png"));
            let Ok(image) = super::assets::open(&path) else {
                return Ok(());
            };
            let size = image.width().min(image.height()).min(64);
            if size == 0 {
                return Ok(());
            }
            let first = image::imageops::crop_imm(
                &image,
                0,
                0,
                image.width(),
                image.width().min(image.height()),
            )
            .to_image();
            let image =
                image::imageops::resize(&first, size, size, image::imageops::FilterType::Nearest);
            let mut elements = Vec::new();
            for (x, y, pixel) in image.enumerate_pixels() {
                if pixel[3] == 0 {
                    continue;
                }
                let u = 16.0 * f64::from(x) / f64::from(size);
                let v = 16.0 * f64::from(y) / f64::from(size);
                let step = 16.0 / f64::from(size);
                let face = json!({"uv":[u,v,u+step,v+step],"texture":"#layer0"});
                let mut faces = Map::from_iter([
                    ("north".into(), face.clone()),
                    ("south".into(), face.clone()),
                ]);
                for (direction, dx, dy) in [
                    ("west", -1, 0),
                    ("east", 1, 0),
                    ("up", 0, -1),
                    ("down", 0, 1),
                ] {
                    let (px, py) = (x as i32 + dx, y as i32 + dy);
                    if px < 0
                        || py < 0
                        || px >= size as i32
                        || py >= size as i32
                        || image.get_pixel(px as u32, py as u32)[3] == 0
                    {
                        faces.insert(direction.into(), face.clone());
                    }
                }
                elements.push(
                    json!({"from":[u,16.0-v-step,7.5],"to":[u+step,16.0-v,8.5],"faces":faces}),
                );
            }
            model["elements"] = json!(elements);
        }
        write(
            self.root,
            &model_path(self.root, reference).unwrap(),
            &model,
        )
    }

    fn texture(&self, reference: &str) -> Result<()> {
        let Some((namespace, name)) = resource(reference) else {
            return Ok(());
        };
        if namespace != "minecraft" {
            return Ok(());
        }
        let output = self
            .root
            .join(format!("assets/minecraft/textures/{name}.png"));
        if output.pack_is_file() || super::model_integrity::native_texture(self.target, reference) {
            return Ok(());
        }
        let Some(bytes) = images::get(&format!("{name}.png")) else {
            return Ok(());
        };
        fs::create_dir_all(output.parent().unwrap())?;
        fs::write(&output, bytes)?;
        if let Some(metadata) = images::get(&format!("{name}.png.mcmeta")) {
            let path = output.with_extension("png.mcmeta");
            if !path.pack_is_file() {
                fs::write(path, metadata)?;
            }
        }
        Ok(())
    }

    fn model(&mut self, reference: &str) -> Result<Option<String>> {
        let Some((namespace, name)) = resource(reference) else {
            return Ok(None);
        };
        if name.starts_with("builtin/") {
            return Ok(None);
        }
        let id = format!("{namespace}:{name}");
        if let Some(output) = self.installed.get(&id) {
            return Ok(Some(output.clone()));
        }
        if self.installed.len() >= 4096 {
            return Ok(None);
        }
        let source = model_path(self.root, &id).unwrap();
        let provided = source.pack_is_file();
        if !provided
            && matches!(
                name,
                "item/generated" | "item/handheld" | "item/handheld_rod" | "block/block"
            )
        {
            return Ok(None);
        }
        let value = if provided {
            fs::read(&source)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        } else {
            CATALOG
                .models
                .get(name)
                .cloned()
                .filter(|_| namespace == "minecraft")
        };
        let Some(mut value) = value else {
            return Ok(None);
        };
        if value.get("fastpack_via_bridge") == Some(&json!(1))
            && namespace == "minecraft"
            && let Some(carrier) = name.strip_prefix("item/")
        {
            let saved = self
                .root
                .join(format!("fastpack/via-bridge/base/{carrier}.json"));
            if let Some(original) = fs::read(saved)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            {
                value = original;
            }
        }
        let output = format!("fastpack:via/{namespace}/{name}");
        self.installed.insert(id, output.clone());
        let aliases: BTreeSet<String> = value
            .get("textures")
            .and_then(Value::as_object)
            .map(|values| values.keys().cloned().collect())
            .unwrap_or_default();
        if let Some(elements) = value.get_mut("elements").and_then(Value::as_array_mut) {
            for element in elements {
                if let Some(faces) = element.get_mut("faces").and_then(Value::as_object_mut) {
                    for face in faces.values_mut() {
                        if let Some(texture) = face.get_mut("texture")
                            && let Some(name) = texture.as_str()
                            && aliases.contains(name)
                        {
                            *texture = json!(format!("#{name}"));
                        }
                    }
                }
            }
        }
        self.references(&mut value)?;
        let path = model_path(self.root, &output).unwrap();
        write(self.root, &path, &value)?;
        Ok(Some(output))
    }

    fn references(&mut self, value: &mut Value) -> Result<()> {
        match value {
            Value::Object(values) => {
                for (key, value) in values {
                    if matches!(key.as_str(), "parent" | "model")
                        && let Some(reference) = value.as_str()
                    {
                        if let Some(output) = self.model(reference)? {
                            *value = json!(output);
                        }
                    } else if key == "textures"
                        && let Some(textures) = value.as_object()
                    {
                        for reference in textures.values().filter_map(Value::as_str) {
                            self.texture(reference)?;
                        }
                    } else if key == "texture"
                        && let Some(reference) = value.as_str()
                    {
                        self.texture(reference)?;
                    } else {
                        self.references(value)?;
                    }
                }
            }
            Value::Array(values) => {
                for value in values {
                    self.references(value)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn model_data(value: &Value) -> f64 {
    value
        .get("predicate")
        .and_then(|p| p.get("custom_model_data"))
        .and_then(Value::as_f64)
        .unwrap_or(f64::NEG_INFINITY)
}

fn numeric_model(
    root: &Path,
    carrier: &str,
    original: &Value,
    rules: &[(Rule, String)],
    report: &mut AssetReport,
) -> Result<()> {
    let original_overrides = original
        .get("overrides")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let usable:Vec<_>=rules.iter().filter(|(rule,_)| {
        let collision=original_overrides.iter().any(|entry|model_data(entry)==f64::from(rule.cmd));
        if collision {report.warnings.push(format!("协议物品 {}：原包已使用 CustomModelData={}，保留原包数字规则；仍生成按协议标签识别的 CIT/Chime 规则",rule.item,rule.cmd));}
        !collision
    }).collect();
    if usable.is_empty() {
        return Ok(());
    }
    let base_reference = format!("fastpack:via/base/{carrier}");
    ensure!(original.is_object(), "CIT 载体模型 {carrier} 不是 JSON 对象");
    let mut base = original.clone();
    base.as_object_mut().unwrap().remove("overrides");
    write(root, &model_path(root, &base_reference).unwrap(), &base)?;
    let mut thresholds: Vec<f64> = usable
        .iter()
        .flat_map(|(rule, _)| [f64::from(rule.cmd), f64::from(rule.cmd) + 1.0])
        .collect();
    let first = thresholds.iter().copied().min_by(f64::total_cmp).unwrap();
    thresholds.extend(
        original_overrides
            .iter()
            .map(model_data)
            .filter(|value| value.is_finite() && *value >= first),
    );
    thresholds.sort_by(f64::total_cmp);
    thresholds.dedup();
    let mut overrides = original_overrides.clone();
    for threshold in thresholds {
        overrides.push(json!({"predicate":{"custom_model_data":threshold},"model":base_reference}));
        for entry in &original_overrides {
            if model_data(entry) > threshold {
                continue;
            }
            let Some(mut predicate) = entry.get("predicate").and_then(Value::as_object).cloned()
            else {
                continue;
            };
            predicate.insert("custom_model_data".into(), json!(threshold));
            let mut restored = entry.clone();
            restored["predicate"] = Value::Object(predicate);
            overrides.push(restored);
        }
        if let Some((_, model)) = usable
            .iter()
            .find(|(rule, _)| f64::from(rule.cmd) == threshold)
        {
            overrides.push(json!({"predicate":{"custom_model_data":threshold},"model":model}));
        }
    }
    let mut value = original.clone();
    value["overrides"] = json!(overrides);
    value["fastpack_via_bridge"] = json!(1);
    write(
        root,
        &root.join(format!("assets/minecraft/models/item/{carrier}.json")),
        &value,
    )
}

fn tags(root: &Path, carrier: &str, rules: &[(Rule, String)]) -> Result<()> {
    let chime = root.join(format!("assets/minecraft/overrides/item/{carrier}.json"));
    let mut definition = if chime.pack_is_file() {
        let Ok(value) = serde_json::from_slice::<Value>(&fs::read(&chime)?) else {
            return Ok(());
        };
        value
    } else {
        json!({"overrides":[]})
    };
    if !definition.get("overrides").is_some_and(Value::is_array) {
        return Ok(());
    }
    for (rule, model) in rules {
        let key = rule.key.clone();
        let suffix = key
            .strip_prefix("VB|Protocol")
            .and_then(|key| key.strip_suffix("|id"))
            .and_then(|key| key.split_once("To"));
        let keys = if let Some((new, old)) = suffix {
            vec![key.clone(), format!("VB|Protocol{old}To{new}|id")]
        } else {
            vec![key]
        };
        for (index, key) in keys.into_iter().enumerate() {
            let mut nbt = Map::new();
            nbt.insert(key.clone(), json!(rule.id));
            let entry = json!({"predicate":{"nbt":nbt},"model":model});
            let entries = definition["overrides"].as_array_mut().unwrap();
            if !entries
                .iter()
                .any(|existing| existing.get("predicate") == entry.get("predicate"))
            {
                entries.push(entry);
            }
            let path = root.join(format!(
                "assets/minecraft/optifine/cit/fastpack_via/{}_{}.properties",
                rule.item, index
            ));
            let properties = BTreeMap::from([
                ("type".into(), "item".into()),
                ("matchItems".into(), format!("minecraft:{carrier}")),
                ("model".into(), model.clone()),
                (format!("nbt.{key}"), rule.id.to_string()),
            ]);
            fs::create_dir_all(path.parent().unwrap())?;
            let text = super::properties::write(&properties);
            if !path.pack_is_file() || fs::read(&path)? != text.as_bytes() {
                if path.pack_is_file() {
                    backup(root, &path)?;
                }
                fs::write(path, text)?;
            }
        }
    }
    write(root, &chime, &definition)
}

fn modern(root: &Path, target: u32, carrier: &str, rules: &[(Rule, String)]) -> Result<()> {
    let path = root.join(format!("assets/minecraft/items/{carrier}.json"));
    let saved = root.join(format!("fastpack/via-bridge/definitions/{carrier}.json"));
    let entry = format!("minecraft:item/{carrier}");
    let provided = model_path(root, &entry).is_some_and(|path| path.pack_is_file());
    let native = super::modern_items::definition_reference(target, carrier)
        .unwrap_or_else(|| json!({"model":{"type":"minecraft:model","model":entry}}));
    let default = if provided {
        json!({"model":{"type":"minecraft:model","model":entry}})
    } else {
        native.clone()
    };
    let mut value = if saved.pack_is_file() {
        serde_json::from_slice::<Value>(&fs::read(&saved)?)?
    } else if path.pack_is_file() {
        serde_json::from_slice::<Value>(&fs::read(&path)?)?
    } else {
        default
    };
    ensure!(value.is_object(), "CIT 物品定义 {carrier} 不是 JSON 对象");
    fn repair_default(value: &mut Value, entry: &str, native: &Value) {
        if value["type"]
            .as_str()
            .is_some_and(|name| name.trim_start_matches("minecraft:") == "model")
            && value["model"].as_str().is_some_and(|name| {
                name.trim_start_matches("minecraft:") == entry.trim_start_matches("minecraft:")
            })
        {
            *value = native.clone();
        } else if let Some(values) = value.as_object_mut() {
            for value in values.values_mut() {
                repair_default(value, entry, native);
            }
        } else if let Some(values) = value.as_array_mut() {
            for value in values {
                repair_default(value, entry, native);
            }
        }
    }
    if !provided {
        repair_default(&mut value["model"], &entry, &native["model"]);
    }
    if !saved.pack_is_file() {
        fs::create_dir_all(saved.parent().unwrap())?;
        fs::write(&saved, serde_json::to_vec_pretty(&value)?)?;
    }
    let fallback = value
        .get("model")
        .cloned()
        .unwrap_or_else(|| native["model"].clone());
    let mut entries = Vec::new();
    for (rule, model) in rules {
        entries
            .push(json!({"threshold":rule.cmd,"model":{"type":"minecraft:model","model":model}}));
        entries.push(json!({"threshold":rule.cmd+1,"model":fallback}));
    }
    entries.sort_by(|a, b| a["threshold"].as_u64().cmp(&b["threshold"].as_u64()));
    for (rule, model) in rules {
        if let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry["threshold"] == rule.cmd)
        {
            entry["model"] = json!({"type":"minecraft:model","model":model});
        }
    }
    entries.dedup_by(|a, b| a["threshold"] == b["threshold"]);
    let numeric = json!({"type":"minecraft:range_dispatch","property":"minecraft:custom_model_data","index":0,"entries":entries,"fallback":fallback});
    let cases:Vec<_>=rules.iter().map(|(rule,model)|json!({"when":format!("minecraft:{}",rule.item),"model":{"type":"minecraft:model","model":model}})).collect();
    value["model"] = json!({"type":"minecraft:select","property":"minecraft:custom_model_data","index":0,"cases":cases,"fallback":numeric});
    value["fastpack_via_bridge"] = json!(1);
    write(root, &path, &value)
}

#[cfg(test)]
mod modern_tests {
    use super::*;

    #[test]
    fn carrier_fallback_uses_target_definition_and_repairs_previous_generic_entries() {
        let work = tempfile::tempdir().unwrap();
        for name in ["end_stone", "granite", "diorite", "smooth_sandstone"] {
            let saved = work
                .path()
                .join(format!("fastpack/via-bridge/definitions/{name}.json"));
            fs::create_dir_all(saved.parent().unwrap()).unwrap();
            fs::write(&saved, serde_json::to_vec(&json!({"model":{"type":"minecraft:model","model":format!("minecraft:item/{name}")}})).unwrap()).unwrap();
            modern(work.path(), 69, name, &[]).unwrap();
            let value: Value = serde_json::from_slice(
                &fs::read(
                    work.path()
                        .join(format!("assets/minecraft/items/{name}.json")),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(
                value["model"]["fallback"]["fallback"]["model"],
                format!("minecraft:block/{name}")
            );
        }
    }

    #[test]
    fn custom_carrier_model_stays_bound_to_the_author_model() {
        let work = tempfile::tempdir().unwrap();
        let model = work
            .path()
            .join("assets/minecraft/models/item/end_stone.json");
        fs::create_dir_all(model.parent().unwrap()).unwrap();
        fs::write(
            &model,
            br#"{"parent":"minecraft:block/end_stone","display":{"gui":{"scale":[0.8,0.8,0.8]}}}"#,
        )
        .unwrap();
        modern(work.path(), 69, "end_stone", &[]).unwrap();
        let value: Value = serde_json::from_slice(
            &fs::read(work.path().join("assets/minecraft/items/end_stone.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            value["model"]["fallback"]["fallback"]["model"],
            "minecraft:item/end_stone"
        );
    }
}

pub(super) fn disable(root: &Path, report: &mut AssetReport) -> Result<()> {
    let metadata = root.join("fastpack/via-bridge/manifest.json");
    if !metadata.pack_is_file() {
        return Ok(());
    }
    let mut manifest: Value = serde_json::from_slice(&fs::read(&metadata)?)?;
    let mut carriers = BTreeSet::new();
    for item in manifest["items"].as_array().into_iter().flatten() {
        if let Some(carrier) = item["carrier"].as_str().filter(|name| {
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        }) {
            carriers.insert(carrier.to_string());
        }
    }
    for carrier in carriers {
        let path = root.join(format!("assets/minecraft/models/item/{carrier}.json"));
        let saved = root.join(format!("fastpack/via-bridge/base/{carrier}.json"));
        if let Ok(current) = fs::read(&path).and_then(|bytes| {
            serde_json::from_slice::<Value>(&bytes).map_err(std::io::Error::other)
        }) {
            if current["fastpack_via_bridge"] == json!(1) && saved.pack_is_file() {
                write(root, &path, &serde_json::from_slice(&fs::read(saved)?)?)?;
            }
        }
        let chime = root.join(format!("assets/minecraft/overrides/item/{carrier}.json"));
        if let Ok(bytes) = fs::read(&chime) {
            if let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) {
                if let Some(entries) = value["overrides"].as_array_mut() {
                    entries.retain(|entry| {
                        !entry["model"]
                            .as_str()
                            .is_some_and(|model| model.starts_with("fastpack:via/"))
                    });
                    write(root, &chime, &value)?;
                }
            }
        }
    }
    let cit = root.join("assets/minecraft/optifine/cit/fastpack_via");
    if cit.pack_is_dir() {
        for entry in crate::packfs::WalkDir::new(&cit).min_depth(1) {
            let entry = entry?;
            if entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "properties")
            {
                backup(root, entry.path())?;
                fs::remove_file(entry.path())?;
            }
        }
    }
    manifest["enabled"] = json!(false);
    write(root, &metadata, &manifest)?;
    report
        .converted
        .push("已关闭本次布吉岛物品适配，恢复转换包中由 Fast Pack 添加的载体模型规则".into());
    Ok(())
}

pub(super) fn migrate(
    root: &Path,
    target: u32,
    version: &str,
    report: &mut AssetReport,
) -> Result<()> {
    if target < 6
        || version.starts_with("1.16")
            && !matches!(version, "1.16.2" | "1.16.3" | "1.16.4" | "1.16.5")
    {
        return Ok(());
    }
    let Some((_, profile)) = CATALOG.profiles.range(..=target).next_back() else {
        return Ok(());
    };
    if target >= 98 {
        return Ok(());
    }
    let mut installer = Installer {
        root,
        target,
        installed: BTreeMap::new(),
    };
    let mut carriers = BTreeMap::<String, Vec<(Rule, String)>>::new();
    for rule in &profile.rules {
        let Some(item) = CATALOG.items.get(&rule.item) else {
            continue;
        };
        let Some(model) = installer.model(&format!("minecraft:{}", item.model))? else {
            continue;
        };
        if rule.carrier.ends_with("_spawn_egg")
            || matches!(
                rule.carrier.as_str(),
                "leather_horse_armor"
                    | "tipped_arrow"
                    | "potion"
                    | "splash_potion"
                    | "lingering_potion"
                    | "firework_star"
            )
        {
            installer.untinted(&model)?;
        }
        carriers
            .entry(rule.carrier.clone())
            .or_default()
            .push((rule.clone(), model));
    }
    let mut count = 0;
    for (carrier, rules) in &carriers {
        let path = root.join(format!("assets/minecraft/models/item/{carrier}.json"));
        let saved = root.join(format!("fastpack/via-bridge/base/{carrier}.json"));
        let current = if path.pack_is_file() {
            let Ok(value) = serde_json::from_slice::<Value>(&fs::read(&path)?) else {
                report.warnings.push(format!(
                    "协议物品 {carrier}：原包物品模型 JSON 无效，已保留原文件，跳过该载体"
                ));
                continue;
            };
            if !value.is_object() {
                report.warnings.push(format!(
                    "协议物品 {carrier}：原包物品模型不是 JSON 对象，已保留原文件，跳过该载体"
                ));
                continue;
            }
            value
        } else {
            super::item_transitions::model_reference(target, carrier)
                .or_else(|| {
                    super::model_integrity::native_model(
                        target,
                        &format!("minecraft:item/{carrier}"),
                    )
                })
                .unwrap_or_else(|| profile.carriers[carrier].clone())
        };
        let original =
            if current.get("fastpack_via_bridge") == Some(&json!(1)) && saved.pack_is_file() {
                serde_json::from_slice::<Value>(&fs::read(&saved)?)?
            } else {
                current
            };
        if !saved.pack_is_file() {
            fs::create_dir_all(saved.parent().unwrap())?;
            fs::write(saved, serde_json::to_vec_pretty(&original)?)?;
        }
        if target < 46 {
            numeric_model(root, carrier, &original, rules, report)?;
            tags(root, carrier, rules)?;
        } else {
            modern(root, target, carrier, rules)?;
        }
        count += rules.len();
    }
    for (language, translations) in &CATALOG.languages {
        let path = root.join(format!("assets/minecraft/lang/{language}.json"));
        let mut values = if path.pack_is_file() {
            let Ok(value) = serde_json::from_slice::<Value>(&fs::read(&path)?) else {
                continue;
            };
            value
        } else {
            json!({})
        };
        let Some(values) = values.as_object_mut() else {
            continue;
        };
        for rules in carriers.values() {
            for (rule, _) in rules {
                if let Some(name) = translations
                    .get(&format!("item.minecraft.{}", rule.item))
                    .or_else(|| translations.get(&format!("block.minecraft.{}", rule.item)))
                {
                    values
                        .entry(format!("vb.item.{}", rule.item))
                        .or_insert_with(|| name.clone());
                }
            }
        }
        write(root, &path, &Value::Object(values.clone()))?;
    }
    let metadata = root.join("fastpack/via-bridge/manifest.json");
    let rules:Vec<_>=carriers.values().flatten().map(|(rule,model)|json!({"item":rule.item,"carrier":rule.carrier,"custom_model_data":rule.cmd,"nbt_key":rule.key,"nbt_id":rule.id,"model":model})).collect();
    write(
        root,
        &metadata,
        &json!({"schema":1,"client":version,"registry_profile":profile.version,"items":rules,"requires":"ViaBackwards identifiers; native CustomModelData with recent servers, or OptiFine CIT / Chime with older servers","scope":"inventory and held models only"}),
    )?;
    fs::write(
        root.join("fastpack/via-bridge/NOTICE.txt"),
        include_str!("../../assets/via-items/NOTICE.txt"),
    )?;
    report.generated.push(format!("协议物品外观：为 {version} 自动生成 {count} 个新版物品的原版 CustomModelData{}规则及资源，保留未标记物品和原包模型",if target<46{"、OptiFine CIT / Chime "}else{" / 字符串选择 "}));
    Ok(())
}
