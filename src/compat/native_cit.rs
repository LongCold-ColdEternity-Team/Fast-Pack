//! Native item model trees -> OptiFine rules, plus lossless provenance for return trips.
use super::assets::{AssetReport, backup};
use super::properties::{self, Properties};
use crate::packfs::{self as fs, PathExt, WalkDir};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

#[path = "native_cit_context.rs"]
mod contexts;
#[path = "native_cit_models.rs"]
mod models;
#[path = "native_cit_potions.rs"]
mod potions;
#[cfg(test)]
#[path = "native_cit_tests.rs"]
mod tests;
#[path = "native_cit_tree.rs"]
mod tree;

const CACHE: &str = "fastpack/native-cit.json";
const FOLDER: &str = "assets/minecraft/optifine/cit/fastpack_native";
const GENERATOR: u32 = 3;

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))[..24].into()
}
fn write(root: &Path, relative: &str, bytes: &[u8]) -> Result<()> {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().context("缺少资源目录")?)?;
    fs::write_atomic(&path, bytes)?;
    Ok(())
}
fn resource(value: &str) -> Result<(String, String)> {
    let (namespace, name) = value.split_once(':').unwrap_or(("minecraft", value));
    ensure!(
        !namespace.is_empty()
            && !name.is_empty()
            && namespace
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
            && name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-/".contains(&c))
            && !name.split('/').any(|part| matches!(part, "" | "." | "..")),
        "非法资源引用 {value}"
    );
    Ok((namespace.into(), name.into()))
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    source: u32,
    definitions: BTreeMap<String, Value>,
    models: BTreeMap<String, Value>,
    generated: BTreeMap<String, String>,
    ready: bool,
    restoring: bool,
    generator: u32,
}

impl State {
    fn validate(&self) -> Result<()> {
        let safe = |value: &str| {
            !value.contains('\\')
                && Path::new(value)
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)))
                && value.starts_with("assets/")
                && value.split('/').count() > 3
        };
        ensure!(
            self.definitions
                .keys()
                .chain(self.models.keys())
                .all(|path| safe(path)
                    && path.ends_with(".json")
                    && (path.contains("/items/") || path.contains("/models/"))),
            "原版 CIT 缓存含无效资源路径"
        );
        ensure!(
            self.generated.keys().all(|path| safe(path)
                && (path.starts_with(&format!("{FOLDER}/"))
                    || path.starts_with("assets/minecraft/models/item/fastpack_native_cit/")
                    || path.starts_with("assets/minecraft/textures/item/fastpack_native_cit/"))),
            "原版 CIT 缓存含无效生成路径"
        );
        Ok(())
    }
}

