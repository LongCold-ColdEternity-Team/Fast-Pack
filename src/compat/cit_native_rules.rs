use super::super::cit_conditions;
use super::*;

pub(super) fn needed(paths: &BTreeMap<PathBuf, String>) -> Result<bool> {
    for path in paths.keys() {
        let Ok(values) = properties::parse(&fs::read(path)?) else {
            continue;
        };
        if values.get("type").is_some_and(|value| value != "item") {
            continue;
        }
        if values.keys().any(|key| {
            matches!(
                key.as_str(),
                "enchantments"
                    | "enchantmentIDs"
                    | "enchantmentLevels"
                    | "damage"
                    | "stackSize"
                    | "nbt.Potion"
            ) || key.starts_with("nbt.Fireworks")
                || key.contains(":fireworks")
                || key.contains("potion_contents")
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

impl Resources<'_> {
    fn stateful_appearance(
        &mut self,
        source: &Path,
        values: &Properties,
        item: &str,
    ) -> Result<Value> {
        if values.contains_key("model") {
            return self.appearance(source, values, item);
        }
        let (_, name) = id(item)?;
        if !matches!(
            name.as_str(),
            "bow" | "crossbow" | "fishing_rod" | "shield" | "trident"
        ) {
            return self.appearance(source, values, item);
        }
        let Some(mut tree) = super::super::modern_items::definition_reference(self.target, &name)
            .map(|value| value["model"].clone())
        else {
            return self.appearance(source, values, item);
        };
        // Keep the selector template (pulling, charging, casting, etc.) intact and
        // substitute only the authored appearance for each named state.
        fn walk(
            resources: &mut Resources<'_>,
            source: &Path,
            props: &Properties,
            item: &str,
            name: &str,
            tree: &mut Value,
        ) -> Result<()> {
            if tree["type"] == "minecraft:model" {
                if let Some(reference) = tree["model"].as_str() {
                    let state = reference.rsplit('/').next().unwrap();
                    let mut variant = props.clone();
                    if let Some(model) = props.get(&format!("model.{state}")) {
                        variant.insert("model".into(), model.clone());
                    } else if let Some(texture) = props.get(&format!("texture.{state}")) {
                        variant.insert("texture".into(), texture.clone());
                    } else if state != name {
                        return Ok(());
                    }
                    let replacement = resources.appearance(source, &variant, item)?;
                    let reference = replacement["model"].as_str().unwrap();
                    let (ns, path) = id(reference)?;
                    let output = resources
                        .root
                        .join(format!("assets/{ns}/models/{path}.json"));
                    let bytes = resources
                        .outputs
                        .get(&output)
                        .context("缺少生成状态模型")?
                        .clone();
                    let unique = format!("{PREFIX}{}", hash(&bytes));
                    resources.outputs.insert(
                        resources
                            .root
                            .join(format!("assets/{ns}/models/{unique}.json")),
                        bytes,
                    );
                    *tree = json!({"type":"minecraft:model","model":format!("{ns}:{unique}")});
                    return Ok(());
                }
            }
            match tree {
                Value::Object(map) => {
                    for value in map.values_mut() {
                        walk(resources, source, props, item, name, value)?;
                    }
                }
                Value::Array(array) => {
                    for value in array {
                        walk(resources, source, props, item, name, value)?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        walk(self, source, values, item, &name, &mut tree)?;
        Ok(tree)
    }
}

pub(super) fn migrate(
    root: &Path,
    target: u32,
    version: &str,
    paths: &BTreeMap<PathBuf, String>,
    report: &mut AssetReport,
) -> Result<()> {
    let cache = root.join("fastpack/cit-items.json");
    let previous: State = fs::read(&cache)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let catalog: BTreeSet<_> = super::super::custom_items::items(version)
        .iter()
        .map(|item| item.id.clone())
        .collect();
    let mut rules = vec![];
    let mut notes = BTreeMap::new();
    for (source, origin) in paths {
        let parsed = match rule(source, origin, &catalog) {
            Ok(rule) => rule,
            Err(error) => {
                notes.insert(origin.clone(), format!("{error:#}"));
                continue;
            }
        };
        if let Some(mut rule) = parsed {
            match cit_conditions::compile(&rule.values) {
                Ok(tests) => {
                    rule.unsupported = None;
                    rules.push((rule, Some(tests)));
                }
                Err(error) => {
                    notes.insert(origin.clone(), format!("{error:#}"));
                    rules.push((rule, None));
                }
            }
        }
    }
    rules.sort_by(|(a, _), (b, _)| {
        b.weight
            .cmp(&a.weight)
            .then_with(|| a.origin.cmp(&b.origin))
    });
    let mut resources = Resources::new(root, target)?;
    let mut state = State {
        items: BTreeMap::new(),
        assets: previous.assets.clone(),
    };
    let all: BTreeSet<_> = rules
        .iter()
        .flat_map(|(rule, _)| rule.items.iter().cloned())
        .chain(previous.items.keys().cloned())
        .collect();
    let mut definitions = BTreeMap::new();
    let mut count = 0;
    for item in all {
        let (namespace, name) = id(&item)?;
        let path = root.join(format!("assets/{namespace}/items/{name}.json"));
        let mut definition = if path.pack_is_file() {
            crate::model_json::parse(&fs::read(&path)?)?
        } else {
            super::super::modern_items::definition_reference(target,&name).unwrap_or_else(||json!({"model":{"type":"minecraft:model","model":format!("{namespace}:item/{name}")}}))
        };
        ensure!(definition.is_object(), "CIT：物品定义 {item} 不是 JSON 对象");
        if let Some(saved) = previous.items.get(&item) {
            if definition["model"] != saved.output {
                report
                    .warnings
                    .push(format!("CIT：{item} 的定义已被作者修改，保留原样"));
                state.items.insert(item, saved.clone());
                continue;
            }
            definition["model"] = saved.base.clone();
        }
        let base = definition["model"].clone();
        let matching: Vec<_> = rules
            .iter()
            .filter(|(rule, _)| rule.items.contains(&item))
            .take_while(|(_, tests)| tests.is_some())
            .collect();
        let mut tree = base.clone();
        for (rule, tests) in matching.into_iter().rev() {
            match resources.stateful_appearance(&rule.source, &rule.values, &item) {
                Ok(appearance) => {
                    let factor = tests
                        .as_ref()
                        .unwrap()
                        .iter()
                        .map(cit_conditions::Test::budget_factor)
                        .sum::<usize>()
                        .max(1);
                    let estimate = serde_json::to_vec(&tree)?
                        .len()
                        .saturating_mul(factor * 2)
                        .saturating_add(serde_json::to_vec(&appearance)?.len() * 256);
                    if estimate > 4 * 1024 * 1024 {
                        notes.insert(rule.origin.clone(),"条件树展开超过 4 MiB 安全上限，整项保留原模型与 CIT 规则，避免条件变宽".into());
                        tree = base.clone();
                        break;
                    }
                    tree = cit_conditions::wrap_all(tests.as_ref().unwrap(), appearance, &tree);
                    count += 1;
                }
                Err(error) => {
                    notes.insert(rule.origin.clone(), format!("资源未接入：{error:#}"));
                    // A broken higher-priority rule must not expose a lower-priority
                    // appearance for the same condition. Keep the original base there.
                    tree = cit_conditions::wrap_all(tests.as_ref().unwrap(), base.clone(), &tree);
                }
            }
        }
        definition["model"] = tree.clone();
        if tree != base {
            state.items.insert(item, Saved { base, output: tree });
        }
        definitions.insert(path, definition);
    }
    let mut reachable = BTreeSet::new();
    for value in definitions.values() {
        resources.reachable(value, &mut reachable);
    }
    for path in reachable {
        let relative = path
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = &resources.outputs[&path];
        ensure!(
            !path.pack_is_file()
                || fs::read(&path)? == *bytes
                || previous.assets.contains(&relative),
            "CIT 生成资源已有不同内容：{relative}"
        );
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write_atomic(&path, bytes)?;
        state.assets.insert(relative);
    }
    for (path, value) in definitions {
        if path.pack_is_file() {
            backup(root, &path)?;
        }
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write_atomic(&path, &serde_json::to_vec(&value)?)?;
    }
    fs::create_dir_all(cache.parent().unwrap())?;
    fs::write_atomic(&cache, &serde_json::to_vec(&state)?)?;
    if count > 0 {
        report.converted.push(format!("CIT 原版条件适配：为 {version} 接入 {count} 条附魔、耐久、药水或烟花规则，保留弓弩各状态与未命中的原模型"));
    }
    warnings(&notes, report);
    Ok(())
}
