use crate::packfs as fs;
use crate::packfs::PathExt;
use crate::packfs::WalkDir;
use anyhow::{Result, ensure};
use image::{Rgba, RgbaImage, imageops};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct AssetReport {
    pub converted: Vec<String>,
    pub generated: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct Sprite {
    source: String,
    target: String,
    source_width: u32,
    source_height: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

pub(crate) fn backup(root: &Path, path: &Path) -> Result<()> {
    let relative = path.strip_prefix(root)?;
    let destination = root.join("fastpack/original-assets").join(relative);
    if !destination.pack_exists() {
        fs::create_dir_all(destination.parent().unwrap())?;
        fs::copy(path, destination)?;
    }
    Ok(())
}

pub(crate) fn save(root: &Path, path: &Path, image: &RgbaImage) -> Result<()> {
    if path.pack_exists() {
        backup(root, path)?;
    }
    fs::create_dir_all(path.parent().unwrap())?;
    crate::packfs::save_image(image, path)?;
    Ok(())
}

pub(super) fn open(path: &Path) -> Result<RgbaImage> {
    Ok(crate::packfs::image_reader(path)?
        .with_guessed_format()?
        .decode()?
        .to_rgba8())
}

pub fn chest(source: &RgbaImage, kind: &str, reverse: bool) -> Result<RgbaImage> {
    let regions: BTreeMap<String, Vec<[u32; 9]>> =
        serde_json::from_str(include_str!("../../assets/chest-regions.json"))?;
    let recipes = regions
        .get(kind)
        .ok_or_else(|| anyhow::anyhow!("未知箱子类型"))?;
    let scale = source.height() / 64;
    ensure!(
        scale > 0 && source.height().is_multiple_of(64),
        "箱子纹理高度必须是 64 的倍数"
    );
    let width = if reverse && kind != "single" { 128 } else { 64 };
    let mut out = RgbaImage::new(width * scale, 64 * scale);
    if kind == "single" {
        ensure!(source.width() == 64 * scale, "单箱子纹理比例不匹配");
    } else {
        ensure!(
            source.width() == if reverse { 64 * scale } else { 128 * scale },
            "双箱子纹理比例不匹配"
        );
    }
    for recipe in recipes {
        let [sx, sy, dx, dy, w, h, _, flipx, flipy] = *recipe;
        for y in 0..h * scale {
            for x in 0..w * scale {
                let source_x = sx * scale + if flipx == 1 { w * scale - 1 - x } else { x };
                let source_y = sy * scale + if flipy == 1 { h * scale - 1 - y } else { y };
                if reverse {
                    out.put_pixel(
                        source_x,
                        source_y,
                        *source.get_pixel(dx * scale + x, dy * scale + y),
                    );
                } else {
                    out.put_pixel(
                        dx * scale + x,
                        dy * scale + y,
                        *source.get_pixel(source_x, source_y),
                    );
                }
            }
        }
    }
    Ok(out)
}

fn chests(root: &Path, source: u32, target: u32, report: &mut AssetReport) -> Result<()> {
    let upgrade = source < 5 && target >= 5;
    let downgrade = target < 5 && source >= 5;
    if !upgrade && !downgrade {
        return Ok(());
    }
    let dir = root.join("assets/minecraft/textures/entity/chest");
    if !dir.pack_exists() {
        return Ok(());
    }
    for name in ["normal", "trapped", "christmas", "ender"] {
        let single = dir.join(format!("{name}.png"));
        if single.pack_exists() {
            let original = open(&single)?;
            let converted = chest(&original, "single", downgrade)?;
            save(root, &single, &converted)?;
            report.converted.push(format!(
                "箱子 {name}：1.15 UV 布局{}",
                if downgrade { "还原" } else { "重建" }
            ));
        }
        let double = dir.join(format!("{name}_double.png"));
        if upgrade && double.pack_exists() {
            let original = open(&double)?;
            for kind in ["left", "right"] {
                let output = dir.join(format!("{name}_{kind}.png"));
                if output.pack_exists() {
                    continue;
                }
                save(root, &output, &chest(&original, kind, false)?)?;
            }
            backup(root, &double)?;
            fs::remove_file(&double)?;
            report
                .converted
                .push(format!("双箱子 {name}：拆为左右纹理并重建 UV"));
        }
        if downgrade && !double.pack_exists() {
            let left = dir.join(format!("{name}_left.png"));
            let right = dir.join(format!("{name}_right.png"));
            if left.pack_exists() && right.pack_exists() {
                let a = chest(&open(&left)?, "left", true)?;
                let b = chest(&open(&right)?, "right", true)?;
                ensure!(
                    a.dimensions() == b.dimensions(),
                    "双箱子的左右纹理分辨率不同"
                );
                let mut joined = a;
                for (x, y, pixel) in b.enumerate_pixels() {
                    if pixel[3] > 0 {
                        joined.put_pixel(x, y, *pixel);
                    }
                }
                save(root, &double, &joined)?;
                report
                    .converted
                    .push(format!("双箱子 {name}：自动合并旧版 UV"));
            }
        }
    }
    Ok(())
}

pub fn alpha_pattern(
    pattern: &RgbaImage,
    base: Option<&RgbaImage>,
    shield: bool,
    reverse: bool,
) -> Result<RgbaImage> {
    let scale = pattern.width() / 64;
    ensure!(
        scale > 0 && pattern.width().is_multiple_of(64),
        "旗帜/盾牌纹理宽度必须是 64 的倍数"
    );
    if let Some(base) = base {
        ensure!(
            base.dimensions() == pattern.dimensions(),
            "底图与图案分辨率不同"
        );
    }
    let mut out = RgbaImage::new(pattern.width(), pattern.height());
    let region = if shield {
        (2, 2, 12, 22)
    } else {
        (0, 0, 42, 41)
    };
    for y in region.1 * scale..(region.3 * scale).min(pattern.height()) {
        for x in region.0 * scale..(region.2 * scale).min(pattern.width()) {
            let pixel = pattern.get_pixel(x, y);
            if reverse {
                out.put_pixel(x, y, Rgba([pixel[3], pixel[3], pixel[3], 255]));
            } else {
                let rgb = base
                    .map(|base| base.get_pixel(x, y).0)
                    .unwrap_or([255, 255, 255, 255]);
                out.put_pixel(
                    x,
                    y,
                    Rgba([
                        rgb[0],
                        rgb[1],
                        rgb[2],
                        ((u16::from(pixel[0]) * u16::from(pixel[3])) / 255) as u8,
                    ]),
                );
            }
        }
    }
    Ok(out)
}

fn patterns(root: &Path, source: u32, target: u32, report: &mut AssetReport) -> Result<()> {
    if !(source < 5 && target >= 5 || target < 5 && source >= 5) {
        return Ok(());
    }
    let reverse = target < 5;
    for family in ["banner", "shield"] {
        let dir = root.join(format!("assets/minecraft/textures/entity/{family}"));
        if !dir.pack_exists() {
            continue;
        }
        let base_path = root.join(format!(
            "assets/minecraft/textures/entity/{family}_base.png"
        ));
        let base = base_path
            .pack_exists()
            .then(|| open(&base_path))
            .transpose()?;
        let files: Vec<_> = fs::read_dir(&dir)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "png"))
            .collect();
        for path in files {
            let image = open(&path)?;
            let converted = alpha_pattern(&image, base.as_ref(), family == "shield", reverse)?;
            save(root, &path, &converted)?;
            report.converted.push(format!(
                "{family}/{}：透明掩码{}",
                path.file_name().unwrap().to_string_lossy(),
                if reverse { "还原" } else { "转为 alpha" }
            ));
        }
    }
    for name in ["dragon", "dragon_exploding"] {
        let path = root.join(format!(
            "assets/minecraft/textures/entity/enderdragon/{name}.png"
        ));
        if !path.pack_exists() || reverse {
            continue;
        }
        let mut image = open(&path)?;
        let scale = image.width() / 256;
        if scale == 0 {
            report
                .warnings
                .push(format!("{name} 分辨率不能表达原生末影龙 UV"));
            continue;
        }
        backup(root, &path)?;
        for y in 88 * scale..(200 * scale).min(image.height()) {
            for x in 56 * scale..(112 * scale).min(image.width()) {
                image.put_pixel(x, y, Rgba([0, 0, 0, 0]));
            }
        }
        crate::packfs::save_image(&image, path)?;
        report
            .converted
            .push(format!("{name}：清理 1.15 移除的龙翼底面 UV"));
    }
    Ok(())
}

