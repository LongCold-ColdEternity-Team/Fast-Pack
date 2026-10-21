use super::*;
fn settings(features: &[Feature]) -> Settings {
    Settings {
        features: features.to_vec(),
        ..Default::default()
    }
}

#[test]
fn name_background_defaults_and_saved_color_survive_old_settings_and_reconversion() {
    let old: Settings = serde_json::from_str(r#"{"features":["highlight-names"]}"#).unwrap();
    assert_eq!(old.name_background, DEFAULT_NAME_BACKGROUND);
    assert!(!old.name_background_rainbow);
    let selected = Settings {
        name_background: [17, 102, 204],
        name_background_rainbow: true,
        ..old
    };
    let restored: Settings =
        serde_json::from_slice(&serde_json::to_vec(&selected).unwrap()).unwrap();
    assert_eq!(restored, selected);
    let temp = tempfile::tempdir().unwrap();
    let mut report = AssetReport::default();
    apply(temp.path(), "1.20.1", &selected, &mut report).unwrap();
    assert_eq!(
        effective(temp.path(), &Settings::default()).unwrap(),
        selected
    );
    for stem in [
        "rendertype_text",
        "rendertype_text_see_through",
        "rendertype_text_background",
        "rendertype_text_background_see_through",
    ] {
        let program = read(
            temp.path(),
            &format!("assets/minecraft/shaders/core/{stem}.json"),
        );
        assert_eq!(
            program["uniforms"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|uniform| uniform["name"] == "GameTime")
                .count(),
            1
        );
    }
    let mut newer = AssetReport::default();
    apply(temp.path(), "1.21.10", &selected, &mut newer).unwrap();
    assert!(
        newer
            .warnings
            .iter()
            .any(|note| note.contains("固定背景颜色"))
    );
    let shader = std::fs::read_to_string(
        temp.path()
            .join("assets/minecraft/shaders/core/rendertype_text_background_see_through.fsh"),
    )
    .unwrap();
    assert!(!shader.contains("uniform float GameTime"));
    assert!(shader.contains("0.066667, 0.400000, 0.800000"));
}

#[test]
fn removed_appearance_settings_keep_other_features_and_provenance() {
    let mut legacy = serde_json::to_value(settings(&Feature::ALL)).unwrap();
    legacy["features"]
        .as_array_mut()
        .unwrap()
        .insert(2, json!("heypixel-pack"));
    let restored: Settings = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(restored, settings(&Feature::ALL));
    assert!(
        !serde_json::to_string(&restored)
            .unwrap()
            .contains("heypixel-pack")
    );
    assert!(
        !serde_json::from_str::<Settings>(r#"{"features":["heypixel-pack"]}"#)
            .unwrap()
            .active()
    );
    assert!(serde_json::from_str::<Settings>(r#"{"features":["unknown-feature"]}"#).is_err());

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let path = "assets/minecraft/items/diamond_sword.json";
    let base = json!({"model":{"type":"minecraft:model","model":"minecraft:item/diamond_sword"}});
    let generated = json!({"model":{"type":"minecraft:empty"},"fastpack_builtin_hud":base});
    let bytes = serde_json::to_vec(&generated).unwrap();
    legacy["generated"] = json!({path:digest(&bytes)});
    write(root, path, &bytes).unwrap();
    write(root, STATE, &serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert_eq!(effective(root, &Settings::default()).unwrap(), restored);
    assert!(prepare(root).unwrap().is_empty());
    assert_eq!(read(root, path), base);
}

#[test]
fn all_builtin_features_preserve_authored_appearance_assets() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let assets = [
        "assets/minecraft/textures/gui/container/inventory.png",
        "assets/minecraft/textures/block/stone.png",
        "assets/minecraft/textures/item/diamond_sword.png",
        "assets/minecraft/font/default.json",
        "assets/minecraft/sounds.json",
        "assets/heypixel/models/cosmetic/example.json",
    ];
    for name in assets {
        write(root, name, b"authored appearance").unwrap();
    }
    apply(
        root,
        "1.20.1",
        &settings(&Feature::ALL),
        &mut AssetReport::default(),
    )
    .unwrap();
    for name in assets {
        assert_eq!(
            std::fs::read(root.join(name)).unwrap(),
            b"authored appearance",
            "{name}"
        );
    }
}

#[test]
fn reconversion_unwraps_only_owned_layers_and_preserves_edited_graphs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut report = AssetReport::default();
    let settings = settings(&[Feature::Sharpness]);
    apply(root, "1.21.10", &settings, &mut report).unwrap();
    let original = read(root, "assets/minecraft/items/diamond_sword.json");
    assert_eq!(effective(root, &Settings::default()).unwrap(), settings);
    prepare(root).unwrap();
    assert_eq!(
        read(root, "assets/minecraft/items/diamond_sword.json"),
        original["fastpack_builtin_hud"]
    );
    apply(root, "1.21.10", &settings, &mut report).unwrap();
    assert_eq!(
        read(root, "assets/minecraft/items/diamond_sword.json"),
        original
    );
    let mut edited = original.clone();
    edited["hand_animation_on_swap"] = json!(false);
    write(
        root,
        "assets/minecraft/items/diamond_sword.json",
        &serde_json::to_vec(&edited).unwrap(),
    )
    .unwrap();
    assert!(!prepare(root).unwrap().is_empty());
    apply(root, "1.21.10", &settings, &mut report).unwrap();
    assert_eq!(
        read(root, "assets/minecraft/items/diamond_sword.json"),
        edited
    );
}
fn read(root: &Path, name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(root.join(name)).unwrap()).unwrap()
}

#[test]
fn bundled_armor_template_filters_future_trims_and_keeps_supported_materials() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for item in ["diamond_chestplate", "leather_chestplate", "turtle_helmet"] {
        let source: Value =
            serde_json::from_slice(&ARMOR[&format!("assets/minecraft/items/{item}.json")]).unwrap();
        for (version, resin) in [
            ("1.20.1", false),
            ("1.21.3", false),
            ("1.21.4", true),
            ("1.21.10", true),
        ] {
            let mut base = json!({"model":source["model"]["fallback"]});
            target_trim_template(root, crate::pack_format_of(version).unwrap(), &mut base);
            let text = base.to_string();
            assert_eq!(text.contains("minecraft:resin"), resin, "{version}/{item}");
            for material in [
                "quartz",
                "iron",
                "netherite",
                "redstone",
                "copper",
                "gold",
                "emerald",
                "diamond",
                "lapis",
                "amethyst",
            ] {
                assert!(
                    text.contains(&format!("minecraft:{material}")),
                    "{version}/{item}/{material}"
                );
            }
            if item.starts_with("leather") {
                assert!(text.contains("minecraft:dye"));
            }
        }
    }
}

#[test]
fn builtin_protection_1201_has_every_level_and_no_resin_warning_or_rule() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut report = AssetReport::default();
    apply(
        root,
        "1.20.1",
        &settings(&[Feature::ArmorProtection]),
        &mut report,
    )
    .unwrap();
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(report.warnings[0].contains("CIT"));
    let mut levels = BTreeMap::<String, BTreeSet<String>>::new();
    for entry in
        crate::packfs::WalkDir::new(root.join("assets/minecraft/optifine/cit/fastpack_builtin_hud"))
    {
        let entry = entry.unwrap();
        if !entry.file_type().is_file() {
            continue;
        }
        let bytes = std::fs::read(entry.path()).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(!text.contains("resin"));
        let rule = crate::compat::properties::parse(&bytes).unwrap();
        if rule
            .get("enchantments")
            .is_some_and(|value| value == "minecraft:protection")
        {
            levels
                .entry(rule["items"].clone())
                .or_default()
                .insert(rule["enchantmentLevels"].clone());
        }
    }
    assert_eq!(levels.len(), 25);
    for (item, available) in levels {
        for level in ["1-", "2-", "3-", "4-"] {
            assert!(available.contains(level), "{item}/{level}");
        }
    }
}

