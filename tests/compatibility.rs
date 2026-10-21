use fast_pack::compat::assets;
use fast_pack::compat::atlas::{self, AtlasLayout};
use fast_pack::compat::{self, CompatibilityReport, ShaderPolicy, SkyRuntime};
use fast_pack::{ConversionOptions, convert_pack, probe_pack};
use image::{Rgba, RgbaImage};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn write(root: &Path, name: &str, bytes: impl AsRef<[u8]>) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn picture(root: &Path, name: &str, width: u32, height: u32) -> RgbaImage {
    let image = RgbaImage::from_fn(width, height, |x, y| {
        Rgba([
            (x % 251) as u8,
            (y % 239) as u8,
            ((x + 3 * y) % 253) as u8,
            255,
        ])
    });
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    image.save(path).unwrap();
    image
}

fn read(root: &Path, name: &str) -> RgbaImage {
    image::open(root.join(name)).unwrap().to_rgba8()
}
fn json_file(root: &Path, name: &str) -> Value {
    serde_json::from_slice(&fs::read(root.join(name)).unwrap()).unwrap()
}

#[test]
fn chest_uv_faces_and_hd_scale_follow_official_adapter() {
    for scale in [1, 2, 4] {
        let single = RgbaImage::from_fn(64 * scale, 64 * scale, |x, y| {
            Rgba([x as u8, y as u8, 17, 255])
        });
        let converted = assets::chest(&single, "single", false).unwrap();
        assert_eq!(
            converted.get_pixel(28 * scale, 0),
            single.get_pixel(14 * scale, 14 * scale - 1)
        );
        assert_eq!(
            converted.get_pixel(42 * scale, 14 * scale),
            single.get_pixel(28 * scale - 1, 19 * scale - 1)
        );
        let reverse = assets::chest(&converted, "single", true).unwrap();
        assert_eq!(
            reverse.get_pixel(14 * scale, 0),
            single.get_pixel(14 * scale, 0)
        );
        let double = RgbaImage::from_fn(128 * scale, 64 * scale, |x, y| {
            Rgba([x as u8, y as u8, 29, 255])
        });
        let left = assets::chest(&double, "left", false).unwrap();
        let right = assets::chest(&double, "right", false).unwrap();
        assert_eq!(left.dimensions(), (64 * scale, 64 * scale));
        assert_eq!(
            left.get_pixel(29 * scale, 0),
            double.get_pixel(29 * scale, 14 * scale - 1)
        );
        assert_eq!(
            right.get_pixel(29 * scale, 0),
            double.get_pixel(14 * scale, 14 * scale - 1)
        );
        assert_ne!(
            left.get_pixel(29 * scale, 0),
            right.get_pixel(29 * scale, 0)
        );
    }
}

#[test]
fn pattern_mask_becomes_alpha_and_preserves_base_rgb() {
    let pattern = RgbaImage::from_fn(64, 64, |x, _| {
        Rgba([if x % 2 == 0 { 255 } else { 0 }, 45, 89, 255])
    });
    let base = RgbaImage::from_pixel(64, 64, Rgba([43, 71, 113, 255]));
    let banner = assets::alpha_pattern(&pattern, Some(&base), false, false).unwrap();
    assert_eq!(*banner.get_pixel(0, 0), Rgba([43, 71, 113, 255]));
    assert_eq!(banner.get_pixel(1, 0)[3], 0);
    assert_eq!(banner.get_pixel(42, 0)[3], 0);
    let shield = assets::alpha_pattern(&pattern, Some(&base), true, false).unwrap();
    assert_eq!(shield.get_pixel(2, 2)[3], 255);
    assert_eq!(shield.get_pixel(0, 2)[3], 0);
    assert_eq!(shield.get_pixel(12, 2)[3], 0);
    let reverse = assets::alpha_pattern(&banner, None, false, true).unwrap();
    assert_eq!(*reverse.get_pixel(0, 0), Rgba([255, 255, 255, 255]));
    assert_eq!(*reverse.get_pixel(1, 0), Rgba([0, 0, 0, 255]));
}