fn box_regions(u: u32, v: u32, x: u32, y: u32, z: u32) -> [(u32, u32, u32, u32); 6] {
    [
        (u + z, v, x, z),
        (u + z + x, v, x, z),
        (u, v + z, z, y),
        (u + z, v + z, x, y),
        (u + z + x, v + z, z, y),
        (u + z + x + z, v + z, x, y),
    ]
}

fn reproject_box(
    source: &RgbaImage,
    out: &mut RgbaImage,
    old: (u32, u32, u32, u32, u32),
    new: (u32, u32, u32, u32, u32),
    scale: u32,
) {
    let old = box_regions(old.0, old.1, old.2, old.3, old.4);
    let new = box_regions(new.0, new.1, new.2, new.3, new.4);
    for (a, b) in old.into_iter().zip(new) {
        let patch = imageops::crop_imm(source, a.0 * scale, a.1 * scale, a.2 * scale, a.3 * scale)
            .to_image();
        let patch = imageops::resize(
            &patch,
            b.2 * scale,
            b.3 * scale,
            imageops::FilterType::Nearest,
        );
        imageops::replace(out, &patch, i64::from(b.0 * scale), i64::from(b.1 * scale));
    }
}

pub fn boat(source: &RgbaImage) -> Result<RgbaImage> {
    ensure!(
        source.width().is_multiple_of(64) && source.height() * 2 == source.width(),
        "1.8 船纹理需要 64×32 或等比例高清布局"
    );
    let scale = source.width() / 64;
    ensure!(scale > 0, "船纹理为空");
    let mut out = RgbaImage::new(128 * scale, 64 * scale);
    reproject_box(
        source,
        &mut out,
        (0, 8, 24, 16, 4),
        (0, 0, 28, 16, 3),
        scale,
    );
    for new in [
        (0, 19, 18, 6, 2),
        (0, 27, 16, 6, 2),
        (0, 35, 28, 6, 2),
        (0, 43, 28, 6, 2),
    ] {
        reproject_box(source, &mut out, (0, 0, 20, 6, 2), new, scale);
    }
    for y in [0, 20] {
        reproject_box(source, &mut out, (0, 0, 20, 6, 2), (62, y, 2, 2, 18), scale);
        reproject_box(source, &mut out, (0, 0, 20, 6, 2), (62, y, 1, 6, 7), scale);
    }
    Ok(out)
}

