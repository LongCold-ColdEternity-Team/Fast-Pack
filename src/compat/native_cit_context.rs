//! Compile display-context alternatives without turning GUI layers into global layers.
use super::*;
use tree::{Atom, Case};

pub(super) fn compile(node: &Value) -> Result<(Vec<Case>, bool)> {
    if !has_context(node) {
        return Ok((tree::compile(node)?, false));
    }
    let base = select(node, "none", 0)?;
    let mut cases = tree::compile(&base)?;
    let mut scoped = false;
    for context in [
        "gui",
        "firstperson_righthand",
        "firstperson_lefthand",
        "thirdperson_righthand",
        "thirdperson_lefthand",
        "ground",
        "fixed",
        "head",
    ] {
        let branch = select(node, context, 0)?;
        if branch == base {
            continue;
        }
        scoped = true;
        let alternatives = tree::compile(&branch)?;
        let mut combined = vec![];
        // Preserve each branch's first-match priority, including its fallback.
        for alternative in alternatives {
            for previous in &cases {
                let Some(tests) = intersect(&previous.tests, &alternative.tests)? else {
                    continue;
                };
                let mut case = previous.clone();
                case.tests = tests;
                for (key, threshold) in &alternative.runtime {
                    let old = case.runtime.entry(key.clone()).or_insert(*threshold);
                    *old = old.max(*threshold);
                }
                for (key, threshold) in &alternative.runtime_max {
                    let old = case.runtime_max.entry(key.clone()).or_insert(*threshold);
                    *old = old.min(*threshold);
                }
                if case
                    .runtime_max
                    .iter()
                    .any(|(key, upper)| case.runtime.get(key).copied().unwrap_or(0.0) >= *upper)
                {
                    continue;
                }
                case.perspectives
                    .insert(context.into(), alternative.parts.clone());
                combined.push(case);
                ensure!(combined.len() <= 8192, "显示场景条件组合超过安全上限");
            }
        }
        cases = combined;
    }
    Ok((cases, scoped))
}

fn has_context(node: &Value) -> bool {
    match node {
        Value::Object(map) => {
            map.get("property")
                .is_some_and(|v| v == "minecraft:display_context" || v == "display_context")
                || map.values().any(has_context)
        }
        Value::Array(values) => values.iter().any(has_context),
        _ => false,
    }
}

fn select(node: &Value, context: &str, depth: usize) -> Result<Value> {
    ensure!(depth < 128, "显示场景模型树嵌套过深");
    let kind = |key: &str| {
        node[key]
            .as_str()
            .unwrap_or("")
            .trim_start_matches("minecraft:")
    };
    if kind("type") == "select" && kind("property") == "display_context" {
        let entries = node["cases"].as_array().context("显示场景选择缺少 cases")?;
        let selected = entries.iter().find(|entry| {
            entry["when"] == context
                || entry["when"]
                    .as_array()
                    .is_some_and(|list| list.iter().any(|value| value == context))
        });
        let selected = selected
            .map(|entry| &entry["model"])
            .or_else(|| node.get("fallback"))
            .context("显示场景选择缺少 fallback")?;
        return select(selected, context, depth + 1);
    }
    Ok(match node {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| Ok((key.clone(), select(value, context, depth + 1)?)))
                .collect::<Result<_>>()?,
        ),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| select(value, context, depth + 1))
                .collect::<Result<_>>()?,
        ),
        value => value.clone(),
    })
}