#[test]
fn uploaded_pack_runs_all_asset_steps_without_manual_todos() {
    let work = tempdir().unwrap();
    let source = work.path().join("source");
    let output = work.path().join("converted");
    write(
        &source,
        "pack.mcmeta",
        br#"{"pack":{"pack_format":1,"description":"automatic"}}"#,
    );
    picture(
        &source,
        "assets/minecraft/textures/entity/chest/normal.png",
        64,
        64,
    );
    picture(
        &source,
        "assets/minecraft/textures/entity/chest/normal_double.png",
        128,
        64,
    );
    picture(
        &source,
        "assets/minecraft/textures/entity/banner_base.png",
        64,
        64,
    );
    picture(
        &source,
        "assets/minecraft/textures/entity/banner/cross.png",
        64,
        64,
    );
    picture(
        &source,
        "assets/minecraft/textures/entity/shield_base.png",
        64,
        64,
    );
    picture(
        &source,
        "assets/minecraft/textures/entity/shield/cross.png",
        64,
        64,
    );
    picture(&source, "assets/minecraft/textures/entity/boat.png", 64, 32);
    let dragon = picture(
        &source,
        "assets/minecraft/textures/entity/enderdragon/dragon.png",
        256,
        256,
    );
    let inventory = picture(
        &source,
        "assets/minecraft/textures/gui/container/inventory.png",
        256,
        256,
    );
    let slots = picture(
        &source,
        "assets/minecraft/textures/gui/container/gamemode_switcher.png",
        128,
        128,
    );
    let icons = picture(&source, "assets/minecraft/textures/gui/icons.png", 512, 512);
    picture(
        &source,
        "assets/minecraft/textures/gui/widgets.png",
        256,
        256,
    );
    picture(
        &source,
        "assets/minecraft/textures/misc/enchanted_item_glint.png",
        128,
        128,
    );
    picture(
        &source,
        "assets/minecraft/textures/font/unicode_page_4e.png",
        256,
        256,
    );
    picture(
        &source,
        "assets/minecraft/textures/environment/moon_phases.png",
        128,
        64,
    );
    write(&source,"assets/minecraft/font/uniform.json",br#"{"providers":[{"type":"legacy_unicode","sizes":"minecraft:font/glyph_sizes.bin","template":"minecraft:font/unicode_page_%s.png"}]}"#);
    write(&source,"assets/minecraft/models/item/diamond_sword.json",br#"{"parent":"builtin/generated","display":{"firstperson":{"rotation":[0,30,0]}},"textures":{"layer0":"items/sword_diamond"}}"#);
    write(
        &source,
        "assets/minecraft/models/block/wool_red.json",
        br#"{"parent":"block/cube_all","textures":{"all":"blocks/wool_colored_red"}}"#,
    );
    write(&source,"assets/minecraft/blockstates/wool.json",br#"{"variants":{"color=red":{"model":"wool_red"},"color=silver":{"model":"block/wool_silver","x":90}}}"#);
    let report = convert_pack(
        &source,
        &output,
        &ConversionOptions {
            as_zip: false,
            ..Default::default()
        },
        |_, _| {},
    )
    .unwrap();
    assert!(report.manual_todo.is_empty());
    assert!(!report.render().contains("手动"));
    for expected in [
        "箱子",
        "双箱子",
        "banner",
        "shield",
        "龙翼",
        "船",
        "背包",
        "26px",
        "GUI 图集",
        "附魔光效",
        "字体",
        "模型",
    ] {
        assert!(
            report
                .assets
                .converted
                .iter()
                .any(|value| value.contains(expected)),
            "{expected}"
        );
    }
    assert_eq!(
        read(&output, "assets/minecraft/textures/entity/boat/oak.png").dimensions(),
        (128, 64)
    );
    let migrated_dragon = read(
        &output,
        "assets/minecraft/textures/entity/enderdragon/dragon.png",
    );
    assert_eq!(migrated_dragon.get_pixel(56, 88)[3], 0);
    assert_eq!(
        migrated_dragon.get_pixel(112, 200),
        dragon.get_pixel(112, 200)
    );
    let migrated_inventory = read(
        &output,
        "assets/minecraft/textures/gui/container/inventory.png",
    );
    assert_eq!(
        migrated_inventory.get_pixel(0, 198),
        inventory.get_pixel(0, 166)
    );
    assert_eq!(
        migrated_inventory.get_pixel(31, 198),
        inventory.get_pixel(119, 166)
    );
    let small = read(
        &output,
        "assets/minecraft/textures/gui/sprites/container/inventory/effect_background_small.png",
    );
    assert_eq!(small.dimensions(), (32, 32));
    assert_eq!(small.get_pixel(0, 0), inventory.get_pixel(0, 166));
    let migrated_slots = read(
        &output,
        "assets/minecraft/textures/gui/container/gamemode_switcher.png",
    );
    assert_eq!(migrated_slots.get_pixel(25, 100), slots.get_pixel(24, 99));
    assert_eq!(migrated_slots.get_pixel(26, 75), slots.get_pixel(25, 75));
    assert_eq!(migrated_slots.get_pixel(0, 0), slots.get_pixel(0, 0));
    let crosshair = read(
        &output,
        "assets/minecraft/textures/gui/sprites/hud/crosshair.png",
    );
    assert_eq!(crosshair.dimensions(), (30, 30));
    assert_eq!(crosshair.get_pixel(29, 29), icons.get_pixel(29, 29));
    assert!(
        output
            .join("assets/minecraft/textures/misc/enchanted_glint_entity.png")
            .exists()
    );
    assert!(
        output
            .join("assets/minecraft/textures/misc/enchanted_glint_item.png")
            .exists()
    );
    let model = json_file(&output, "assets/minecraft/models/item/diamond_sword.json");
    assert_eq!(model["parent"], "minecraft:item/handheld");
    assert_eq!(
        model["display"]["firstperson_righthand"]["rotation"],
        json!([0.0, 75.0, 0.0])
    );
    assert_eq!(
        json_file(&output, "assets/minecraft/items/diamond_sword.json")["model"]["type"],
        "minecraft:model"
    );
    assert_eq!(
        json_file(&output, "assets/minecraft/blockstates/red_wool.json")["variants"][""]["model"],
        "minecraft:block/red_wool"
    );
    let model = json_file(&output, "assets/minecraft/models/block/red_wool.json");
    assert_eq!(model["parent"], "block/cube_all");
    assert_eq!(model["textures"]["all"], "block/red_wool");
    let state = json_file(&output, "assets/minecraft/blockstates/wool.json");
    assert_eq!(
        state["variants"]["color=silver"]["model"],
        "minecraft:block/light_gray_wool"
    );
    assert_eq!(state["variants"]["color=silver"]["x"], 90);
    let font = json_file(&output, "assets/minecraft/font/uniform.json");
    assert_eq!(font["providers"][0]["type"], "bitmap");
    assert_eq!(
        font["providers"][0]["chars"][0]
            .as_str()
            .unwrap()
            .chars()
            .next(),
        Some('一')
    );
    assert_eq!(font["providers"][1]["type"], "reference");
    assert!(
        output
            .join("fastpack/original-assets/assets/minecraft/textures/entity/chest/normal.png")
            .exists()
    );
    assert_eq!(
        read(
            &source,
            "assets/minecraft/textures/entity/enderdragon/dragon.png"
        ),
        dragon
    );
}

#[test]
fn gui_downgrade_keeps_atlas_regions_not_covered_by_sprites() {
    let work = tempdir().unwrap();
    let root = work.path();
    picture(root, "assets/minecraft/textures/gui/icons.png", 256, 256);
    let sprite = picture(
        root,
        "assets/minecraft/textures/gui/sprites/hud/crosshair.png",
        15,
        15,
    );
    let before = read(root, "assets/minecraft/textures/gui/icons.png");
    assets::migrate(root, 55, 15).unwrap();
    let after = read(root, "assets/minecraft/textures/gui/icons.png");
    assert_eq!(after.get_pixel(255, 255), before.get_pixel(255, 255));
    assert_eq!(after.get_pixel(14, 14), sprite.get_pixel(14, 14));
}

#[test]
fn sky_cube_orientation_is_bijective_in_both_dimensions() {
    for size in [1, 3, 16] {
        let source = RgbaImage::from_fn(3 * size, 2 * size, |x, y| {
            Rgba([x as u8, y as u8, (x + 100 * y) as u8, 255])
        });
        for overworld in [true, false] {
            for layout in [AtlasLayout::FabricSkyboxes, AtlasLayout::Nuit] {
                let target =
                    atlas::remap(&source, AtlasLayout::Optifine, layout, overworld).unwrap();
                let roundtrip =
                    atlas::remap(&target, layout, AtlasLayout::Optifine, overworld).unwrap();
                assert_eq!(roundtrip, source);
            }
        }
    }
    assert!(atlas::dimensions(&RgbaImage::new(32, 32)).is_err());
}

#[test]
fn java_properties_support_continuations_unicode_and_surrogate_pairs() {
    let parsed=compat::properties::parse(b"# comment\nsource : ./sky.png\nbiomes=plains \\\n forest\nname=\\u5929\\u7a7a\\ud83c\\udf0c\nescaped\\ key=one\\:two\n").unwrap();
    assert_eq!(parsed["biomes"], "plains forest");
    assert_eq!(parsed["name"], "天空🌌");
    assert_eq!(parsed["escaped key"], "one:two");
    assert_eq!(
        compat::properties::parse(compat::properties::write(&parsed).as_bytes()).unwrap(),
        parsed
    );
}

fn sky_pack(root: &Path, rotate: bool) -> RgbaImage {
    write(root, "pack.mcmeta", br#"{"pack":{"pack_format":1}}"#);
    let image = picture(
        root,
        "assets/minecraft/mcpatcher/sky/world0/day.png",
        48,
        32,
    );
    write(
        root,
        "assets/minecraft/mcpatcher/sky/world0/sky1.properties",
        format!(
            "source=./day.png\nrotate={rotate}\nstartFadeIn=06:00\nendFadeIn=07:00\nstartFadeOut=18:00\nendFadeOut=19:00\nblend=add\nweather=clear\n"
        ),
    );
    image
}

#[test]
fn legacy_sky_to_1201_rewrites_paths_automatically() {
    let work = tempdir().unwrap();
    let source = work.path().join("source");
    let output = work.path().join("output");
    let image = sky_pack(&source, true);
    assert_eq!(probe_pack(&source).features.optifine_sky, 1);
    let report = convert_pack(
        &source,
        &output,
        &ConversionOptions {
            tgt_format: 15,
            as_zip: false,
            ..Default::default()
        },
        |_, _| {},
    )
    .unwrap();
    assert_eq!(report.compatibility.sky_layers, 1);
    let properties = compat::properties::parse(&fs::read(
        output
            .join("assets/minecraft/optifine/sky/world0/sky1.properties"),
    )
    .unwrap())
    .unwrap();
    assert_eq!(properties["rotate"], "true");
    let (namespace, name) = properties["source"].split_once(':').unwrap();
    assert_eq!(namespace, "minecraft");
    assert_eq!(
        read(&output, &format!("assets/{namespace}/{name}")),
        image
    );
    assert!(!output
        .join("assets/minecraft/mcpatcher/sky/world0/sky1.properties")
        .exists());
    assert!(output
        .join("fastpack/original-sky/assets/minecraft/mcpatcher/sky/world0/sky1.properties")
        .exists());
}

#[test]
fn static_sky_uses_nuit_schema_and_can_return_to_legacy() {
    let work = tempdir().unwrap();
    let source = work.path().join("source");
    let modern = work.path().join("modern");
    let legacy = work.path().join("legacy");
    let image = sky_pack(&source, false);
    let report = convert_pack(
        &source,
        &modern,
        &ConversionOptions {
            as_zip: false,
            ..Default::default()
        },
        |_, _| {},
    )
    .unwrap();
    assert_eq!(report.compatibility.sky_layers, 1);
    let definition = json_file(&modern, "assets/nuit/sky/fastpack_layer_0.json");
    assert_eq!(definition["schemaVersion"], 1);
    assert_eq!(definition["type"], "square-textured");
    assert_eq!(definition["properties"]["fade"]["keyFrames"]["0"], 0.0);
    assert_eq!(
        definition["conditions"]["dimensions"]["entries"],
        json!(["minecraft:overworld"])
    );
    let texture = definition["texture"].as_str().unwrap();
    assert!(texture.starts_with("fastpack:textures/sky/converted_"));
    let (namespace, name) = texture.split_once(':').unwrap();
    let converted = read(&modern, &format!("assets/{namespace}/{name}"));
    assert_eq!(
        atlas::remap(&converted, AtlasLayout::Nuit, AtlasLayout::Optifine, true).unwrap(),
        image
    );
    convert_pack(
        &modern,
        &legacy,
        &ConversionOptions {
            tgt_format: 1,
            as_zip: false,
            ..Default::default()
        },
        |_, _| {},
    )
    .unwrap();
    let returned = compat::properties::parse(&fs::read(
        legacy
            .join("assets/minecraft/mcpatcher/sky/world0/sky1.properties"),
    )
    .unwrap())
    .unwrap();
    let (namespace, name) = returned["source"].split_once(':').unwrap();
    assert_eq!(read(&legacy, &format!("assets/{namespace}/{name}")), image);
    assert!(!legacy.join("assets/nuit/sky/fastpack_layer_0.json").exists());
}

#[test]
fn unrecognized_sky_definition_survives_conversion_and_reconversion() {
    let work = tempdir().unwrap();
    let source = work.path().join("source");
    let modern = work.path().join("modern");
    let legacy = work.path().join("legacy");
    sky_pack(&source, false);
    let custom = br#"{"schemaVersion":2,"type":"procedural-test","unknown":123}"#;
    write(&source, "assets/custom/sky/special.json", custom);
    let report = convert_pack(
        &source,
        &modern,
        &ConversionOptions {
            as_zip: false,
            ..Default::default()
        },
        |_, _| {},
    )
    .unwrap();
    assert!(!report.compatibility.warnings.is_empty());
    assert_eq!(
        json_file(&modern, "fastpack/compatibility.json")["warnings"],
        serde_json::to_value(&report.compatibility.warnings).unwrap()
    );
    assert_eq!(
        fs::read(modern.join("assets/custom/sky/special.json")).unwrap(),
        custom
    );
    convert_pack(
        &modern,
        &legacy,
        &ConversionOptions {
            tgt_format: 1,
            as_zip: false,
            ..Default::default()
        },
        |_, _| {},
    )
    .unwrap();
    assert_eq!(
        fs::read(legacy.join("assets/custom/sky/special.json")).unwrap(),
        custom
    );
}

#[test]
fn sky_texture_path_traversal_is_not_followed() {
    let work = tempdir().unwrap();
    let root = work.path();
    sky_pack(root, false);
    write(
        root,
        "assets/minecraft/mcpatcher/sky/world0/sky1.properties",
        b"source=../../../../outside.png\n",
    );
    let warnings = compat::sky::prepare(root).unwrap();
    assert!(warnings.iter().any(|warning| warning.contains("越界")));
    assert!(!root.join("fastpack/canonical-sky/layers.json").exists());
}

#[test]
fn cross_generation_shader_fallback_is_explicit_and_backed_up() {
    let work = tempdir().unwrap();
    let root = work.path();
    let shader=b"#version 150\nuniform mat4 ModelViewMat;\nvoid main(){gl_Position=ModelViewMat*vec4(1.0);}\n";
    write(root, "assets/minecraft/shaders/core/position.vsh", shader);
    let mut report = CompatibilityReport::default();
    compat::shaders::migrate(root, 15, 88, ShaderPolicy::Adaptive, &mut report).unwrap();
    assert_eq!(report.disabled_shaders.len(), 1);
    assert!(
        !root
            .join("assets/minecraft/shaders/core/position.vsh")
            .exists()
    );
    assert_eq!(
        fs::read(root.join("fastpack/original-shaders/assets/minecraft/shaders/core/position.vsh"))
            .unwrap(),
        shader
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("原着色器效果未保留"))
    );
}

#[test]
fn exact_shader_variant_is_selected_without_player_configuration() {
    let work = tempdir().unwrap();
    let root = work.path();
    write(root, "assets/minecraft/shaders/core/position.vsh", b"old");
    write(
        root,
        "fastpack/variants/26.2/assets/minecraft/shaders/core/position.vsh",
        b"#version 330\nnew",
    );
    let report = compat::migrate(
        root,
        15,
        88,
        "26.2",
        SkyRuntime::Auto,
        ShaderPolicy::Adaptive,
    )
    .unwrap();
    assert_eq!(report.selected_variant.as_deref(), Some("26.2"));
    assert!(report.disabled_shaders.is_empty());
    assert_eq!(
        fs::read(root.join("assets/minecraft/shaders/core/position.vsh")).unwrap(),
        b"#version 330\nnew"
    );
}

#[test]
fn shader_cache_recovers_original_when_returning_to_source_version() {
    let work = tempdir().unwrap();
    let root = work.path();
    let shader = b"#version 150\noriginal body";
    write(root, "assets/minecraft/shaders/core/position.vsh", shader);
    let mut report = CompatibilityReport::default();
    compat::shaders::migrate(root, 15, 88, ShaderPolicy::Adaptive, &mut report).unwrap();
    let restored = compat::migrate(
        root,
        88,
        15,
        "1.20.1",
        SkyRuntime::Auto,
        ShaderPolicy::Adaptive,
    )
    .unwrap();
    assert!(restored.disabled_shaders.is_empty());
    assert_eq!(
        fs::read(root.join("assets/minecraft/shaders/core/position.vsh")).unwrap(),
        shader
    );
}

#[test]
fn model_only_packs_migrate_default_texture_refs_and_downgrade_color_states() {
    let work = tempdir().unwrap();
    let source = work.path().join("source");
    let output = work.path().join("old");
    write(&source, "pack.mcmeta", br#"{"pack":{"pack_format":15}}"#);
    write(
        &source,
        "assets/minecraft/models/block/red_wool.json",
        br#"{"parent":"minecraft:block/cube_all","textures":{"all":"minecraft:block/red_wool"}}"#,
    );
    write(
        &source,
        "assets/minecraft/blockstates/red_wool.json",
        br#"{"variants":{"":{"model":"minecraft:block/red_wool"}}}"#,
    );
    let report = convert_pack(
        &source,
        &output,
        &ConversionOptions {
            tgt_format: 1,
            as_zip: false,
            ..Default::default()
        },
        |_, _| {},
    )
    .unwrap();
    assert!(!output
        .join("assets/minecraft/models/block/red_wool.json")
        .exists());
    assert!(!output
        .join("assets/minecraft/blockstates/red_wool.json")
        .exists());
    let model = json_file(
        &output,
        "fastpack/modern-defaults/assets/minecraft/models/block/red_wool.json",
    );
    assert_eq!(model["parent"], "minecraft:block/cube_all");
    assert_eq!(model["textures"]["all"], "minecraft:block/red_wool");
    assert_eq!(
        json_file(
            &output,
            "fastpack/modern-defaults/assets/minecraft/blockstates/red_wool.json"
        )["variants"][""]["model"],
        "minecraft:block/red_wool"
    );
    assert!(
        report
            .assets
            .converted
            .iter()
            .any(|value| value.contains("原版资源"))
    );
}

#[test]
fn native_fsb_static_sky_downgrades_without_requesting_a_runtime() {
    let work = tempdir().unwrap();
    let source = work.path().join("source");
    let output = work.path().join("legacy");
    write(&source, "pack.mcmeta", br#"{"pack":{"pack_format":15}}"#);
    picture(&source, "assets/test/textures/sky/day.png", 48, 32);
    write(&source,"assets/test/sky/day.json",br#"{"schemaVersion":2,"type":"single-sprite-square-textured","texture":"test:textures/sky/day.png","properties":{"fade":{"alwaysOn":true},"rotation":{"rotationSpeedX":0,"rotationSpeedY":0,"rotationSpeedZ":0}}}"#);
    for format in [1, 6] {
        let destination = output.with_extension(format.to_string());
        let report = convert_pack(
            &source,
            &destination,
            &ConversionOptions {
                tgt_format: format,
                as_zip: false,
                ..Default::default()
            },
            |_, _| {},
        )
        .unwrap();
        assert_eq!(report.compatibility.sky_layers, 1);
        let base = if format < 4 { "mcpatcher" } else { "optifine" };
        assert!(
            destination
                .join(format!(
                    "assets/minecraft/{base}/sky/world0/sky1.properties"
                ))
                .exists()
        );
        assert!(!destination.join("assets/test/sky/day.json").exists());
    }
}

#[test]
fn unknown_blockstate_values_are_never_used_as_output_paths() {
    let work = tempdir().unwrap();
    let root = work.path().join("pack");
    write(&root,"assets/minecraft/blockstates/wool.json",br#"{"variants":{"color=../../../../escaped":{"model":"wool_red"},"color=red":{"model":"wool_red"}}}"#);
    let report = assets::migrate(&root, 1, 88).unwrap();
    assert!(!work.path().join("escaped_wool.json").exists());
    assert!(!report.warnings.is_empty());
    assert!(
        root.join("assets/minecraft/blockstates/red_wool.json")
            .exists()
    );
}

#[test]
fn cached_sky_and_shader_variant_paths_are_validated_before_writing() {
    let work = tempdir().unwrap();
    let root = work.path();
    write(root,"fastpack/canonical-sky/layers.json",br#"[{"name":"day","world":"../../outside","texture":"assets/test/day.png","layout":"optifine","properties":{},"original":"assets/test/sky/day.properties","decoration":false}]"#);
    assert!(
        compat::sky::migrate(
            root,
            15,
            SkyRuntime::Auto,
            &mut CompatibilityReport::default()
        )
        .is_err()
    );
    assert!(
        compat::shaders::select_variant(
            root,
            "../outside",
            88,
            &mut CompatibilityReport::default()
        )
        .is_err()
    );
}
