use crate::converter::load_extra_renames;
use crate::{ConversionOptions, convert_pack, pack_format_of, probe_pack, suggest_output, tables};
use anyhow::{Context, Result};
use clap::Parser;
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "fast-pack",
    version,
    about = "Fast Pack · Minecraft Java 材质包转换器"
)]
struct Arguments {
    #[arg(
        long,
        value_name = "MINECRAFT",
        hide = !crate::client_audio::ENABLED,
        help = "通过网易 1.20.1 的 ViaBackwards 用户映射配置汉化名称与附魔"
    )]
    client_audio: Option<PathBuf>,
    #[arg(long, hide = !crate::client_audio::ENABLED, requires = "client_audio", help = "恢复 ViaForge 原始备份")]
    restore_client_audio: bool,
    #[arg(long, hide = !crate::client_audio::ENABLED, help = "关闭目标 1.20.1 的布吉岛资源与客户端自动适配")]
    no_heypixel: bool,
    #[arg(long, hide = !crate::client_audio::ENABLED, help = "仅生成兼容资源，不修改或导入本机客户端")]
    no_client_setup: bool,
    #[arg(
        long,
        help = "关闭缺失下界合金剑、工具和盔甲材质的自动生成，保留原包已有材质"
    )]
    no_netherite: bool,
    #[arg(long, help = "关闭缺失下界合金剑材质的自动生成，保留原包已有材质")]
    no_netherite_sword: bool,
    #[arg(
        long,
        value_name = "PACK_OR_PNG",
        help = "使用准星包或 PNG 中的准星，仅替换准星区域"
    )]
    crosshair: Option<PathBuf>,
    #[arg(long, help = "隐藏 1.9+ 的准星与快捷栏攻击冷却指示")]
    hide_attack_indicator: bool,
    #[arg(value_name = "SRC", help = "源 ZIP、RAR 或材质包文件夹")]
    source: Option<PathBuf>,
    #[arg(
        value_name = "OUT",
        help = "输出 ZIP 或文件夹，省略时按 [目标版本]原名自动命名"
    )]
    output: Option<PathBuf>,
    #[arg(long = "from", default_value = "auto", help = "源版本或 auto")]
    source_version: String,
    #[arg(long = "to", default_value = crate::LATEST_VERSION, help = "目标版本；26.4 对应当前官方快照")]
    target_version: String,
    #[arg(long, help = "输出为文件夹")]
    folder: bool,
    #[arg(long, help = "材质包描述，默认沿用原包")]
    desc: Option<String>,
    #[arg(long, value_parser = crate::compat::glint::parse_color, value_name = "RRGGBB", help = "自定义附魔闪烁颜色，例如 #FFFFFF；省略则保留原包效果")]
    glint_color: Option<[u8; 3]>,
    #[arg(
        long,
        conflicts_with = "glint_color",
        help = "恢复原版紫色附魔光效，保留原有图案与动画"
    )]
    vanilla_glint: bool,
    #[arg(long, help = "仅输出最终结果")]
    quiet: bool,
    #[arg(long, help = "仅识别材质包并输出 JSON")]
    probe: bool,
    #[arg(long, help = "列出支持的版本")]
    versions: bool,
    #[arg(long, help = "自定义纹理名称映射 JSON 文件")]
    renames: Option<PathBuf>,
    #[arg(
        long,
        value_name = "JSON",
        help = "按目标版本应用物品改名、大小和贴图描边规则"
    )]
    custom_items: Option<PathBuf>,
    #[arg(
        long = "builtin",
        value_enum,
        value_name = "FEATURE",
        help = "启用内置覆盖功能，可重复使用：sharpness、golden-durability、bow-enchantments、armor-protection、highlight-names、clear-tooltips、rainbow-outline"
    )]
    builtin: Vec<crate::builtin_overlays::Feature>,
    #[arg(long, value_parser=crate::compat::glint::parse_color, value_name="RRGGBB", help="昵称高亮背景颜色，例如 #FF99CC；自动启用昵称高亮")]
    name_background_color: Option<[u8; 3]>,
    #[arg(
        long,
        help = "动态彩色昵称背景，支持 1.17–1.21.4 和 26.2+；其余版本使用固定背景色"
    )]
    rainbow_name_background: bool,
    #[arg(long,value_parser=crate::compat::glint::parse_color,value_name="RRGGBB",help="自定义鱼竿线颜色，需要目标版本 1.17+")]
    fishing_line_color: Option<[u8; 3]>,
    #[arg(long,value_parser=crate::compat::glint::parse_color,value_name="RRGGBB",help="自定义瞄准方块边框颜色，需要目标版本 1.17+")]
    block_outline_color: Option<[u8; 3]>,
    #[arg(long, value_parser=crate::compat::line_colors::parse_width, value_name="MULTIPLIER", help="鱼竿线宽度倍数，0.5–8，1 为原版；需要 1.17+")]
    fishing_line_width: Option<f32>,
    #[arg(long, value_parser=crate::compat::line_colors::parse_width, value_name="MULTIPLIER", help="瞄准方块边框宽度倍数，0.5–8，1 为原版；需要 1.17+")]
    block_outline_width: Option<f32>,
    #[arg(long, conflicts_with = "vanilla_glint", help = "动态彩虹附魔光效")]
    rainbow_glint: bool,
    #[arg(
        long,
        requires = "crosshair",
        help = "动态彩色准星；旧版需要 OptiFine 动画"
    )]
    rainbow_crosshair: bool,
    #[arg(long, help = "动态彩色鱼竿线，支持 1.17+")]
    rainbow_fishing_line: bool,
    #[arg(long, help = "动态彩色方块选中边框，支持 1.17+")]
    rainbow_block_outline: bool,
    #[arg(long, help = "关闭小写规范化")]
    no_lowercase: bool,
    #[arg(long, help = "关闭语言文件转换")]
    no_lang: bool,
    #[arg(long, help = "将结构化转换报告另存为 JSON")]
    report_json: Option<PathBuf>,
}