fn intersect(a: &[Atom], b: &[Atom]) -> Result<Option<Vec<Atom>>> {
    let mut result = a.to_vec();
    for atom in b {
        let mut duplicate = false;
        for old in &mut result {
            match (&mut *old, atom) {
                (Atom::Trim(a), Atom::Trim(b)) => {
                    if a != b {
                        return Ok(None);
                    }
                    duplicate = true;
                }
                (
                    Atom::Damage {
                        min: a,
                        max: b,
                        percent: p,
                    },
                    Atom::Damage {
                        min: c,
                        max: d,
                        percent: q,
                    },
                ) => {
                    ensure!(p == q, "不同单位的损伤条件无法等价合并");
                    *a = (*a).max(*c);
                    *b = upper(*b, *d);
                    if b.is_some_and(|end| end < *a) {
                        return Ok(None);
                    }
                    duplicate = true;
                }
                (Atom::Cmd { min: a, max: b }, Atom::Cmd { min: c, max: d }) => {
                    *a = (*a).max(*c);
                    *b = upper(*b, *d);
                    if b.is_some_and(|end| end < *a) {
                        return Ok(None);
                    }
                    duplicate = true;
                }
                (Atom::Component(a, x), Atom::Component(b, y)) if a == b && x == y => {
                    duplicate = true
                }
                _ => {}
            }
            if duplicate {
                break;
            }
        }
        if !duplicate {
            result.push(atom.clone());
        }
    }
    // Other conjunctions must remain expressible as an actual CIT AND, never
    // become an OR by overwriting one predicate with another.
    tree::properties(&result)?;
    Ok(Some(result))
}

fn upper(a: Option<u32>, b: Option<u32>) -> Option<u32> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

pub(super) fn model(
    root: &Path,
    target: u32,
    item: &str,
    case: &Case,
    outputs: &mut BTreeMap<String, Vec<u8>>,
    live_base: bool,
) -> Result<Value> {
    let compose = if live_base {
        models::compose_overlay
    } else {
        models::compose
    };
    let mut base = compose(root, target, item, &case.parts, outputs)?;
    atlas_safe(root, target, &mut base, outputs)?;
    if case.perspectives.is_empty() {
        return Ok(base);
    }
    let mut perspectives = serde_json::Map::new();
    for (name, parts) in &case.perspectives {
        if *parts != case.parts {
            let mut child = compose(root, target, item, parts, outputs)?;
            atlas_safe(root, target, &mut child, outputs)?;
            perspectives.insert(name.clone(), child);
        }
    }
    if perspectives.is_empty() {
        return Ok(base);
    }
    let mut result = base.clone();
    result["loader"] = json!("forge:separate_transforms");
    result["base"] = base;
    result["perspectives"] = Value::Object(perspectives);
    if live_base && result["textures"].get("particle").is_none() {
        result["textures"]["particle"] = json!("#layer0");
    }
    if result["textures"].get("particle").is_none() {
        if let Some(particle) = result["textures"]["layer0"].as_str().map(str::to_owned) {
            result["textures"]["particle"] = json!(particle);
        }
    }
    Ok(result)
}

fn atlas_safe(
    root: &Path,
    target: u32,
    model: &mut Value,
    outputs: &mut BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    if target < 12 {
        return Ok(());
    }
    if let Some(textures) = model.get_mut("textures").and_then(Value::as_object_mut) {
        for value in textures.values_mut() {
            let Some(reference) = value.as_str().map(str::to_owned) else {
                continue;
            };
            if reference.starts_with('#') {
                continue;
            }
            let (namespace, name) = resource(&reference)?;
            if name.starts_with("item/") || name.starts_with("block/") {
                continue;
            }
            let path = root.join(format!("assets/{namespace}/textures/{name}.png"));
            if !path.pack_is_file() {
                continue;
            } // Native palette-generated sprite.
            let id = hash(reference.as_bytes());
            let output = format!("assets/minecraft/textures/item/fastpack_native_cit/{id}.png");
            outputs.insert(output.clone(), fs::read(&path)?);
            let metadata = path.with_extension("png.mcmeta");
            if metadata.pack_is_file() {
                outputs.insert(format!("{output}.mcmeta"), fs::read(metadata)?);
            }
            *value = json!(format!("minecraft:item/fastpack_native_cit/{id}"));
        }
    }
    Ok(())
}