#[test]
fn authored_modern_definition_retains_its_custom_trim_conditions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let author = json!({"model":{"type":"minecraft:select","property":"minecraft:trim_material","cases":[{"when":"author:new_trim","model":{"type":"minecraft:model","model":"author:item/armor"}}],"fallback":{"type":"minecraft:model","model":"minecraft:item/diamond_chestplate"}}});
    let path = "assets/minecraft/items/diamond_chestplate.json";
    write(root, path, &serde_json::to_vec(&author).unwrap()).unwrap();
    apply(
        root,
        "1.21.5",
        &settings(&[Feature::ArmorProtection]),
        &mut AssetReport::default(),
    )
    .unwrap();
    assert_eq!(read(root, path)["model"]["fallback"], author["model"]);
}

#[test]
fn disabled_features_leave_resources_and_settings_untouched() {
    let temp = tempfile::tempdir().unwrap();
    apply(
        temp.path(),
        "1.20.1",
        &Settings::default(),
        &mut AssetReport::default(),
    )
    .unwrap();
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    let mut s = Settings::default();
    s.set(Feature::Sharpness, true);
    s.set(Feature::Sharpness, true);
    assert_eq!(s.count(), 1);
    s.set(Feature::Sharpness, false);
    assert!(!s.active());
}

#[test]
fn modern_gui_layers_preserve_custom_base_bow_states_and_do_not_accumulate() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let path = "assets/minecraft/items/diamond_sword.json";
    let authored = json!({"hand_animation_on_swap":false,"model":{"type":"minecraft:model","model":"author:item/short_sword"}});
    write(root, path, &serde_json::to_vec(&authored).unwrap()).unwrap();
    let s = settings(&[
        Feature::Sharpness,
        Feature::GoldenDurability,
        Feature::BowEnchantments,
        Feature::ArmorProtection,
    ]);
    let mut report = AssetReport::default();
    apply(root, "1.21.10", &s, &mut report).unwrap();
    let first = read(root, path);
    assert_eq!(first["fastpack_builtin_hud"], authored);
    assert_eq!(first["model"]["fallback"], authored["model"]);
    assert_eq!(
        first["model"]["cases"][0]["model"]["models"][0],
        authored["model"]
    );
    assert_eq!(first["hand_animation_on_swap"], false);
    apply(root, "1.21.10", &s, &mut report).unwrap();
    assert_eq!(read(root, path), first);
    let bow = read(root, "assets/minecraft/items/bow.json");
    let text = bow.to_string();
    assert!(
        text.contains("using_item")
            && text.contains("use_duration")
            && text.contains("bowhud:item/pro_5")
    );
    let gold = read(root, "assets/minecraft/items/golden_sword.json").to_string();
    assert!(gold.contains("swordhud:item/dur/32") && gold.contains("swordhud:item/sharp/5"));
    let leather = read(root, "assets/minecraft/items/leather_chestplate.json").to_string();
    assert!(
        leather.contains("minecraft:dye")
            && leather.contains("trim")
            && leather.contains("armorhud:item/pro_4")
    );
}