fn entities(root: &Path, source: u32, target: u32, report: &mut AssetReport) -> Result<()> {
    if target >= 2 {
        let old = root.join("assets/minecraft/textures/entity/boat.png");
        if old.pack_exists() {
            let converted = boat(&open(&old)?)?;
            let woods = ["oak", "spruce", "birch", "jungle", "acacia", "dark_oak"];
            for wood in woods {
                for name in [format!("boat/{wood}.png"), format!("boat/boat_{wood}.png")] {
                    let output = root.join("assets/minecraft/textures/entity").join(name);
                    if !output.pack_exists() {
                        save(root, &output, &converted)?;
                    }
                }
            }
            backup(root, &old)?;
            fs::remove_file(old)?;
            report
                .converted
                .push("船：按新船模型 UV 重建船体，新增桨纹理从原木材图案生成".into());
        }
    }
    if target < 2 && source >= 2 {
        let old = root.join("assets/minecraft/textures/entity/boat.png");
        if !old.pack_exists() {
            let original =
                root.join("fastpack/original-assets/assets/minecraft/textures/entity/boat.png");
            if original.pack_exists() {
                fs::copy(original, &old)?;
                report
                    .converted
                    .push("船：自动恢复备份中的旧版船纹理".into());
            } else if let Some(path) = ["boat/oak.png", "boat/boat_oak.png"]
                .iter()
                .map(|name| root.join("assets/minecraft/textures/entity").join(name))
                .find(|path| path.pack_exists())
            {
                let image = open(&path)?;
                let scale = image.width() / 128;
                if scale == 0 || image.height() != 64 * scale {
                    report.warnings.push(format!(
                        "{}：现代船 UV 比例无效，保留原纹理",
                        path.strip_prefix(root).unwrap_or(&path).display()
                    ));
                } else {
                    let mut out = RgbaImage::new(64 * scale, 32 * scale);
                    reproject_box(
                        &image,
                        &mut out,
                        (0, 0, 28, 16, 3),
                        (0, 8, 24, 16, 4),
                        scale,
                    );
                    reproject_box(&image, &mut out, (0, 35, 28, 6, 2), (0, 0, 20, 6, 2), scale);
                    save(root, &old, &out)?;
                    report
                        .converted
                        .push("船：自动反向重建旧船 UV，使用橡木船作为旧版统一材质".into());
                }
            }
        }
    }
    if target >= 4 {
        for wood in ["oak", "spruce", "birch", "jungle", "acacia", "dark_oak"] {
            let old = root.join(format!(
                "assets/minecraft/textures/entity/boat/boat_{wood}.png"
            ));
            let new = root.join(format!("assets/minecraft/textures/entity/boat/{wood}.png"));
            if old.pack_exists() && !new.pack_exists() {
                fs::copy(&old, &new)?;
                report
                    .converted
                    .push(format!("船目录：boat_{wood}.png → {wood}.png"));
            }
        }
    }
    if target >= 4 {
        for (old, new) in [
            ("iron_golem.png", "iron_golem/iron_golem.png"),
            (
                "endercrystal/endercrystal.png",
                "end_crystal/end_crystal.png",
            ),
        ] {
            let old = root.join("assets/minecraft/textures/entity").join(old);
            let new = root.join("assets/minecraft/textures/entity").join(new);
            if old.pack_exists() && !new.pack_exists() {
                backup(root, &old)?;
                fs::create_dir_all(new.parent().unwrap())?;
                fs::copy(&old, &new)?;
                report.converted.push(format!(
                    "实体目录 {} → {}",
                    old.file_name().unwrap().to_string_lossy(),
                    new.display()
                ));
            }
        }
    }
    if target >= 84 && source < 84 {
        for (old, new) in [
            ("entity/minecart.png", "entity/minecart/minecart.png"),
            ("entity/banner_base.png", "entity/banner/banner_base.png"),
            ("environment/sun.png", "environment/celestial/sun.png"),
        ] {
            let old = root.join("assets/minecraft/textures").join(old);
            let new = root.join("assets/minecraft/textures").join(new);
            if old.pack_exists() && !new.pack_exists() {
                backup(root, &old)?;
                fs::create_dir_all(new.parent().unwrap())?;
                fs::copy(&old, &new)?;
                report.converted.push(format!(
                    "26.x 资源路径 {}",
                    new.strip_prefix(root)?.display()
                ));
            }
        }
        let moon = root.join("assets/minecraft/textures/environment/moon_phases.png");
        if moon.pack_exists() {
            let image = open(&moon)?;
            ensure!(
                image.width() % 4 == 0 && image.height() % 2 == 0,
                "月相图比例无效"
            );
            let w = image.width() / 4;
            let h = image.height() / 2;
            for (index, name) in [
                "full_moon",
                "waning_gibbous",
                "third_quarter",
                "waning_crescent",
                "new_moon",
                "waxing_crescent",
                "first_quarter",
                "waxing_gibbous",
            ]
            .into_iter()
            .enumerate()
            {
                let part = imageops::crop_imm(
                    &image,
                    (index as u32 % 4) * w,
                    (index as u32 / 4) * h,
                    w,
                    h,
                )
                .to_image();
                let output = root.join(format!(
                    "assets/minecraft/textures/environment/celestial/moon/{name}.png"
                ));
                if !output.pack_exists() {
                    save(root, &output, &part)?;
                }
            }
            report
                .converted
                .push("月相图：自动拆为 26.x 八张月相资源".into());
        }
    }
    if target < 84 && source >= 84 {
        for (old, new) in [
            ("entity/minecart.png", "entity/minecart/minecart.png"),
            ("entity/banner_base.png", "entity/banner/banner_base.png"),
            ("environment/sun.png", "environment/celestial/sun.png"),
        ] {
            let old = root.join("assets/minecraft/textures").join(old);
            let new = root.join("assets/minecraft/textures").join(new);
            if new.pack_exists() && !old.pack_exists() {
                fs::copy(&new, &old)?;
                report.converted.push(format!(
                    "实体/天体路径还原 {}",
                    old.strip_prefix(root)?.display()
                ));
            }
        }
        let old = root.join("assets/minecraft/textures/environment/moon_phases.png");
        if !old.pack_exists() {
            let names = [
                "full_moon",
                "waning_gibbous",
                "third_quarter",
                "waning_crescent",
                "new_moon",
                "waxing_crescent",
                "first_quarter",
                "waxing_gibbous",
            ];
            let paths: Vec<_> = names
                .iter()
                .map(|name| {
                    root.join(format!(
                        "assets/minecraft/textures/environment/celestial/moon/{name}.png"
                    ))
                })
                .collect();
            if paths.iter().all(|path| path.pack_exists()) {
                let tiles = paths
                    .iter()
                    .map(|path| open(path))
                    .collect::<Result<Vec<_>>>()?;
                let (w, h) = tiles[0].dimensions();
                ensure!(
                    tiles.iter().all(|tile| tile.dimensions() == (w, h)),
                    "月相图分辨率不一致"
                );
                let mut atlas = RgbaImage::new(w * 4, h * 2);
                for (index, tile) in tiles.iter().enumerate() {
                    imageops::replace(
                        &mut atlas,
                        tile,
                        i64::from(index as u32 % 4 * w),
                        i64::from(index as u32 / 4 * h),
                    );
                }
                save(root, &old, &atlas)?;
                report
                    .converted
                    .push("月相：自动合并新版八张月相回旧版图集".into());
            }
        }
    }
    Ok(())
}

pub(super) fn nine_slice(source: &RgbaImage, width: u32, height: u32, border: u32) -> RgbaImage {
    let mut out = RgbaImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let sx = if x < border {
                x
            } else if x >= width - border {
                source.width() - (width - x)
            } else {
                border + (x - border) * (source.width() - 2 * border) / (width - 2 * border)
            };
            let sy = if y < border {
                y
            } else if y >= height - border {
                source.height() - (height - y)
            } else {
                border + (y - border) * (source.height() - 2 * border) / (height - 2 * border)
            };
            out.put_pixel(x, y, *source.get_pixel(sx, sy));
        }
    }
    out
}

fn item_resources(root: &Path, source: u32, target: u32, report: &mut AssetReport) -> Result<()> {
    let folder = if target >= 4 { "item" } else { "items" };
    let textures = root.join(format!("assets/minecraft/textures/{folder}"));
    if !textures.pack_is_dir() {
        return Ok(());
    }
    if source < 4 && target >= 4 {
        for (old, aliases) in &crate::tables::legacy_items().aliases {
            let name = crate::flatten_texture_name(old, "item");
            let image = textures.join(format!("{name}.png"));
            if !image.pack_exists() {
                continue;
            }
            for alias in aliases {
                let destination = textures.join(format!("{alias}.png"));
                if destination.pack_exists() {
                    continue;
                }
                fs::copy(&image, &destination)?;
                let metadata = image.with_extension("png.mcmeta");
                if metadata.pack_exists() {
                    fs::copy(metadata, destination.with_extension("png.mcmeta"))?;
                }
                report
                    .converted
                    .push(format!("染料/材料：{name} → {alias} 共用原贴图"));
            }
        }
    }
    if source < 2 && target >= 2 {
        for (name, count) in [("clock", 64u32), ("compass", 32)] {
            let path = textures.join(format!("{name}.png"));
            if !path.pack_exists() {
                continue;
            }
            let image = open(&path)?;
            let width = image.width();
            if width == 0 || !image.height().is_multiple_of(width) {
                report
                    .warnings
                    .push(format!("{name} 图集比例无法识别，已保留原图"));
                continue;
            }
            let frames = image.height() / width;
            for index in 0..count {
                let destination = textures.join(format!("{name}_{index:02}.png"));
                if destination.pack_exists() {
                    continue;
                }
                let frame = index * frames / count;
                let cropped = imageops::crop_imm(&image, 0, frame * width, width, width).to_image();
                save(root, &destination, &cropped)?;
            }
            report
                .converted
                .push(format!("{name}：旧动态贴图自动拆为 {count} 个模型帧"));
        }
    }
    Ok(())
}

