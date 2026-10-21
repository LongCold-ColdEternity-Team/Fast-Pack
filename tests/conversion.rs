use fast_pack::converter::detect_format;
use fast_pack::{
    LATEST_VERSION, ConversionOptions, convert_pack, flatten_texture_name, legacy_texture_name,
    pack_format_of, probe_pack, suggest_output, tables, version_label_of,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};
#[path = "support/mod.rs"]
mod support;
use support::fixture;

fn contents(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut result = BTreeMap::new();
    if path.is_dir() {
        for entry in walkdir::WalkDir::new(path).min_depth(1) {
            let entry = entry.unwrap();
            if entry.file_type().is_file() {
                result.insert(
                    entry
                        .path()
                        .strip_prefix(path)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    } else {
        let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            if entry.is_dir() {
                continue;
            }
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            result.insert(entry.name().replace('\\', "/"), bytes);
        }
    }
    result
}

#[test]
fn original_reference_upgrade_matches() {
    let work = tempdir().unwrap();
    let output = work.path().join("modern.zip");
    let mut progress = Vec::new();
    let report = convert_pack(
        &fixture("legacy"),
        &output,
        &ConversionOptions::default(),
        |name, percent| progress.push((name.to_string(), percent)),
    )
    .unwrap();
    let actual = contents(&output);
    let expected = contents(&fixture("reference_upgrade.zip"));
    assert_eq!(
        actual.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>()
    );
    for (name, reference) in expected {
        let actual = actual.get(&name).unwrap();
        if name == "转换报告.txt" {
            continue;
        }
        if name.ends_with(".json") || name.ends_with(".mcmeta") {
            match (
                serde_json::from_slice::<Value>(actual),
                serde_json::from_slice::<Value>(&reference),
            ) {
                (Ok(actual), Ok(expected)) => assert_eq!(actual, expected, "{name}"),
                _ => assert_eq!(actual, &reference, "{name}"),
            }
        } else if name.ends_with(".txt") {
            let expected = String::from_utf8(reference)
                .unwrap()
                .replace("\r\n", "\n")
                .replace("Minecraft 材质包转换报告", "Fast Pack 材质包转换报告");
            assert_eq!(
                String::from_utf8(actual.clone())
                    .unwrap()
                    .replace("\r\n", "\n"),
                expected,
                "{name}"
            );
        } else {
            assert_eq!(actual, &reference, "{name}");
        }
    }
    assert_eq!(report.files_total, 22);
    assert_eq!(report.src_format, 1);
    assert_eq!(report.tgt_format, 100);
    assert_eq!(
        progress
            .iter()
            .map(|(_, percent)| *percent)
            .collect::<Vec<_>>(),
        [3, 10, 15, 30, 60, 70, 78, 80, 82, 84, 90, 95, 100]
    );
    assert!(report.render().contains("Fast Pack"));
    assert!(!report.render().contains("需人工确认"));
}

#[test]
fn downgrade_preserves_unknown_and_converts_language() {
    let work = tempdir().unwrap();
    let output = work.path().join("legacy");
    let options = ConversionOptions {
        tgt_format: 1,
        as_zip: false,
        ..Default::default()
    };
    let report = convert_pack(&fixture("modern"), &output, &options, |_, _| {}).unwrap();
    let files = contents(&output);
    assert!(files.contains_key("assets/minecraft/textures/blocks/stonebrick.png"));
    assert!(files.contains_key("assets/minecraft/textures/blocks/grass_path.png"));
    assert!(files.contains_key("assets/minecraft/textures/blocks/grass.png"));
    assert!(files.contains_key("assets/minecraft/textures/blocks/sulfur.png"));
    assert!(files.contains_key("assets/minecraft/textures/items/diamond_sword.png"));
    let language = String::from_utf8(files["assets/minecraft/lang/en_US.lang"].clone()).unwrap();
    assert!(language.contains("greeting=你好"));
    assert!(language.contains("value=3"));
    assert!(language.contains("nested={\"k\": \"v\"}"));
    assert_eq!(report.direction, "降级");
    assert!(
        report
            .unmapped
            .iter()
            .any(|name| name.ends_with("sulfur.png"))
    );
    assert!(
        !report
            .unmapped
            .iter()
            .any(|name| name.contains("short_grass") || name.contains("dirt_path"))
    );
    let meta: Value = serde_json::from_slice(&files["pack.mcmeta"]).unwrap();
    assert_eq!(meta["pack"]["pack_format"], 1);
    assert!(meta["pack"].get("min_format").is_none());
}

#[test]
fn embedded_tables_and_all_explicit_names() {
    let table = tables();
    assert_eq!(table.pack_formats.len(), 80);
    assert_eq!(table.explicit_block.len(), 38);
    assert_eq!(table.explicit_item.len(), 79);
    assert_eq!(table.block_rules.len(), 16);
    assert_eq!(table.inv_block_rules.len(), 15);
    assert_eq!(table.legacy_stable_block.len(), 157);
    assert_eq!(table.legacy_stable_item.len(), 115);
    for (name, expected) in &table.explicit_block {
        let flattened = flatten_texture_name(name, "block");
        let downgraded = if flattened == *name && *name != **expected {
            legacy_texture_name(name, "block")
        } else {
            Some(flattened)
        };
        assert_eq!(downgraded, Some(expected.to_owned()));
    }
    for (name, expected) in &table.explicit_item {
        assert_eq!(flatten_texture_name(name, "item"), *expected);
    }
    for (name, expected) in &table.inv_block {
        let expected = expected.replace("dark_oak", "big_oak");
        assert_eq!(legacy_texture_name(name, "block"), Some(expected));
    }
    assert_eq!(version_label_of(4), "1.14.4");
    assert_eq!(pack_format_of("26.2").unwrap(), 88);
    assert!(pack_format_of("missing").is_err());
}

#[test]
fn ordered_rules_and_legacy_color_normalization() {
    for color in &tables().colors {
        let old = if color == "light_gray" {
            "silver"
        } else {
            color.as_str()
        };
        assert_eq!(
            flatten_texture_name(&format!("wool_colored_{old}"), "block"),
            format!("{color}_wool")
        );
        assert_eq!(
            legacy_texture_name(&format!("{color}_wool"), "block"),
            Some(format!("wool_colored_{old}"))
        );
        assert_eq!(
            legacy_texture_name(&format!("{color}_glazed_terracotta"), "block"),
            Some(format!("glazed_terracotta_{old}"))
        );
    }
    assert_eq!(
        flatten_texture_name("log_big_oak_top", "block"),
        "dark_oak_log_top"
    );
    assert_eq!(
        legacy_texture_name("dark_oak_log_top", "block"),
        Some("log_big_oak_top".into())
    );
    assert_eq!(flatten_texture_name("CUSTOM", "block"), "CUSTOM");
    assert_eq!(legacy_texture_name("sulfur", "block"), None);
}

#[test]
fn format_boundaries_and_metadata_matrix() {
    let work = tempdir().unwrap();
    for target in [1, 3, 4, 7, 22, 68, 69, 88] {
        let output = work.path().join(format!("{target}.zip"));
        let report = convert_pack(
            &fixture("legacy"),
            &output,
            &ConversionOptions {
                tgt_format: target,
                ..Default::default()
            },
            |_, _| {},
        )
        .unwrap();
        let files = contents(&output);
        let meta: Value = serde_json::from_slice(&files["pack.mcmeta"]).unwrap();
        assert_eq!(meta["pack"]["pack_format"], target);
        assert_eq!(meta["pack"].get("min_format").is_some(), target >= 69);
        assert_eq!(report.lang_converted.is_empty(), target < 4);
        assert!(report.manual_todo.is_empty());
        if target >= 69 {
            assert_eq!(meta["pack"]["min_format"], json!([target, 0]));
        }
    }
}

#[test]
fn wrapped_zip_and_unsafe_entries() {
    let work = tempdir().unwrap();
    let source = work.path().join("wrapped.zip");
    let mut archive = ZipWriter::new(File::create(&source).unwrap());
    let options = SimpleFileOptions::default();
    for (name, content) in [
        ("wrapper/pack.mcmeta", "{\"pack\":{\"pack_format\":1}}"),
        (
            "wrapper/assets/minecraft/textures/blocks/stonebrick.png",
            "pixels",
        ),
        ("../escape.txt", "escape"),
        ("C:/escape.txt", "escape"),
        ("/escape.txt", "escape"),
    ] {
        archive.start_file(name, options).unwrap();
        archive.write_all(content.as_bytes()).unwrap();
    }
    archive.finish().unwrap();
    let output = work.path().join("out.zip");
    convert_pack(&source, &output, &ConversionOptions::default(), |_, _| {}).unwrap();
    let files = contents(&output);
    assert!(files.contains_key("assets/minecraft/textures/block/stone_bricks.png"));
    assert!(
        !files
            .keys()
            .any(|name| name.contains("escape") || name.starts_with("wrapper"))
    );
    assert!(!work.path().join("escape.txt").exists());
}

#[test]
fn custom_mapping_and_language_switch() {
    let work = tempdir().unwrap();
    let output = work.path().join("custom.zip");
    let options = ConversionOptions {
        convert_lang: false,
        extra_renames: vec![("custom".into(), "special".into())],
        ..Default::default()
    };
    convert_pack(&fixture("legacy"), &output, &options, |_, _| {}).unwrap();
    let files = contents(&output);
    assert!(files.contains_key("assets/minecraft/textures/block/special.png"));
    assert!(files.contains_key("assets/minecraft/textures/block/special.png.mcmeta"));
    assert!(files.keys().any(|name| name.ends_with(".lang")));
    assert!(!files.contains_key("assets/minecraft/lang/en_us.json"));
}

#[test]
fn collisions_preserve_existing_content() {
    let work = tempdir().unwrap();
    let source = work.path().join("source");
    fs::create_dir_all(source.join("assets/minecraft/textures/blocks")).unwrap();
    fs::create_dir_all(source.join("assets/minecraft/textures/block")).unwrap();
    fs::write(source.join("pack.mcmeta"), "{\"pack\":{\"pack_format\":1}}").unwrap();
    fs::write(
        source.join("assets/minecraft/textures/blocks/stonebrick.png"),
        "old",
    )
    .unwrap();
    fs::write(
        source.join("assets/minecraft/textures/block/stonebrick.png"),
        "preserved",
    )
    .unwrap();
    let output = work.path().join("out.zip");
    let report = convert_pack(&source, &output, &ConversionOptions::default(), |_, _| {}).unwrap();
    let files = contents(&output);
    assert_eq!(
        files["assets/minecraft/textures/block/stone_bricks.png"],
        b"old"
    );
    assert_eq!(
        files["fastpack/original-assets/assets/minecraft/textures/block/stonebrick.png"],
        b"preserved"
    );
    assert_eq!(report.skipped_conflict.len(), 0);
}

#[test]
fn refuses_aliases_and_keeps_source_unchanged() {
    let work = tempdir().unwrap();
    let source = work.path().join("source");
    fs::create_dir_all(source.join("assets")).unwrap();
    fs::write(source.join("pack.mcmeta"), "{\"pack\":{\"pack_format\":1}}").unwrap();
    let initial = contents(&source);
    let options = ConversionOptions {
        as_zip: false,
        ..Default::default()
    };
    for output in [&source, work.path(), &source.join("child")] {
        assert!(convert_pack(&source, output, &options, |_, _| {}).is_err());
    }
    assert_eq!(contents(&source), initial);
    let invalid = work.path().join("invalid.zip");
    fs::write(&invalid, "invalid zip").unwrap();
    assert!(convert_pack(&invalid, &work.path().join("out.zip"), &options, |_, _| {}).is_err());
}

#[test]
fn detect_probe_and_output_suggestion() {
    let info = probe_pack(&fixture("legacy"));
    assert!(info.ok);
    assert_eq!(info.src_format, Some(1));
    assert_eq!(info.file_count, 22);
    assert_eq!(info.suggested_target, LATEST_VERSION);
    assert_eq!(
        probe_pack(&fixture("modern")).suggested_target,
        LATEST_VERSION
    );
    assert_eq!(
        suggest_output(Path::new("pack.zip"), LATEST_VERSION, true),
        PathBuf::from(format!("[{LATEST_VERSION}]pack.zip"))
    );
    let work = tempdir().unwrap();
    for meta in [
        json!({"pack":{"min_format":[69,0]}}),
        json!({"pack":{"min_format":69}}),
    ] {
        fs::write(work.path().join("pack.mcmeta"), meta.to_string()).unwrap();
        assert_eq!(detect_format(work.path()), Some(69));
    }
}

#[test]
fn cli_runs_without_runtime_dependencies() {
    let work = tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_fast-pack-cli");
    let help = std::process::Command::new(binary)
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("Fast Pack")
    );
    let output = work.path().join("cli.zip");
    let source = fixture("legacy");
    let result = std::process::Command::new(binary)
        .arg(&source)
        .arg(&output)
        .args(["--to", LATEST_VERSION, "--quiet"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.exists());
    let invalid = std::process::Command::new(binary)
        .arg(&source)
        .arg(work.path().join("bad.zip"))
        .args(["--to", "missing"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(!work.path().join("bad.zip").exists());
}

#[cfg(windows)]
#[test]
fn native_gui_starts_converts_and_exits() {
    let work = tempdir().unwrap();
    let source_fixture = fixture("legacy");
    let output = work.path().join("gui.zip");
    let mut process = std::process::Command::new(env!("CARGO_BIN_EXE_FastPack"))
        .arg("--smoke-test")
        .env("FAST_PACK_SMOKE_INPUT", source_fixture.as_ref())
        .env("FAST_PACK_SMOKE_OUTPUT", &output)
        .env("FAST_PACK_SMOKE_TARGET", "26.2")
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if let Some(status) = process.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if std::time::Instant::now() > deadline {
            process.kill().unwrap();
            let _ = process.wait();
            panic!("GUI 转换超时");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(output.exists());
    let report: Value =
        serde_json::from_slice(&fs::read(output.with_extension("gui-report.json")).unwrap())
            .unwrap();
    assert_eq!(report["tgt_format"], 88);
    assert!(contents(&output).contains_key("assets/minecraft/textures/block/stone_bricks.png"));
}

