//! Regression tests: malformed (non-object) JSON in user packs must produce
//! a clean error (or a graceful skip) instead of a panic. Part 2 of BUGFIX-PLAN.
use fast_pack::builtin_overlays::{Feature, Settings};
use fast_pack::compat::custom_items::Rule;
use fast_pack::{convert_pack, ConversionOptions};
use tempfile::tempdir;

const MCMETA_1: &[u8] = br#"{"pack":{"pack_format":1,"description":"t"}}"#;
const MCMETA_2: &[u8] = br#"{"pack":{"pack_format":2,"description":"t"}}"#;

fn put(source: &std::path::Path, relative: &str, bytes: &[u8]) {
    let path = source.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn assert_object_error<T: std::fmt::Debug>(result: Result<T, anyhow::Error>) {
    let error = result.expect_err("convert_pack must fail cleanly on malformed JSON, not panic");
    let message = error.to_string();
    assert!(
        message.contains("对象"),
        "error must mention 对象, got: {message}"
    );
}

/// Scenario 1 (guard 2c): a non-object model file must be skipped gracefully.
/// models() guards with `continue`, so the conversion completes without panic
/// and the untouched file is carried into the output.
#[test]
fn malformed_bow_model_is_skipped_without_panic() {
    let work = tempdir().unwrap();
    let source = work.path().join("source");
    put(&source, "pack.mcmeta", MCMETA_1);
    put(&source, "assets/minecraft/models/item/bow.json", b"[]");
    let output = tempdir().unwrap();
    let destination = output.path().join("out");
    let options = ConversionOptions {
        tgt_format: 2,
        as_zip: false,
        ..Default::default()
    };
    convert_pack(&source, &destination, &options, |_, _| {})
        .expect("non-object model must be skipped, not panic");
    assert!(
        destination
            .join("assets/minecraft/models/item/bow.json")
            .exists(),
        "skipped model must still be copied to the output"
    );
}

/// Scenario 2 (guards 2d/2e): a non-object item definition with a builtin
/// overlay enabled on 1.21.4+ must return Err mentioning 对象.
#[test]
fn malformed_item_definition_returns_object_error_with_builtin_overlay() {
    let work = tempdir().unwrap();
    let source = work.path().join("source");
    put(&source, "pack.mcmeta", MCMETA_1);
    for sword in [
        "wooden_sword",
        "stone_sword",
        "iron_sword",
        "golden_sword",
        "diamond_sword",
        "netherite_sword",
    ] {
        put(
            &source,
            &format!("assets/minecraft/items/{sword}.json"),
            b"[]",
        );
    }
    let output = tempdir().unwrap();
    let destination = output.path().join("out");
    let options = ConversionOptions {
        tgt_format: 46,
        as_zip: false,
        builtin_overlays: Settings {
            features: vec![Feature::Sharpness],
            ..Default::default()
        },
        ..Default::default()
    };
    assert_object_error(convert_pack(&source, &destination, &options, |_, _| {}));
}

/// Scenario 3 (guard 2a): a non-object model referenced by a custom_items
/// rule must be handled gracefully — the model self-check (model_integrity)
/// backs the file up and falls back to the native model before custom_items
/// reads it, and the protocol-item step records a warning mentioning 对象
/// instead of panicking.
#[test]
fn malformed_custom_item_model_degrades_gracefully() {
    let work = tempdir().unwrap();
    let source = work.path().join("source");
    put(&source, "pack.mcmeta", MCMETA_1);
    put(
        &source,
        "assets/minecraft/models/item/diamond_sword.json",
        b"[]",
    );
    let output = tempdir().unwrap();
    let destination = output.path().join("out");
    let options = ConversionOptions {
        tgt_format: 46,
        as_zip: false,
        custom_items: vec![Rule {
            item: "diamond_sword".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let report = convert_pack(&source, &destination, &options, |_, _| {})
        .expect("malformed custom-item model must degrade gracefully, not panic");
    assert!(
        report
            .assets
            .warnings
            .iter()
            .any(|warning| warning.contains("对象")),
            "expected a warning mentioning 对象, got: {:?}",
            report.assets.warnings
    );
}

/// Scenario 4 (Part 3): a boat texture whose dimensions cannot express the
/// legacy UV must degrade to a warning instead of aborting the conversion.
#[test]
fn malformed_boat_texture_degrades_to_warning() {
    let work = tempdir().unwrap();
    let source = work.path().join("source");
    put(&source, "pack.mcmeta", MCMETA_2);
    let boat = source.join("assets/minecraft/textures/entity/boat/oak.png");
    std::fs::create_dir_all(boat.parent().unwrap()).unwrap();
    let image = image::RgbaImage::from_pixel(64, 32, image::Rgba([255, 0, 0, 255]));
    image
        .save_with_format(&boat, image::ImageFormat::Png)
        .unwrap();
    let output = tempdir().unwrap();
    let destination = output.path().join("out");
    let options = ConversionOptions {
        tgt_format: 1,
        as_zip: false,
        ..Default::default()
    };
    let report = convert_pack(&source, &destination, &options, |_, _| {})
        .expect("invalid boat UV must degrade to a warning, not abort");
    assert!(
        report
            .assets
            .warnings
            .iter()
            .any(|warning| warning.contains("现代船 UV 比例无效")),
        "expected a boat UV warning, got: {:?}",
        report.assets.warnings
    );
    assert!(
        destination
            .join("assets/minecraft/textures/entity/boat/oak.png")
            .exists(),
        "original boat texture must remain in the output"
    );
}