pub(super) fn clear_inventory_region(
    image: &mut RgbaImage,
    old: &RgbaImage,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) {
    for row in y..y + height {
        let left = old.get_pixel(x - 1, row);
        let right = old.get_pixel(x + width, row);
        for column in 0..width {
            let color = std::array::from_fn(|channel| {
                ((u32::from(left[channel]) * (width - column) + u32::from(right[channel]) * column)
                    / width) as u8
            });
            image.put_pixel(x + column, row, Rgba(color));
        }
    }
}

pub fn inventory_layout(old: &RgbaImage, reverse: bool) -> Result<RgbaImage> {
    let scale = old.width() / 256;
    ensure!(
        scale > 0 && old.width().is_multiple_of(256) && old.height() >= 166 * scale,
        "背包贴图比例无效"
    );
    let (sx, sy, dx, dy) = if reverse {
        (96, 16, 86, 24)
    } else {
        (86, 24, 96, 16)
    };
    let mut image = old.clone();
    clear_inventory_region(
        &mut image,
        old,
        sx * scale,
        sy * scale,
        76 * scale,
        38 * scale,
    );
    let crafting =
        imageops::crop_imm(old, sx * scale, sy * scale, 76 * scale, 38 * scale).to_image();
    imageops::replace(
        &mut image,
        &crafting,
        i64::from(dx * scale),
        i64::from(dy * scale),
    );
    if reverse {
        clear_inventory_region(
            &mut image,
            old,
            76 * scale,
            61 * scale,
            18 * scale,
            18 * scale,
        );
    } else {
        for row in 7 * scale..81 * scale {
            for column in 76 * scale..79 * scale {
                image.put_pixel(column, row, *old.get_pixel(79 * scale, row));
            }
        }
        let slot =
            imageops::crop_imm(old, 7 * scale, 83 * scale, 18 * scale, 18 * scale).to_image();
        imageops::replace(
            &mut image,
            &slot,
            76 * i64::from(scale),
            61 * i64::from(scale),
        );
    }
    let player = imageops::crop_imm(
        old,
        25 * scale,
        7 * scale,
        if reverse { 51 * scale } else { 54 * scale },
        74 * scale,
    )
    .to_image();
    let player = imageops::resize(
        &player,
        if reverse { 54 * scale } else { 51 * scale },
        74 * scale,
        imageops::FilterType::Nearest,
    );
    imageops::replace(
        &mut image,
        &player,
        25 * i64::from(scale),
        7 * i64::from(scale),
    );
    Ok(image)
}

fn recipe_controls(root: &Path, image: &mut RgbaImage) -> Result<()> {
    let scale = image.width() / 256;
    let book_path = root.join("assets/minecraft/textures/item/book.png");
    let book = if book_path.pack_exists() {
        Some(open(&book_path)?)
    } else {
        None
    };
    let atlas_path = root.join("assets/minecraft/textures/gui/recipe_button.png");
    let mut atlas = RgbaImage::new(image.width(), image.height());
    for state in 0..3 {
        let slot =
            imageops::crop_imm(image, 7 * scale, 83 * scale, 18 * scale, 18 * scale).to_image();
        let mut button = nine_slice(&slot, 20 * scale, 18 * scale, scale.max(1));
        if let Some(book) = &book {
            let size = book.width().min(book.height());
            let frame = imageops::crop_imm(book, 0, 0, size, size).to_image();
            let icon = imageops::resize(
                &frame,
                14 * scale,
                14 * scale,
                imageops::FilterType::Nearest,
            );
            imageops::overlay(
                &mut button,
                &icon,
                3 * i64::from(scale),
                2 * i64::from(scale),
            );
        } else {
            for y in 3 * scale..15 * scale {
                for x in 4 * scale..16 * scale {
                    let edge = x < 5 * scale || x >= 15 * scale || y < 4 * scale || y >= 14 * scale;
                    button.put_pixel(
                        x,
                        y,
                        Rgba(if edge {
                            [235, 238, 221, 255]
                        } else if state == 1 {
                            [32, 92, 65, 255]
                        } else {
                            [54, 128, 68, 255]
                        }),
                    );
                }
            }
        }
        imageops::replace(
            image,
            &button,
            178 * i64::from(scale),
            i64::from(state * 19 * scale),
        );
        imageops::replace(&mut atlas, &button, 0, i64::from(state * 19 * scale));
    }
    if !atlas_path.pack_exists() {
        save(root, &atlas_path, &atlas)?;
    }
    Ok(())
}