/// Capture before any version-dependent rewriting changes the authored model tree.
pub(crate) fn prepare(root: &Path, source: u32, target: u32) -> Result<Vec<String>> {
    let mut notes = vec![];
    if source >= 46 && target < 46 {
        let own: Value = fs::read(root.join("fastpack/cit-items.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or(Value::Null);
        let mut state = State {
            source,
            ..Default::default()
        };
        for entry in WalkDir::new(root.join("assets")) {
            let entry = entry?;
            if !entry.file_type().is_file()
                || entry
                    .path()
                    .extension()
                    .is_none_or(|extension| extension != "json")
            {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            let parts: Vec<_> = relative.split('/').collect();
            let mut value = match crate::model_json::parse(&fs::read(entry.path())?) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if parts.get(2) == Some(&"items") && parts.len() >= 4 && conditional(&value) {
                let item = format!(
                    "{}:{}",
                    parts[1],
                    parts[3..].join("/").trim_end_matches(".json")
                );
                if own["items"][&item]["output"] == value["model"] {
                    continue;
                }
                // Protocol carrier selectors have their own migration. Only the
                // author's saved fallback belongs to this conditional-CIT bridge.
                if value["fastpack_via_bridge"] == json!(1) && parts[1] == "minecraft" {
                    let saved = root.join(format!(
                        "fastpack/via-bridge/definitions/{}",
                        parts[3..].join("/")
                    ));
                    if saved.pack_is_file() {
                        value = crate::model_json::parse(&fs::read(saved)?)?;
                        if !conditional(&value) {
                            continue;
                        }
                    }
                }
                state.definitions.insert(relative, value);
            } else if parts.get(2) == Some(&"models") {
                state.models.insert(relative, value);
            }
        }
        if !state.definitions.is_empty() {
            write(root, CACHE, &serde_json::to_vec(&state)?)?;
        }
    } else if source < 46 && target >= 46 && root.join(CACHE).pack_is_file() {
        let mut state: State = serde_json::from_slice(&fs::read(root.join(CACHE))?)?;
        state.validate()?;
        if target < 55 && state.definitions.values().any(component_condition) {
            notes.push("此目标原版尚不支持组件条件物品模型，保留旧版 CIT 规则；条件外观仍需兼容 CIT 客户端".into());
            return Ok(notes);
        }
        if !state.ready {
            return Ok(notes);
        }
        let modified = state.generated.iter().any(|(path, expected)| {
            fs::read(root.join(path))
                .map(|bytes| hash(&bytes) != *expected)
                .unwrap_or(true)
        });
        if modified {
            notes.push("原版条件模型往返：已生成的旧版规则或模型被编辑，保留编辑结果，不覆盖回最初的模型树".into());
            return Ok(notes);
        }
        for (path, value) in state.definitions.iter().chain(&state.models) {
            if root.join(path).pack_is_file() {
                backup(root, &root.join(path))?;
            }
            write(root, path, &serde_json::to_vec(value)?)?;
        }
        for path in state.generated.keys() {
            if root.join(path).pack_is_file() {
                fs::remove_file(root.join(path))?;
            }
        }
        state.ready = false;
        state.restoring = true;
        notes.push(format!(
            "原版条件模型往返：恢复 {} 个完整定义，包含背包显示条件、叠加图层与原有状态",
            state.definitions.len()
        ));
        write(root, CACHE, &serde_json::to_vec(&state)?)?;
    }
    Ok(notes)
}

fn conditional(value: &Value) -> bool {
    match value {
        Value::Object(values) => {
            values
                .get("property")
                .and_then(Value::as_str)
                .is_some_and(|value| {
                    matches!(
                        value,
                        "minecraft:component" | "minecraft:damage" | "minecraft:custom_model_data"
                    )
                })
                || values.values().any(conditional)
        }
        Value::Array(array) => array.iter().any(conditional),
        _ => false,
    }
}

fn component_condition(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            map.get("property")
                .is_some_and(|value| value == "minecraft:component")
                || map.values().any(component_condition)
        }
        Value::Array(array) => array.iter().any(component_condition),
        _ => false,
    }
}

pub(super) fn preserve_restored_definitions(root: &Path, target: u32) -> Result<()> {
    if target < 55 || !root.join(CACHE).pack_is_file() {
        return Ok(());
    }
    let mut state: State = serde_json::from_slice(&fs::read(root.join(CACHE))?)?;
    state.validate()?;
    if state.ready || !state.restoring {
        return Ok(());
    }
    for (path, value) in &state.definitions {
        write(root, path, &serde_json::to_vec(value)?)?;
    }
    state.restoring = false;
    write(root, CACHE, &serde_json::to_vec(&state)?)?;
    Ok(())
}

pub(super) fn migrate(
    root: &Path,
    target: u32,
    version: &str,
    report: &mut AssetReport,
) -> Result<()> {
    if target >= 46 || !root.join(CACHE).pack_is_file() {
        return Ok(());
    }
    let mut state: State = serde_json::from_slice(&fs::read(root.join(CACHE))?)?;
    state.validate()?;
    if target < 4 {
        report.warnings.push("原版条件模型：此旧版本没有可等价表达所有模型／组件条件的标准接口，完整原始模型树保留供再次升级".into());
        return Ok(());
    }
    if state.ready {
        if state.generator >= GENERATOR {
            return Ok(());
        }
        if state.generated.iter().any(|(path, expected)| {
            fs::read(root.join(path))
                .map(|bytes| hash(&bytes) != *expected)
                .unwrap_or(true)
        }) {
            report.warnings.push("旧版条件模型修复：生成文件已被编辑或删除，保留用户改动；使用原包重新转换可应用最新修复".into());
            return Ok(());
        }
        for path in state.generated.keys() {
            if root.join(path).pack_is_file() {
                fs::remove_file(root.join(path))?;
            }
        }
        state.generated.clear();
        state.ready = false;
        report.converted.push("重新生成旧版条件模型，修复此前转换的背包数字层与药水匹配；用户未修改的旧生成文件已替换".into());
    }
    let catalog: BTreeSet<_> = super::custom_items::items(version)
        .iter()
        .map(|item| item.id.clone())
        .collect();
    let mut total = 0;
    let mut skipped = 0;
    for (path, definition) in state.definitions.clone() {
        let parts: Vec<_> = path.split('/').collect();
        let item = format!(
            "{}:{}",
            parts[1],
            parts[3..].join("/").trim_end_matches(".json")
        );
        if !catalog.contains(&item) {
            skipped += 1;
            continue;
        }
        let compiled = if target >= 8 {
            contexts::compile(&definition["model"])
        } else {
            tree::legacy_contexts(&definition["model"]).and_then(|(node, removed)| {
                if removed > 0 { report.warnings.push(format!("原版条件模型 {item}：此目标没有可用的 Forge 场景模型接口，保留基础外观与完整原数据")); }
                Ok((tree::compile(&node)?, false))
            })
        };
        match compiled {
            Ok((cases, scoped)) => {
                if scoped {
                    report.warnings.push(format!("原版条件模型 {item}：背包数字通过 Forge separate_transforms 单独显示；手持、掉落和展示保留对应基础模型。非 Forge 客户端使用基础外观"));
                }
                match emit(root, target, &item, &cases, &mut state, report, false) {
                    Ok(count) => total += count,
                    Err(error) => report.warnings.push(format!(
                        "原版条件模型 {item}：{error:#}；完整原始定义已保留"
                    )),
                }
            }
            Err(error) => report.warnings.push(format!(
                "原版条件模型 {item}：{error:#}；原数据保留，不放宽条件"
            )),
        }
    }
    if total > 0 {
        report.converted.push(format!("原版条件模型降级：为 {version} 生成 {total} 条 OptiFine CIT 规则，处理附魔、药水、耐久与叠加图层，保留弓的拉弓状态"));
        report.warnings.push("旧版条件外观需要启用兼容客户端的自定义物品功能；显示场景模型需 Forge，原版客户端仍使用基础模型".into());
    }
    if skipped > 0 {
        report.warnings.push(format!(
            "原版条件模型：目标版本尚无 {skipped} 个物品，相关定义保留供后续升级"
        ));
    }
    state.ready = true;
    state.generator = GENERATOR;
    for path in state.definitions.keys() {
        if root.join(path).pack_is_file() {
            fs::remove_file(root.join(path))?;
        }
    }
    write(root, CACHE, &serde_json::to_vec(&state)?)?;
    Ok(())
}

fn emit(
    root: &Path,
    target: u32,
    item: &str,
    cases: &[tree::Case],
    state: &mut State,
    report: &mut AssetReport,
    live_base: bool,
) -> Result<usize> {
    let cases = cases
        .iter()
        .cloned()
        .map(|mut case| -> Result<_> {
            if let Some(value) = case.runtime.remove("__using_item") {
                let key = match item {
                    "minecraft:bow" | "minecraft:crossbow" => "pulling",
                    "minecraft:shield" => "blocking",
                    "minecraft:trident" => "throwing",
                    "minecraft:goat_horn" => "tooting",
                    _ => bail!("目标物品没有可表达 using_item 的旧版状态属性"),
                };
                case.runtime.insert(key.into(), value);
            }
            ensure!(
                !case.runtime.contains_key("pull")
                    || matches!(item, "minecraft:bow" | "minecraft:crossbow"),
                "此物品的使用时长无法转换为旧版拉弓状态"
            );
            Ok(case)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut grouped: BTreeMap<String, (Properties, Vec<(usize, &tree::Case)>)> = BTreeMap::new();
    for (priority, case) in cases.iter().enumerate() {
        let props = tree::properties(&case.tests)?;
        let key = properties::write(&props);
        grouped
            .entry(key)
            .or_insert_with(|| (props, vec![]))
            .1
            .push((priority, case));
    }
    ensure!(grouped.len() <= 4096, "条件展开超过安全上限");
    let mut outputs = BTreeMap::new();
    let mut missing = BTreeSet::new();
    for (identity, (mut props, variants)) in grouped {
        if props.is_empty()
            && variants.iter().all(|(_, case)| {
                !case.perspectives.is_empty()
                    && case.perspectives.values().all(|parts| *parts == case.parts)
            })
        {
            continue;
        }
        let mut pending = BTreeMap::new();
        let result = (|| -> Result<()> {
            let base = variants
                .iter()
                .find(|(_, case)| case.runtime.is_empty())
                .or_else(|| variants.last())
                .unwrap()
                .1;
            let mut model = contexts::model(root, target, item, base, &mut pending, live_base)?;
            let mut overrides = vec![];
            let mut dynamic = variants
                .iter()
                .filter(|(_, case)| !case.runtime.is_empty())
                .collect::<Vec<_>>();
            dynamic.sort_by(|(a, x), (b, y)| {
                x.runtime.len().cmp(&y.runtime.len()).then_with(|| b.cmp(a))
            });
            for (_, case) in dynamic {
                let value = contexts::model(root, target, item, case, &mut pending, live_base)?;
                let reference = model_output(&value, &mut pending)?;
                overrides.push(json!({"predicate":case.runtime,"model":reference}));
            }
            if !overrides.is_empty() {
                model["overrides"] = json!(overrides);
            }
            let reference = model_output(&model, &mut pending)?;
            props.insert("type".into(), "item".into());
            props.insert("items".into(), item.into());
            props.insert("model".into(), reference);
            let priority = variants
                .iter()
                .map(|(priority, _)| *priority)
                .min()
                .unwrap();
            props.insert(
                "weight".into(),
                (100000usize.saturating_sub(priority)).to_string(),
            );
            for (suffix, variant) in potions::variants(&props, target, item) {
                pending.insert(
                    format!(
                        "{FOLDER}/{}-{suffix}.properties",
                        hash(format!("{item}:{identity}").as_bytes())
                    ),
                    properties::write(&variant).into_bytes(),
                );
            }
            potions::prioritize(&mut props, target, item);
            let name = hash(format!("{item}:{identity}").as_bytes());
            pending.insert(
                format!("{FOLDER}/{name}.properties"),
                properties::write(&props).into_bytes(),
            );
            Ok(())
        })();
        match result {
            Ok(()) => outputs.extend(pending),
            Err(error) => {
                missing.insert(format!("{error:#}"));
            }
        }
    }
    if !missing.is_empty() {
        report.warnings.push(format!(
            "原版条件模型 {item}：{} 个目标不支持的状态保留在原始定义中（{}），其他状态继续迁移",
            missing.len(),
            missing.into_iter().take(3).collect::<Vec<_>>().join("；")
        ));
    }
    for (path, bytes) in &outputs {
        ensure!(
            !root.join(path).pack_is_file()
                || fs::read(root.join(path))? == *bytes
                || state.generated.contains_key(path),
            "生成资源与作者已有文件冲突：{path}"
        );
    }
    let count = outputs
        .keys()
        .filter(|path| path.ends_with(".properties"))
        .count();
    for (path, bytes) in outputs {
        write(root, &path, &bytes)?;
        state.generated.insert(path, hash(&bytes));
    }
    Ok(count)
}

fn model_output(model: &Value, outputs: &mut BTreeMap<String, Vec<u8>>) -> Result<String> {
    let bytes = serde_json::to_vec(model)?;
    let name = hash(&bytes);
    outputs.insert(
        format!("assets/minecraft/models/item/fastpack_native_cit/{name}.json"),
        bytes,
    );
    Ok(format!("minecraft:item/fastpack_native_cit/{name}"))
}

/// Compile built-in GUI layers in a separate output namespace. Do not replace
/// the input pack's native-CIT provenance or its existing generated rules.
pub(crate) fn install_builtin_hud(
    root: &Path,
    target: u32,
    definitions: &BTreeMap<String, Value>,
    report: &mut AssetReport,
) -> Result<()> {
    if target < 8 {
        report.warnings.push("内置等级数字：此版本缺少可验证的背包独立图层接口，保留原物品外观；1.18–1.21.4 需要 CIT 与 Forge 场景模型，1.21.5+ 支持原版条件模型".into());
        return Ok(());
    }
    let scratch = tempfile::tempdir()?;
    let stage = scratch.path().join("hud");
    let mut inputs = Vec::new();
    // Use authored item models/textures when baking the GUI and hand branches.
    for entry in WalkDir::new(root.join("assets")) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(root)?;
        let text = relative.to_string_lossy().replace('\\', "/");
        if text.contains("/models/")
            || text.contains("/textures/item/")
            || text.contains("/textures/items/")
        {
            inputs.push((relative.to_path_buf(), fs::read(entry.path())?));
        }
    }
    let generated = {
        let _memory = fs::Mount::new(scratch.path());
        fs::create_dir_all(&stage)?;
        for (relative, bytes) in inputs {
            write(
                &stage,
                &relative.to_string_lossy().replace('\\', "/"),
                &bytes,
            )?;
        }
        let mut state = State::default();
        for (item, definition) in definitions {
            match contexts::compile(&definition["model"]) {
                Ok((cases, _)) => {
                    emit(&stage, target, item, &cases, &mut state, report, true)?;
                }
                Err(error) => report
                    .warnings
                    .push(format!("内置等级数字 {item}：{error:#}")),
            }
        }
        state
            .generated
            .keys()
            .map(|relative| Ok((relative.clone(), fs::read(stage.join(relative))?)))
            .collect::<Result<Vec<_>>>()?
    };
    for (relative, bytes) in generated {
        let destination = relative.replace("fastpack_native", "fastpack_builtin_hud");
        let bytes = if relative.ends_with(".json") || relative.ends_with(".properties") {
            String::from_utf8(bytes)?
                .replace("fastpack_native", "fastpack_builtin_hud")
                .into_bytes()
        } else {
            bytes
        };
        let output = root.join(&destination);
        if output.pack_is_file() {
            backup(root, &output)?;
        }
        write(root, &destination, &bytes)?;
    }
    report.converted.push(
        "内置背包数字：基础物品模型、纹理与手持姿态跟随当前生效的材质包，数字层只作用于背包显示"
            .into(),
    );
    report.warnings.push("内置背包等级数字在此版本需要兼容 CIT 和 Forge separate_transforms 的客户端；不支持该接口时使用正常基础外观，数字不会贴到手持物品上".into());
    Ok(())
}

pub(super) fn generated_rules(root: &Path) -> Result<BTreeSet<PathBuf>> {
    if !root.join(CACHE).pack_is_file() {
        return Ok(BTreeSet::new());
    }
    let state: State = serde_json::from_slice(&fs::read(root.join(CACHE))?)?;
    state.validate()?;
    Ok(state
        .generated
        .keys()
        .filter(|path| path.ends_with(".properties"))
        .map(|path| root.join(path))
        .collect())
}

pub(super) fn finalize(root: &Path) -> Result<()> {
    if !root.join(CACHE).pack_is_file() {
        return Ok(());
    }
    let mut state: State = serde_json::from_slice(&fs::read(root.join(CACHE))?)?;
    state.validate()?;
    if !state.ready {
        return Ok(());
    }
    for (path, checksum) in &mut state.generated {
        if root.join(path).pack_is_file() {
            *checksum = hash(&fs::read(root.join(path))?);
        }
    }
    write(root, CACHE, &serde_json::to_vec(&state)?)
}