#[test]
fn numeric_features_are_independent_and_skip_items_absent_from_the_target() {
    for feature in [
        Feature::GoldenDurability,
        Feature::Sharpness,
        Feature::BowEnchantments,
        Feature::ArmorProtection,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        apply(
            root,
            "1.21.5",
            &settings(&[feature]),
            &mut AssetReport::default(),
        )
        .unwrap();
        let sword = root
            .join("assets/minecraft/items/diamond_sword.json")
            .is_file();
        let bow = root.join("assets/minecraft/items/bow.json").is_file();
        let armor = root
            .join("assets/minecraft/items/diamond_helmet.json")
            .is_file();
        assert_eq!(sword, feature == Feature::Sharpness);
        assert_eq!(bow, feature == Feature::BowEnchantments);
        assert_eq!(armor, feature == Feature::ArmorProtection);
        assert!(
            !root
                .join("assets/minecraft/items/copper_sword.json")
                .exists()
        );
        assert!(
            !root
                .join("assets/minecraft/items/copper_helmet.json")
                .exists()
        );
    }
}

#[test]
fn every_supported_shader_profile_keeps_its_native_interface_and_glyph_discard() {
    for (version, format) in &crate::tables().pack_formats {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut report = AssetReport::default();
        shaders(
            root,
            version,
            &settings(&[Feature::HighlightNames, Feature::ClearTooltips]),
            &mut report,
        )
        .unwrap();
        if *format < 7 {
            assert!(!root.join("assets").exists());
            assert!(!report.warnings.is_empty());
            continue;
        }
        let profile = &SHADERS.profiles[&SHADERS.versions[version]];
        let mut text_found = false;
        for (name, native) in profile {
            let output =
                std::fs::read_to_string(root.join(format!("assets/minecraft/shaders/core/{name}")))
                    .unwrap();
            if name.contains("text") && name.ends_with(".fsh") {
                assert!(
                    output.contains("color.a < 0.1")
                        && output.contains("discard;")
                        && (name.contains("background") || output.contains("vertexColor.a < 0.5")),
                    "{version}"
                );
                text_found = true;
                if name == "text.fsh" {
                    assert!(output.contains("#ifndef IS_GUI"));
                }
            }
            for line in native.lines().filter(|line| {
                line.starts_with("#version")
                    || line.contains(" in ")
                    || line.contains(" out ")
                    || line.starts_with("uniform ")
                    || line.starts_with("#include")
                    || line.starts_with("#moj_import")
            }) {
                assert!(output.contains(line), "{version}: {line}");
            }
        }
        assert!(text_found, "{version}");
        let stems = if profile.contains_key("text.vsh") {
            vec!["text"]
        } else {
            vec![
                "rendertype_text",
                "rendertype_text_intensity",
                "rendertype_text_see_through",
                "rendertype_text_intensity_see_through",
            ]
        };
        for stem in stems {
            let vertex = std::fs::read_to_string(
                root.join(format!("assets/minecraft/shaders/core/{stem}.vsh")),
            )
            .unwrap();
            let fragment = std::fs::read_to_string(
                root.join(format!("assets/minecraft/shaders/core/{stem}.fsh")),
            )
            .unwrap();
            assert!(
                vertex.contains("fastpackWorldText") && vertex.contains("ProjMat[3][3]"),
                "{version}/{stem}"
            );
            assert!(
                fragment.contains("fastpackWorldText > 0.5"),
                "{version}/{stem}"
            );
            assert!(
                !fragment.contains("vertexColor.a > 0.0 && vertexColor.a < 0.5 &&"),
                "opaque label pass must also be highlighted: {version}/{stem}"
            );
        }
    }
}