fn gui(root: &Path, source: u32, target: u32, report: &mut AssetReport) -> Result<()> {
    let textures = root.join("assets/minecraft/textures");
    if source < 2 && target >= 2 || source >= 2 && target < 2 {
        let path = textures.join("gui/container/inventory.png");
        if path.pack_exists() {
            let mut old = open(&path)?;
            if old.width() < 256 && old.width() > 0 {
                old = imageops::resize(
                    &old,
                    256,
                    old.height() * 256 / old.width(),
                    imageops::FilterType::Nearest,
                );
            }
            let mut converted = inventory_layout(&old, target < 2)?;
            if target >= 3 {
                recipe_controls(root, &mut converted)?;
            }
            save(root, &path, &converted)?;
            report.converted.push(
                if target < 2 {
                    "背包：合成栏反向重定位，清理副手槽"
                } else {
                    "背包：合成格、箭头与结果格右移 10px、上移 8px，并生成副手槽和配方书按钮"
                }
                .into(),
            );
        }
    }
    if source < 8 && target >= 8 {
        let path = textures.join("gui/container/inventory.png");
        if path.pack_exists() {
            let mut image = open(&path)?;
            let scale = image.width() / 256;
            ensure!(
                scale > 0 && image.height() >= 230 * scale,
                "背包 GUI 布局不符合 256 基准"
            );
            let old =
                imageops::crop_imm(&image, 0, 166 * scale, 120 * scale, 32 * scale).to_image();
            let narrow = nine_slice(&old, 32 * scale, 32 * scale, 2 * scale);
            imageops::replace(&mut image, &narrow, 0, 198 * i64::from(scale));
            save(root, &path, &image)?;
            report
                .converted
                .push("背包：用原效果列表边框自动生成 1.18 窄效果列表".into());
        }
    }
    if source < 7 && target >= 7 {
        for name in [
            "gui/container/gamemode_switcher.png",
            "gui/gamemode_switcher.png",
        ] {
            let path = textures.join(name);
            if !path.pack_exists() {
                continue;
            }
            let old = open(&path)?;
            let scale = old.width() / 128;
            if scale == 0 || old.height() < 101 * scale {
                continue;
            }
            let mut out = old.clone();
            for index in 0..2 {
                let tile = imageops::crop_imm(
                    &old,
                    index * 25 * scale,
                    75 * scale,
                    25 * scale,
                    25 * scale,
                )
                .to_image();
                let resized =
                    imageops::resize(&tile, 26 * scale, 26 * scale, imageops::FilterType::Nearest);
                imageops::replace(
                    &mut out,
                    &resized,
                    i64::from(index * 26 * scale),
                    75 * i64::from(scale),
                );
            }
            save(root, &path, &out)?;
            report
                .converted
                .push("游戏模式槽位：25px 自动适配为 26px".into());
        }
    }
    if source < 18 && target >= 18 {
        let recipes: Vec<Sprite> =
            serde_json::from_str(include_str!("../../assets/gui-sprites.json"))?;
        let mut cache = BTreeMap::new();
        let mut count = 0;
        for recipe in &recipes {
            let source = textures.join(&recipe.source);
            let output = textures.join(&recipe.target);
            if !source.pack_exists() {
                continue;
            }
            if !cache.contains_key(&recipe.source) {
                cache.insert(recipe.source.clone(), open(&source)?);
            }
            let image = &cache[&recipe.source];
            ensure!(
                image.width() % recipe.source_width == 0
                    && image.height() % recipe.source_height == 0,
                "GUI 图集比例无效: {}",
                recipe.source
            );
            let sx = image.width() / recipe.source_width;
            let sy = image.height() / recipe.source_height;
            if sx == 0 || sy == 0 {
                continue;
            }
            let cropped = imageops::crop_imm(
                image,
                recipe.x * sx,
                recipe.y * sy,
                recipe.width * sx,
                recipe.height * sy,
            )
            .to_image();
            save(root, &output, &cropped)?;
            count += 1;
        }
        if count > 0 {
            report.converted.push(format!(
                "GUI 图集：自动拆分 {count} 个独立精灵，保留高清比例"
            ));
        }
    }
    if target < 18 && source >= 18 {
        let recipes: Vec<Sprite> =
            serde_json::from_str(include_str!("../../assets/gui-sprites.json"))?;
        let mut rebuilt = BTreeMap::<String, RgbaImage>::new();
        let mut count = 0;
        for recipe in recipes {
            if target < 4
                && recipe.target == "gui/sprites/container/inventory/effect_background_small.png"
            {
                continue;
            }
            let source = textures.join(&recipe.target);
            if !source.pack_exists() {
                continue;
            }
            let tile = open(&source)?;
            let scale = tile.width() / recipe.width;
            ensure!(
                scale > 0
                    && tile.width() % recipe.width == 0
                    && tile.height() == recipe.height * scale,
                "独立 GUI 精灵比例无效"
            );
            if !rebuilt.contains_key(&recipe.source) {
                let existing = textures.join(&recipe.source);
                let canvas = if existing.pack_exists() {
                    open(&existing)?
                } else {
                    RgbaImage::new(recipe.source_width * scale, recipe.source_height * scale)
                };
                rebuilt.insert(recipe.source.clone(), canvas);
            }
            let image = rebuilt.get_mut(&recipe.source).unwrap();
            let atlas_scale = image.width() / recipe.source_width;
            ensure!(
                atlas_scale > 0 && image.width().is_multiple_of(recipe.source_width),
                "现有 GUI 图集缩放比例无效"
            );
            let scale = scale.max(atlas_scale);
            if atlas_scale != scale {
                *image = imageops::resize(
                    image,
                    recipe.source_width * scale,
                    image.height() * scale / atlas_scale,
                    imageops::FilterType::Nearest,
                );
            }
            let tile = if tile.width() != recipe.width * scale {
                imageops::resize(
                    &tile,
                    recipe.width * scale,
                    recipe.height * scale,
                    imageops::FilterType::Nearest,
                )
            } else {
                tile
            };
            imageops::replace(
                image,
                &tile,
                i64::from(recipe.x * scale),
                i64::from(recipe.y * scale),
            );
            count += 1;
        }
        for (name, image) in rebuilt {
            save(root, &textures.join(name), &image)?;
        }
        if count > 0 {
            report
                .converted
                .push(format!("GUI 图集：自动合并 {count} 个精灵回旧版布局"));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "gui_sprite_tests.rs"]
mod gui_sprite_tests;

fn rewrite_model(value: &mut Value, source: u32, target: u32, relative: &str) -> bool {
    let old = value.clone();
    super::item_display::migrate(value, source, target, relative);
    if let Some(variants) = value.get_mut("variants").and_then(Value::as_object_mut)
        && source < 4
        && target >= 4
        && let Some(normal) = variants.remove("normal")
    {
        variants.entry("").or_insert(normal);
    }
    if target < 4
        && source >= 4
        && let Some(variants) = value.get_mut("variants").and_then(Value::as_object_mut)
        && let Some(normal) = variants.remove("")
    {
        variants.entry("normal").or_insert(normal);
    }
    *value != old
}

fn model_name(name: &str) -> String {
    for (prefix, suffix) in [
        ("wool_", "wool"),
        ("carpet_", "carpet"),
        ("glass_", "stained_glass"),
        ("hardened_clay_", "terracotta"),
    ] {
        if let Some(color) = name.strip_prefix(prefix)
            && crate::tables()
                .colors
                .iter()
                .any(|value| value == color || color == "silver" && value == "light_gray")
        {
            return format!(
                "{}_{suffix}",
                if color == "silver" {
                    "light_gray"
                } else {
                    color
                }
            );
        }
    }
    match name {
        "stonebrick" => "stone_bricks",
        "stonebrick_mossy" => "mossy_stone_bricks",
        "stonebrick_cracked" => "cracked_stone_bricks",
        "stonebrick_chiseled" => "chiseled_stone_bricks",
        "hardened_clay" => "terracotta",
        "grass" => "grass_block",
        "end_stone" => "end_stone",
        _ => name,
    }
    .into()
}

pub(super) fn old_model_name(name: &str) -> String {
    for (prefix, suffix) in [
        ("wool_", "_wool"),
        ("carpet_", "_carpet"),
        ("glass_", "_stained_glass"),
        ("hardened_clay_", "_terracotta"),
    ] {
        if let Some(color) = name.strip_suffix(suffix)
            && crate::tables().colors.iter().any(|value| value == color)
        {
            return format!(
                "{prefix}{}",
                if color == "light_gray" {
                    "silver"
                } else {
                    color
                }
            );
        }
    }
    match name {
        "stone_bricks" => "stonebrick",
        "mossy_stone_bricks" => "stonebrick_mossy",
        "cracked_stone_bricks" => "stonebrick_cracked",
        "chiseled_stone_bricks" => "stonebrick_chiseled",
        "terracotta" => "hardened_clay",
        "grass_block" => "grass",
        _ => name,
    }
    .into()
}

fn item_model_name(name: &str) -> String {
    crate::tables::legacy_items()
        .models
        .get(name)
        .cloned()
        .unwrap_or_else(|| model_name(name))
}

fn item_definition(root: &Path, name: &str, target: u32) -> Result<()> {
    if target < 46 {
        return Ok(());
    }
    let path = root.join(format!("assets/minecraft/items/{name}.json"));
    if !path.pack_exists() {
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(
            path,
            serde_json::to_vec_pretty(
                &json!({"model":{"type":"minecraft:model","model":format!("minecraft:item/{name}")}}),
            )?,
        )?;
    }
    Ok(())
}

fn old_model_refs(value: &mut Value, blockstate: bool) -> bool {
    let original = value.clone();
    match value {
        Value::Object(values) => {
            for (key, value) in values {
                if matches!(key.as_str(), "parent" | "model")
                    && let Some(text) = value.as_str()
                {
                    let (namespace, path) = text.split_once(':').unwrap_or(("minecraft", text));
                    if namespace == "minecraft"
                        && let Some((folder, name)) = path.split_once('/')
                        && matches!(folder, "block" | "item")
                    {
                        let mapped = old_model_name(name);
                        if key == "model" && blockstate && folder == "block" {
                            *value = json!(mapped);
                        } else if mapped != name {
                            *value = json!(format!("minecraft:{folder}/{mapped}"));
                        }
                    }
                } else {
                    old_model_refs(value, blockstate);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                old_model_refs(value, blockstate);
            }
        }
        _ => {}
    }
    *value != original
}

fn model_refs(value: &mut Value, default_folder: &str) -> bool {
    let original = value.clone();
    match value {
        Value::Object(values) => {
            for (key, value) in values {
                if matches!(key.as_str(), "parent" | "model")
                    && let Some(text) = value.as_str()
                {
                    let (namespace, path) = text.split_once(':').unwrap_or(("minecraft", text));
                    let (folder, name) = path.split_once('/').unwrap_or((default_folder, path));
                    if matches!(folder, "block" | "item") {
                        let mapped = if namespace == "minecraft" {
                            if folder == "item" {
                                item_model_name(name)
                            } else {
                                model_name(name)
                            }
                        } else {
                            name.into()
                        };
                        if mapped != name || !path.contains('/') {
                            *value = json!(format!("{namespace}:{folder}/{mapped}"));
                        }
                    }
                } else {
                    model_refs(value, default_folder);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                model_refs(value, default_folder);
            }
        }
        _ => {}
    }
    *value != original
}

fn models(root: &Path, source: u32, target: u32, report: &mut AssetReport) -> Result<()> {
    let legacy_models = super::legacy_models::capture(root, source, target)?;
    let assets = root.join("assets");
    if !assets.pack_exists() {
        return Ok(());
    }
    let mut count = 0;
    let mut inherited_items = 0;
    let mut merged = BTreeMap::<String, serde_json::Map<String, Value>>::new();
    let paths: Vec<_> = WalkDir::new(&assets)
        .min_depth(1)
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|entry| {
            entry.file_type().is_file() && entry.path().extension().is_some_and(|ext| ext == "json")
        })
        .map(|entry| entry.into_path())
        .collect();
    let resolver = if source >= 2 && target < 2 {
        Some(super::item_display::ModelResolver::load(root, &paths)?)
    } else {
        None
    };
    for path in paths {
        let relative = path
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        if !relative.contains("/models/") && !relative.contains("/blockstates/") {
            continue;
        }
        let Ok(mut value) = serde_json::from_slice::<Value>(&fs::read(&path)?) else {
            continue;
        };
        if !value.is_object() {
            continue;
        }
        let recovered = if source >= 2 {
            super::item_display::recover(root, &relative, &mut value, target)?
        } else {
            false
        };
        let materialized = resolver
            .as_ref()
            .is_some_and(|resolver| resolver.materialize(&mut value, &relative));
        let mut changed = rewrite_model(&mut value, source, target, &relative);
        changed |= materialized;
        inherited_items += usize::from(materialized);
        changed |= recovered;
        if recovered {
            report
                .converted
                .push(format!("{relative}：从原模型备份自动修复旧转换的手持姿态"));
        }
        if source < 2 && target >= 2 && relative.starts_with("assets/minecraft/models/item/") {
            let name = path.file_stem().unwrap_or_default().to_string_lossy();
            if let Some(overrides) = crate::tables::legacy_items().overrides.get(name.as_ref()) {
                if value.get("overrides").is_none() {
                    value["overrides"] = json!(overrides);
                    changed = true;
                }
                if matches!(name.as_ref(), "clock" | "compass") {
                    if let Some(layer) = value
                        .get_mut("textures")
                        .and_then(|textures| textures.get_mut("layer0"))
                    {
                        *layer = json!(format!(
                            "{}{}",
                            if target >= 4 { "item/" } else { "items/" },
                            if name == "clock" {
                                "clock_00"
                            } else {
                                "compass_16"
                            }
                        ));
                        changed = true;
                    }
                }
            }
        }
        if source < 4 && target >= 4 {
            changed |= model_refs(
                &mut value,
                if relative.contains("/models/item/") {
                    "item"
                } else {
                    "block"
                },
            );
        }
        if target < 4 && source >= 4 {
            changed |= old_model_refs(&mut value, relative.contains("/blockstates/"));
        }
        if changed {
            backup(root, &path)?;
            fs::write(&path, serde_json::to_vec_pretty(&value)?)?;
            count += 1;
        }
        if source < 4 && target >= 4 && relative.contains("/blockstates/") {
            let name = path.file_stem().unwrap_or_default().to_string_lossy();
            let property = match name.as_ref() {
                "wool"
                | "stained_glass"
                | "stained_glass_pane"
                | "stained_hardened_clay"
                | "carpet"
                | "concrete"
                | "concrete_powder" => Some("color"),
                "planks" | "sapling" => Some("variant"),
                _ => None,
            };
            if let Some(property) = property
                && let Some(variants) = value.get("variants").and_then(Value::as_object)
            {
                let mut groups = BTreeMap::<String, serde_json::Map<String, Value>>::new();
                for (key, model) in variants {
                    let entries: Vec<_> = key.split(',').collect();
                    let specific = entries
                        .iter()
                        .find_map(|entry| entry.strip_prefix(&format!("{property}=")));
                    let Some(specific) = specific else {
                        continue;
                    };
                    let specific = if specific == "silver" {
                        "light_gray"
                    } else if specific == "big_oak" {
                        "dark_oak"
                    } else {
                        specific
                    };
                    let recognized = if property == "color" {
                        crate::tables().colors.iter().any(|color| color == specific)
                    } else {
                        ["oak", "spruce", "birch", "jungle", "acacia", "dark_oak"]
                            .contains(&specific)
                    };
                    if !recognized {
                        report.warnings.push(format!(
                            "{relative}：未知 {property} 分支已保留，未将其作为输出文件名"
                        ));
                        continue;
                    }
                    let base = match name.as_ref() {
                        "stained_hardened_clay" => "terracotta",
                        other => other,
                    };
                    let target = format!("{specific}_{base}");
                    let key = entries
                        .into_iter()
                        .filter(|entry| !entry.starts_with(&format!("{property}=")))
                        .collect::<Vec<_>>()
                        .join(",");
                    groups.entry(target).or_default().insert(key, model.clone());
                }
                for (name, variants) in groups {
                    let output = path.with_file_name(format!("{name}.json"));
                    if !output.pack_exists() {
                        fs::write(
                            output,
                            serde_json::to_vec_pretty(&json!({"variants":variants}))?,
                        )?;
                        count += 1;
                    }
                }
            }
        }
        if source < 4 && target >= 4 && relative.starts_with("assets/minecraft/models/") {
            let old = path.file_stem().unwrap_or_default().to_string_lossy();
            let name = if relative.contains("/models/item/") {
                item_model_name(&old)
            } else {
                model_name(&old)
            };
            if name != old {
                let destination = path.with_file_name(format!("{name}.json"));
                if !destination.pack_exists() {
                    fs::write(destination, serde_json::to_vec_pretty(&value)?)?;
                    count += 1;
                }
            }
        }
        if target < 4 && source >= 4 && relative.starts_with("assets/minecraft/models/") {
            let old = path.file_stem().unwrap_or_default().to_string_lossy();
            let name = old_model_name(&old);
            if name != old {
                let destination = path.with_file_name(format!("{name}.json"));
                if !destination.pack_exists() {
                    fs::write(destination, serde_json::to_vec_pretty(&value)?)?;
                    count += 1;
                }
            }
        }
        if target < 4 && source >= 4 && relative.starts_with("assets/minecraft/blockstates/") {
            let name = path.file_stem().unwrap_or_default().to_string_lossy();
            for base in [
                "wool",
                "carpet",
                "stained_glass",
                "stained_glass_pane",
                "terracotta",
                "concrete",
                "concrete_powder",
            ] {
                let Some(color) = name.strip_suffix(&format!("_{base}")) else {
                    continue;
                };
                if !crate::tables().colors.iter().any(|value| value == color) {
                    continue;
                }
                let color = if color == "light_gray" {
                    "silver"
                } else {
                    color
                };
                let legacy = if base == "terracotta" {
                    "stained_hardened_clay"
                } else {
                    base
                };
                if let Some(variants) = value.get("variants").and_then(Value::as_object) {
                    for (key, model) in variants {
                        let key = if key == "normal" || key.is_empty() {
                            format!("color={color}")
                        } else {
                            format!("color={color},{key}")
                        };
                        merged
                            .entry(legacy.into())
                            .or_default()
                            .insert(key, model.clone());
                    }
                }
            }
        }
        if source < 46 && target >= 46 && relative.contains("/models/item/") {
            let (namespace, path_suffix) = relative
                .strip_prefix("assets/")
                .unwrap()
                .split_once('/')
                .unwrap();
            let name = path_suffix
                .strip_prefix("models/item/")
                .unwrap()
                .trim_end_matches(".json");
            let name = if namespace == "minecraft" && source < 4 {
                item_model_name(name)
            } else {
                name.to_string()
            };
            let output = root.join(format!("assets/{namespace}/items/{name}.json"));
            if !output.pack_exists() {
                fs::create_dir_all(output.parent().unwrap())?;
                fs::write(
                    output,
                    serde_json::to_vec_pretty(
                        &json!({"model":{"type":"minecraft:model","model":format!("{namespace}:item/{name}")}}),
                    )?,
                )?;
                count += 1;
            }
            if value.get("overrides").is_some()
                && !super::modern_items::recognized(&relative, &value, target)
            {
                report.warnings.push(format!(
                    "{relative}：旧 overrides 已保留；尚未证明任意旧谓词与新物品调度语义等价"
                ));
            }
        }
    }
    for (name, variants) in merged {
        let path = root.join(format!("assets/minecraft/blockstates/{name}.json"));
        let mut value = if path.pack_exists() {
            serde_json::from_slice::<Value>(&fs::read(&path)?)?
        } else {
            json!({"variants":{}})
        };
        if let Some(existing) = value.get_mut("variants").and_then(Value::as_object_mut) {
            existing.extend(variants);
        }
        if path.pack_exists() {
            backup(root, &path)?;
        }
        fs::write(path, serde_json::to_vec_pretty(&value)?)?;
        count += 1;
    }
    if let Some(migration) = legacy_models {
        migration.apply(root, report)?;
    }
    if inherited_items > 0 {
        report.converted.push(format!("物品手持姿态：解析 {inherited_items} 个平面物品的父模型继承，补齐旋转、位移与缩放后换算为 1.8.9 姿态"));
    }
    if source < 4 && target >= 4 {
        let directory = root.join("assets/minecraft/models/item");
        for (old, aliases) in &crate::tables::legacy_items().aliases {
            let name = crate::flatten_texture_name(old, "item");
            let original = directory.join(format!("{name}.json"));
            if !original.pack_exists() {
                continue;
            }
            for alias in aliases {
                let path = directory.join(format!("{alias}.json"));
                if !path.pack_exists() {
                    fs::copy(&original, &path)?;
                    item_definition(root, alias, target)?;
                    count += 1;
                }
            }
        }
        let bed = root.join("assets/minecraft/textures/item/red_bed.png");
        let model = directory.join("red_bed.json");
        if bed.pack_exists()
            && !model.pack_exists()
            && !root
                .join("assets/minecraft/textures/entity/bed")
                .pack_exists()
        {
            fs::create_dir_all(&directory)?;
            fs::write(
                model,
                serde_json::to_vec_pretty(
                    &json!({"parent":"minecraft:item/generated","textures":{"layer0":"minecraft:item/red_bed"}}),
                )?,
            )?;
            item_definition(root, "red_bed", target)?;
            report
                .generated
                .push("床物品：为旧版二维贴图生成对应物品模型，保留原图标".into());
        }
    }
    if count > 0 {
        report.converted.push(format!(
            "模型/方块状态：自动迁移 {count} 个结构与扁平化分支"
        ));
    }
    Ok(())
}

fn fonts(root: &Path, source: u32, target: u32, report: &mut AssetReport) -> Result<()> {
    if !(source < 15 && target >= 15) {
        return Ok(());
    }
    let directory = root.join("assets/minecraft/font");
    let mut pages = Vec::new();
    let font_textures = root.join("assets/minecraft/textures/font");
    if font_textures.pack_is_dir() {
        for entry in fs::read_dir(&font_textures)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let page = name
                .strip_prefix("unicode_page_")
                .and_then(|value| value.strip_suffix(".png"))
                .and_then(|value| u32::from_str_radix(value, 16).ok());
            if let Some(page) = page.filter(|value| *value < 256) {
                let image = open(&entry.path())?;
                ensure!(
                    image.width() % 16 == 0 && image.height() % 16 == 0,
                    "Unicode 字体页不是 16×16 字符网格"
                );
                let chars: Vec<String> = (0..16)
                    .map(|row| {
                        (0..16)
                            .map(|col| {
                                char::from_u32(page * 256 + row * 16 + col)
                                    .filter(|c| !c.is_control())
                                    .unwrap_or('\0')
                            })
                            .collect()
                    })
                    .collect();
                pages.push(json!({"type":"bitmap","file":format!("minecraft:font/{name}"),"height":8,"ascent":7,"chars":chars}));
            }
        }
    }
    let mut files = Vec::new();
    if directory.pack_is_dir() {
        for entry in fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                files.push(path);
            }
        }
    }
    for path in files {
        let Ok(mut value) = serde_json::from_slice::<Value>(&fs::read(&path)?) else {
            continue;
        };
        let Some(providers) = value.get_mut("providers").and_then(Value::as_array_mut) else {
            continue;
        };
        if !providers.iter().any(|provider| {
            matches!(
                provider.get("type").and_then(Value::as_str),
                Some("legacy_unicode" | "minecraft:legacy_unicode")
            )
        }) {
            continue;
        }
        let mut converted = Vec::new();
        for provider in providers.drain(..) {
            if matches!(
                provider.get("type").and_then(Value::as_str),
                Some("legacy_unicode" | "minecraft:legacy_unicode")
            ) {
                converted.extend(pages.clone());
                converted.push(json!({"type":"reference","id":"minecraft:include/unifont"}));
            } else {
                converted.push(provider);
            }
        }
        *providers = converted;
        backup(root, &path)?;
        fs::write(&path, serde_json::to_vec_pretty(&value)?)?;
        report.converted.push(format!(
            "字体 {}：legacy_unicode 自动转为 bitmap 页和目标默认字形回退",
            path.file_name().unwrap().to_string_lossy()
        ));
    }
    if !pages.is_empty() && !directory.join("uniform.json").pack_exists() {
        fs::create_dir_all(&directory)?;
        let mut providers = pages;
        providers.push(json!({"type":"reference","id":"minecraft:include/unifont"}));
        fs::write(
            directory.join("uniform.json"),
            serde_json::to_vec_pretty(&json!({"providers":providers}))?,
        )?;
        report
            .generated
            .push("从上传的 Unicode 字体页生成新版 uniform 字体定义".into());
    }
    Ok(())
}