pub fn run() -> Result<()> {
    if crate::client_audio::finish_pending()? {
        return Ok(());
    }
    let args = Arguments::parse();
    if let Some(client) = args.client_audio {
        if args.restore_client_audio {
            println!(
                "已恢复：{}",
                crate::client_audio::restore(&client)?.display()
            );
        } else {
            let installed = crate::client_audio::install(&client)?;
            println!("{}", serde_json::to_string_pretty(&installed)?);
            println!(
                "已配置官方名称与附魔汉化，保持模组不变；下一次启动读取配置。原声音效尚未启用，不能通过普通映射配置恢复被合并或取消的声音。"
            );
        }
        return Ok(());
    }
    if args.versions {
        for (label, format) in &tables().pack_formats {
            println!("{label}\t{format}");
        }
        return Ok(());
    }
    let source = args
        .source
        .context("请指定源材质包，使用 --help 查看用法")?;
    if args.probe {
        let info = probe_pack(&source);
        println!("{}", serde_json::to_string_pretty(&info)?);
        anyhow::ensure!(
            info.ok,
            "{}",
            info.error
                .unwrap_or_else(|| "没有找到 assets 或 pack.mcmeta".into())
        );
        return Ok(());
    }
    let output = args
        .output
        .unwrap_or_else(|| suggest_output(&source, &args.target_version, !args.folder));
    let as_zip = if output
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        true
    } else {
        !args.folder && !output.is_dir()
    };
    let mut builtin_overlays = crate::builtin_overlays::Settings {
        features: args.builtin,
        name_background: args
            .name_background_color
            .unwrap_or(crate::builtin_overlays::DEFAULT_NAME_BACKGROUND),
        name_background_rainbow: args.rainbow_name_background,
    };
    if args.name_background_color.is_some() || args.rainbow_name_background {
        builtin_overlays.set(crate::builtin_overlays::Feature::HighlightNames, true);
    }
    let options = ConversionOptions {
        src_format: if args.source_version == "auto" {
            None
        } else {
            Some(pack_format_of(&args.source_version)?)
        },
        tgt_format: pack_format_of(&args.target_version)?,
        target_version: Some(args.target_version.clone()),
        description: args.desc,
        glint_color: args.glint_color,
        restore_vanilla_glint: args.vanilla_glint,
        heypixel_compat: crate::client_audio::ENABLED && !args.no_heypixel,
        auto_client_setup: !args.no_client_setup,
        auto_netherite: !args.no_netherite,
        auto_netherite_sword: !args.no_netherite_sword,
        show_attack_indicator: !args.hide_attack_indicator,
        crosshair: args
            .crosshair
            .as_deref()
            .map(crate::crosshair::import)
            .transpose()?
            .and_then(|mut entries| entries.drain(..).next()),
        as_zip,
        lowercase: !args.no_lowercase,
        convert_lang: !args.no_lang,
        fishing_line_color: args.fishing_line_color,
        block_outline_color: args.block_outline_color,
        builtin_overlays,
        line_color_selection: (args.fishing_line_color.is_some()
            || args.block_outline_color.is_some()
            || args.rainbow_fishing_line
            || args.rainbow_block_outline)
            .then_some(crate::compat::line_colors::ColorSelection {
                fishing: args.fishing_line_color.is_some() || args.rainbow_fishing_line,
                outline: args.block_outline_color.is_some() || args.rainbow_block_outline,
            }),
        line_widths: crate::compat::line_colors::Widths {
            fishing: args.fishing_line_width,
            outline: args.block_outline_width,
        },
        rainbow: crate::compat::rainbow::Settings {
            glint: args.rainbow_glint,
            crosshair: args.rainbow_crosshair,
            fishing_line: args.rainbow_fishing_line,
            block_outline: args.rainbow_block_outline,
        },
        extra_renames: args
            .renames
            .as_deref()
            .map(load_extra_renames)
            .transpose()?
            .unwrap_or_default(),
        custom_items: args
            .custom_items
            .as_deref()
            .map(|path| -> Result<Vec<crate::compat::custom_items::Rule>> {
                Ok(serde_json::from_slice(
                    &std::fs::read(path).context("无法读取物品自定义规则")?,
                )?)
            })
            .transpose()?
            .unwrap_or_default(),
    };
    let report = convert_pack(&source, &output, &options, |step, percent| {
        if !args.quiet {
            eprint!("\r  [{percent:3}%] {step:<22}");
            let _ = std::io::stderr().flush();
        }
    })?;
    if !args.quiet {
        eprintln!("\n");
        println!("{}", report.render());
    }
    if let Some(path) = args.report_json {
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
    }
    println!("✔ Fast Pack 完成：{}", output.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_background_arguments_validate_hex_and_preserve_independent_options() {
        let args = Arguments::try_parse_from([
            "fast-pack",
            "source.zip",
            "--name-background-color",
            "#123",
            "--rainbow-name-background",
        ])
        .unwrap();
        assert_eq!(args.name_background_color, Some([17, 34, 51]));
        assert!(args.rainbow_name_background);
        assert!(args.fishing_line_color.is_none() && args.block_outline_color.is_none());
        assert!(
            Arguments::try_parse_from([
                "fast-pack",
                "source.zip",
                "--name-background-color",
                "broken"
            ])
            .is_err()
        );
    }

    #[test]
    fn removed_appearance_flag_is_rejected_but_independent_compatibility_remains() {
        assert!(
            Arguments::try_parse_from(["fast-pack", "pack.zip", "--builtin", "heypixel-pack"])
                .is_err()
        );
        let args = Arguments::try_parse_from([
            "fast-pack",
            "pack.zip",
            "--builtin",
            "sharpness",
            "--no-heypixel",
        ])
        .unwrap();
        assert_eq!(
            args.builtin,
            vec![crate::builtin_overlays::Feature::Sharpness]
        );
        assert!(args.no_heypixel);
    }

    #[test]
    fn width_arguments_accept_independent_values_and_reject_invalid_input() {
        let args = Arguments::try_parse_from([
            "fast-pack",
            "pack.zip",
            "--fishing-line-width",
            "2.5",
            "--block-outline-width",
            "3",
        ])
        .unwrap();
        assert_eq!(args.fishing_line_width, Some(2.5));
        assert_eq!(args.block_outline_width, Some(3.0));
        assert!(args.fishing_line_color.is_none() && args.block_outline_color.is_none());
        for width in ["NaN", "inf", "0", "8.1"] {
            assert!(
                Arguments::try_parse_from(["fast-pack", "pack.zip", "--fishing-line-width", width])
                    .is_err()
            );
        }
    }
}
