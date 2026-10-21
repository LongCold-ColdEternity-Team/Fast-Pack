use fast_pack::{ConversionOptions, compat, convert_pack};
use image::{Rgba, RgbaImage};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use zip::{ZipWriter, write::SimpleFileOptions};

const SKY: &str = "assets/minecraft/mcpatcher/sky/world0";

fn fixture(root: &Path) -> (RgbaImage, Vec<u8>) {
    fs::create_dir_all(root.join(SKY)).unwrap();
    fs::write(root.join("pack.mcmeta"), br#"{"pack":{"pack_format":1}}"#).unwrap();
    let image = RgbaImage::from_fn(48, 32, |x, y| Rgba([x as u8, y as u8, 77, 255]));
    image.save(root.join(SKY).join("sky1.png")).unwrap();
    fs::write(
        root.join(SKY).join("sky1.properties"),
        b"source=./sky1.png\nrotate=false\n",
    )
    .unwrap();
    // AppleDouble resource forks have the same suffix as their real files.
    let mut metadata = vec![0; 4096];
    metadata[..8].copy_from_slice(b"\0\x05\x16\x07\0\x02\0\0");
    metadata[8..16].copy_from_slice(b"Mac OS X");
    for name in ["._sky1.properties", "._sky1.png", "._unused.json"] {
        fs::write(root.join(SKY).join(name), &metadata).unwrap();
    }
    (image, metadata)
}

#[test]
fn appledouble_sky_metadata_is_preserved_without_becoming_sky_layers() {
    let work = tempfile::tempdir().unwrap();
    let source = work.path().join("source");
    let (expected, metadata) = fixture(&source);
    let archive = work.path().join("source.zip");
    let mut zip = ZipWriter::new(File::create(&archive).unwrap());
    for entry in walkdir::WalkDir::new(&source).min_depth(1) {
        let entry = entry.unwrap();
        if entry.file_type().is_file() {
            zip.start_file(
                entry
                    .path()
                    .strip_prefix(&source)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
                SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(&fs::read(entry.path()).unwrap()).unwrap();
        }
    }
    zip.finish().unwrap();

    for (index, input) in [&source, &archive].into_iter().enumerate() {
        let preview = compat::sky::preview(input).unwrap().unwrap();
        assert_eq!(preview.names.len(), 1);
        let output = work.path().join(format!("converted-{index}"));
        let report = convert_pack(
            input,
            &output,
            &ConversionOptions {
                tgt_format: 15,
                target_version: Some("1.20.1".into()),
                as_zip: false,
                auto_client_setup: false,
                ..Default::default()
            },
            |_, _| {},
        )
        .unwrap();
        assert_eq!(report.compatibility.sky_layers, 1);
        assert!(
            report
                .compatibility
                .sky_converted
                .iter()
                .any(|note| note.contains("AppleDouble"))
        );
        assert_eq!(
            image::open(output.join(SKY).join("sky1.png"))
                .unwrap()
                .to_rgba8(),
            expected
        );
        let props = compat::properties::parse(
            &fs::read(output.join("assets/minecraft/optifine/sky/world0/sky1.properties")).unwrap(),
        )
        .unwrap();
        assert_eq!(props["source"], "minecraft:mcpatcher/sky/world0/sky1.png");
        for name in ["._sky1.properties", "._sky1.png", "._unused.json"] {
            assert_eq!(fs::read(source.join(SKY).join(name)).unwrap(), metadata);
            assert_eq!(fs::read(output.join(SKY).join(name)).unwrap(), metadata);
        }
    }
}

#[test]
fn real_sky_configs_with_sidecar_like_names_are_not_filtered_by_name() {
    let work = tempfile::tempdir().unwrap();
    fixture(work.path());
    fs::write(
        work.path().join(SKY).join("._sky1.properties"),
        b"source=./sky1.png\nrotate=false\n",
    )
    .unwrap();
    assert_eq!(
        compat::sky::preview(work.path())
            .unwrap()
            .unwrap()
            .names
            .len(),
        2
    );
}

#[test]
fn inactive_sky_rules_are_backed_up_and_reported_once_across_reconversion() {
    let work = tempfile::tempdir().unwrap();
    let source = work.path().join("source");
    fixture(&source);
    let missing = b"source=./unused.png\nrotate=false\n";
    for name in ["sky2.properties", "sky48.properties"] {
        fs::write(source.join(SKY).join(name), missing).unwrap();
    }
    let options = ConversionOptions {
        tgt_format: 15,
        target_version: Some("1.20.1".into()),
        as_zip: false,
        auto_client_setup: false,
        ..Default::default()
    };
    let first = work.path().join("first");
    let second = work.path().join("second");
    for (input, output) in [(&source, &first), (&first, &second)] {
        let report = convert_pack(input, output, &options, |_, _| {}).unwrap();
        assert_eq!(report.compatibility.sky_layers, 1);
        assert!(report.compatibility.warnings.is_empty());
        let summaries: Vec<_> = report
            .compatibility
            .sky_converted
            .iter()
            .filter(|note| note.contains("天空配置整理"))
            .collect();
        assert_eq!(summaries.len(), 1);
        assert!(summaries[0].contains("2 条") && summaries[0].contains("1 个路径"));
        assert!(!summaries[0].contains("AppData\\Local\\Temp"));
        for name in ["sky2.properties", "sky48.properties"] {
            assert!(!output.join(SKY).join(name).exists());
            assert_eq!(
                fs::read(output.join("fastpack/original-sky").join(SKY).join(name)).unwrap(),
                missing
            );
        }
        let notes: serde_json::Value = serde_json::from_slice(
            &fs::read(output.join("fastpack/canonical-sky/scan.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(notes["inactive"].as_array().unwrap().len(), 2);
        assert!(
            notes["inactive"]
                .as_array()
                .unwrap()
                .iter()
                .all(|entry| entry["texture"] == format!("{SKY}/unused.png"))
        );
        assert!(compat::sky::preview(output).unwrap().is_some());
    }
    for name in ["sky2.properties", "sky48.properties"] {
        assert_eq!(fs::read(source.join(SKY).join(name)).unwrap(), missing);
    }
}

#[test]
fn shared_sky_textures_keep_layer_order_timing_and_blend() {
    let work = tempfile::tempdir().unwrap();
    let source = work.path().join("source");
    fixture(&source);
    fs::remove_file(source.join(SKY).join("sky1.properties")).unwrap();
    let names = [2, 3, 4, 5, 6, 35, 36, 38, 48];
    let mut expected = Vec::new();
    for (index, number) in names.into_iter().enumerate() {
        let texture = format!("image{}.png", index % 6);
        RgbaImage::from_pixel(48, 32, Rgba([index as u8 % 6 * 30, 80, 140, 255]))
            .save(source.join(SKY).join(&texture))
            .unwrap();
        let text = format!(
            "source=./{texture}\nstartFadeIn=17:30\nendFadeIn=20:00\nendFadeOut=6:10\nblend={}\nrotate=true\nspeed={}\naxis=0.0 -0.2 0.0\n",
            if index % 2 == 0 { "add" } else { "replace" },
            index + 1
        );
        fs::write(
            source.join(SKY).join(format!("sky{number}.properties")),
            &text,
        )
        .unwrap();
        expected.push(compat::properties::parse(text.as_bytes()).unwrap());
    }
    let output = work.path().join("output");
    let report = convert_pack(
        &source,
        &output,
        &ConversionOptions {
            tgt_format: 15,
            target_version: Some("1.20.1".into()),
            as_zip: false,
            auto_client_setup: false,
            ..Default::default()
        },
        |_, _| {},
    )
    .unwrap();
    assert_eq!(report.compatibility.sky_layers, 9);
    for (index, original) in expected.iter().enumerate() {
        let path = output.join(format!(
            "assets/minecraft/optifine/sky/world0/sky{}.properties",
            index + 1
        ));
        let actual = compat::properties::parse(&fs::read(path).unwrap()).unwrap();
        for (key, value) in original {
            if key != "source" {
                assert_eq!(&actual[key], value, "layer {index}: {key}");
            }
        }
        let (namespace, texture) = actual["source"].split_once(':').unwrap();
        let actual = image::open(output.join("assets").join(namespace).join(texture))
            .unwrap()
            .to_rgba8();
        let expected = image::open(
            source
                .join(SKY)
                .join(original["source"].trim_start_matches("./")),
        )
        .unwrap()
        .to_rgba8();
        assert_eq!(actual, expected);
    }
}

#[test]
fn cached_inactive_paths_cannot_move_files_outside_the_pack() {
    let work = tempfile::tempdir().unwrap();
    let source = work.path().join("source");
    fixture(&source);
    compat::sky::prepare(&source).unwrap();
    let outside = work.path().join("keep.properties");
    fs::write(&outside, b"keep").unwrap();
    let notes = serde_json::json!({"inactive":[{"configuration":"../keep.properties","texture":"assets/missing.png"}]});
    fs::write(
        source.join("fastpack/canonical-sky/scan.json"),
        serde_json::to_vec(&notes).unwrap(),
    )
    .unwrap();
    let mut report = compat::CompatibilityReport::default();
    assert!(compat::sky::migrate(&source, 15, compat::SkyRuntime::Auto, &mut report).is_err());
    assert_eq!(fs::read(outside).unwrap(), b"keep");
}