pub fn migrate(root: &Path, source: u32, target: u32) -> Result<AssetReport> {
    migrate_with_glint(root, source, target, None)
}

pub fn migrate_with_glint(
    root: &Path,
    source: u32,
    target: u32,
    glint_color: Option<[u8; 3]>,
) -> Result<AssetReport> {
    migrate_assets(root, source, target, glint_color, true, true)
}

pub fn migrate_with_netherite_sword(
    root: &Path,
    source: u32,
    target: u32,
    auto_netherite_sword: bool,
) -> Result<AssetReport> {
    migrate_with_netherite(root, source, target, true, auto_netherite_sword)
}

pub fn migrate_with_netherite(
    root: &Path,
    source: u32,
    target: u32,
    auto_netherite: bool,
    auto_netherite_sword: bool,
) -> Result<AssetReport> {
    migrate_assets(
        root,
        source,
        target,
        None,
        auto_netherite,
        auto_netherite_sword,
    )
}

fn migrate_assets(
    root: &Path,
    source: u32,
    target: u32,
    glint_color: Option<[u8; 3]>,
    auto_netherite: bool,
    auto_netherite_sword: bool,
) -> Result<AssetReport> {
    let mut report = AssetReport::default();
    let detailed = std::env::var_os("FAST_PACK_PROFILE_STAGES").is_some();
    let mut timer = std::time::Instant::now();
    super::texture_formats::migrate(root, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE super::texture_formats::migrate: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    chests(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE chests: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    patterns(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE patterns: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    entities(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE entities: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    item_resources(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE item_resources: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    super::legacy_textures::migrate(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE super::legacy_textures::migrate: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    super::item_bindings::migrate(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE super::item_bindings::migrate: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    super::item_display::bind_definitions(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE super::item_display::bind_definitions: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    super::modern_items::migrate(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE super::modern_items::migrate: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    models(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE models: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    super::item_transparency::migrate(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE super::item_transparency::migrate: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    super::legacy_blocks::migrate(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE super::legacy_blocks::migrate: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    super::equipment::migrate(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE super::equipment::migrate: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    super::versioned_textures::migrate(root, source, target, &mut report)?;
    super::animals::migrate(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE super::versioned_textures::migrate: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    super::players::migrate(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE super::players::migrate: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    super::recent_resources::migrate(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE super::recent_resources::migrate: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    if auto_netherite {
        super::netherite::migrate(root, target, auto_netherite_sword, &mut report)?;
    }
    super::modern_gui::prepare(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE super::modern_gui::prepare: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    super::modern_gui::migrate(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE super::modern_gui::migrate: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    gui(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE gui: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    super::legacy_gui::migrate(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE super::legacy_gui::migrate: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    super::effect_panels::migrate(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE super::effect_panels::migrate: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    super::glint::migrate(root, source, target, glint_color, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE super::glint::migrate: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
        timer = std::time::Instant::now();
    }
    fonts(root, source, target, &mut report)?;
    if detailed {
        eprintln!(
            "PROFILE fonts: {:.3}ms",
            timer.elapsed().as_secs_f64() * 1000.0
        );
    }
    Ok(report)
}