#[test]
fn legacy_gui_numbers_do_not_replace_native_cit_cache_or_leak_into_handheld_base() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "fastpack/native-cit.json",
        br#"{"author":"preserve me"}"#,
    )
    .unwrap();
    let mut report = AssetReport::default();
    apply(
        root,
        "1.20.1",
        &settings(&[Feature::Sharpness]),
        &mut report,
    )
    .unwrap();
    assert_eq!(
        std::fs::read(root.join("fastpack/native-cit.json")).unwrap(),
        br#"{"author":"preserve me"}"#
    );
    let models = crate::packfs::WalkDir::new(
        root.join("assets/minecraft/models/item/fastpack_builtin_hud_cit"),
    )
    .into_iter()
    .filter_map(Result::ok)
    .filter(|e| e.file_type().is_file())
    .collect::<Vec<_>>();
    assert!(!models.is_empty());
    let mut scoped = false;
    for entry in models {
        let model: Value = serde_json::from_slice(&std::fs::read(entry.path()).unwrap()).unwrap();
        if model["loader"] == "forge:separate_transforms" {
            assert!(!model["base"].to_string().contains("swordhud:item/sharp"));
            let gui = model["perspectives"]["gui"].to_string();
            if gui.contains("swordhud:item/sharp") || gui.contains("fastpack_builtin_hud_cit") {
                scoped = true;
            }
        }
    }
    assert!(scoped);
}

#[test]
fn durability_overlay_keeps_live_hand_pose_and_texture_from_other_packs() {
    for pose_below in [true, false] {
        let work = tempfile::tempdir().unwrap();
        let root = work.path();
        let item = "assets/minecraft/models/item/golden_sword.json";
        let authored = json!({"parent":"minecraft:item/handheld",
        "textures":{"layer0":"author:item/golden_sword"},
        "display":{
            "firstperson_righthand":{"rotation":[15,35,55],"scale":[0.3,0.4,0.5]},
            "firstperson_lefthand":{"rotation":[-15,-35,-55],"translation":[2,4,6]}
        }});
        let authored_bytes = serde_json::to_vec(&authored).unwrap();
        let texture = root.join("assets/author/textures/item/golden_sword.png");
        std::fs::create_dir_all(texture.parent().unwrap()).unwrap();
        image::RgbaImage::from_pixel(16, 16, image::Rgba([201, 97, 42, 255]))
            .save(&texture)
            .unwrap();
        if pose_below {
            write(root, item, &authored_bytes).unwrap();
        }
        let mut report = AssetReport::default();
        apply(
            root,
            "1.20.1",
            &settings(&[Feature::GoldenDurability]),
            &mut report,
        )
        .unwrap();
        if !pose_below {
            write(root, item, &authored_bytes).unwrap();
        }
        assert_eq!(std::fs::read(root.join(item)).unwrap(), authored_bytes);
        assert!(
            !root
                .join("assets/minecraft/models/item/handheld.json")
                .exists()
        );
        let mut count = 0;
        for entry in crate::packfs::WalkDir::new(
            root.join("assets/minecraft/models/item/fastpack_builtin_hud_cit"),
        ) {
            let entry = entry.unwrap();
            if !entry.file_type().is_file() {
                continue;
            }
            let model: Value =
                serde_json::from_slice(&std::fs::read(entry.path()).unwrap()).unwrap();
            if model["loader"] != "forge:separate_transforms" {
                continue;
            }
            count += 1;
            assert_eq!(
                model["base"],
                json!({"parent":"minecraft:item/golden_sword"})
            );
            assert!(model.get("display").is_none());
            let gui = &model["perspectives"]["gui"];
            assert_eq!(gui["parent"], "minecraft:item/golden_sword");
            assert!(gui["textures"].get("layer0").is_none());
            assert!(gui["textures"].get("layer1").is_some());
            let inherited = crate::compat::custom_items::cit_model(
                root,
                15,
                model["base"]["parent"].as_str().unwrap(),
            )
            .unwrap();
            assert_eq!(
                inherited["display"]["firstperson_righthand"],
                authored["display"]["firstperson_righthand"]
            );
            assert_eq!(
                inherited["display"]["firstperson_lefthand"],
                authored["display"]["firstperson_lefthand"]
            );
            assert_eq!(inherited["textures"]["layer0"], "author:item/golden_sword");
        }
        assert!(
            count >= 32,
            "all remaining durability counters need a live base"
        );
    }
}
