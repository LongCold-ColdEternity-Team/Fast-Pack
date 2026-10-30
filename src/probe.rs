use crate::converter::{PackInfo, compact_json, find_pack_root, metadata_format, suggest_target};
use crate::tables::{LATEST_VERSION, version_label_of};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use walkdir::WalkDir;
use zip::ZipArchive;

const METADATA_LIMIT: u64 = 1024 * 1024;
const VARIANTS: &str = "fastpack/variants/";

#[derive(Default)]
struct Scan {
    legacy: bool,
    modern: bool,
    variants: BTreeSet<String>,
}

impl Scan {
    fn entry(&mut self, info: &mut PackInfo, name: &str, directory: bool) {
        let lower = name.to_ascii_lowercase();
        info.has_assets |= directory && lower == "assets" || lower.starts_with("assets/");
        self.legacy |= matches!(
            lower.as_str(),
            "assets/minecraft/textures/blocks" | "assets/minecraft/textures/items"
        ) && directory
            || lower.starts_with("assets/minecraft/textures/blocks/")
            || lower.starts_with("assets/minecraft/textures/items/");
        self.modern |= matches!(
            lower.as_str(),
            "assets/minecraft/textures/block" | "assets/minecraft/textures/item"
        ) && directory
            || lower.starts_with("assets/minecraft/textures/block/")
            || lower.starts_with("assets/minecraft/textures/item/");
        let folder = if directory {
            Some(name)
        } else {
            name.rsplit_once('/').map(|(parent, _)| parent)
        };
        if let Some(folder) = folder
            && folder
                .get(..VARIANTS.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(VARIANTS))
            && let Some(variant) = folder[VARIANTS.len()..]
                .split('/')
                .next()
                .filter(|name| !name.is_empty())
        {
            self.variants.insert(variant.into());
        }
        if !directory {
            info.file_count += 1;
            info.has_mcmeta |= lower == "pack.mcmeta";
            info.texture_count +=
                usize::from(lower.starts_with("assets/") && lower.ends_with(".png"));
            info.features.observe_file(&lower);
        }
    }

    fn finish(self, info: &mut PackInfo, metadata: Option<Value>) {
        if let Some(value) = metadata {
            info.src_format = metadata_format(&value);
            if let Some(description) = value.get("pack").and_then(|pack| pack.get("description")) {
                info.description = description
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| compact_json(description));
            }
        }
        info.features.version_variants = self.variants.into_iter().collect();
        info.layout = if self.legacy {
            Some("legacy".into())
        } else if self.modern {
            Some("modern".into())
        } else {
            None
        };
        info.src_format = info.src_format.or_else(|| self.legacy.then_some(1));
        info.version_label = info
            .src_format
            .filter(|value| *value != 0)
            .map(version_label_of);
        info.suggested_target = suggest_target(info.src_format).into();
        info.ok = info.has_assets || info.has_mcmeta;
    }
}

fn metadata(reader: impl Read) -> Result<Option<Value>> {
    let mut bytes = Vec::new();
    reader.take(METADATA_LIMIT + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= METADATA_LIMIT,
        "pack.mcmeta 超过 1 MiB，无法快速识别"
    );
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    Ok(serde_json::from_slice(bytes).ok())
}

fn directory(source: &Path, info: &mut PackInfo) -> Result<()> {
    let root = find_pack_root(source)?;
    let mut scan = Scan::default();
    for entry in WalkDir::new(&root).min_depth(1) {
        let entry = entry?;
        ensure!(
            !entry.file_type().is_symlink(),
            "不支持符号链接: {}",
            entry.path().display()
        );
        let name = entry
            .path()
            .strip_prefix(&root)?
            .to_string_lossy()
            .replace('\\', "/");
        scan.entry(info, &name, entry.file_type().is_dir());
    }
    let value = if info.has_mcmeta {
        metadata(File::open(root.join("pack.mcmeta"))?)?
    } else {
        None
    };
    scan.finish(info, value);
    Ok(())
}

pub(crate) fn normalized(name: &str) -> Option<String> {
    let name = name.replace('\\', "/");
    if name.starts_with('/') || name.contains(':') || name.split('/').any(|part| part == "..") {
        return None;
    }
    let name = name
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>()
        .join("/");
    (!name.is_empty()).then_some(name)
}

fn scan_entries(
    entries: HashMap<String, (usize, bool)>,
    info: &mut PackInfo,
) -> (Scan, Option<usize>) {
    let root_metadata = entries
        .iter()
        .any(|(name, (_, directory))| !directory && name.eq_ignore_ascii_case("pack.mcmeta"));
    let prefix = if root_metadata {
        String::new()
    } else {
        let mut metadata_roots = BTreeSet::new();
        let mut asset_roots = BTreeSet::new();
        for (name, (_, directory)) in &entries {
            if let Some((root, rest)) = name.split_once('/') {
                if !directory && rest.eq_ignore_ascii_case("pack.mcmeta") {
                    metadata_roots.insert(root);
                }
                if rest.eq_ignore_ascii_case("assets") && *directory
                    || rest
                        .get(..7)
                        .is_some_and(|value| value.eq_ignore_ascii_case("assets/"))
                {
                    asset_roots.insert(root);
                }
            }
        }
        metadata_roots
            .first()
            .or_else(|| asset_roots.first())
            .map(|name| format!("{name}/"))
            .unwrap_or_default()
    };
    let mut scan = Scan::default();
    let mut metadata_index = None;
    for (name, (index, directory)) in entries {
        let Some(name) = name.strip_prefix(&prefix) else {
            continue;
        };
        scan.entry(info, name, directory);
        if !directory && name.eq_ignore_ascii_case("pack.mcmeta") {
            metadata_index = Some(index);
        }
    }
    (scan, metadata_index)
}

fn archive(source: &Path, info: &mut PackInfo) -> Result<()> {
    if crate::archive::is_rar(source)? {
        let archive = crate::archive::open_rar(source)?;
        let entries = archive
            .members()
            .enumerate()
            .filter(|(_, member)| !crate::archive::rar_link(&member.meta))
            .filter_map(|(index, member)| {
                crate::archive::rar_name(&member.meta)
                    .map(|name| (name, (index, member.meta.is_directory)))
            })
            .collect();
        let (scan, metadata_index) = scan_entries(entries, info);
        let value = if let Some(index) = metadata_index {
            let member = archive.members().nth(index).unwrap();
            ensure!(
                member.meta.unpacked_size <= METADATA_LIMIT,
                "pack.mcmeta 超过 1 MiB，无法快速识别"
            );
            archive
                .read_member_at_with_options(index, crate::archive::rar_options())?
                .map(|bytes| metadata(bytes.as_slice()))
                .transpose()?
                .flatten()
        } else {
            None
        };
        scan.finish(info, value);
        return Ok(());
    }
    let reader = crate::archive::ArchiveReader::new(
        File::open(source).with_context(|| format!("无法读取 {}", source.display()))?,
    )?;
    let mut archive = ZipArchive::new(reader).context("不是有效的 ZIP 或 RAR 材质包")?;
    let mut entries = HashMap::with_capacity(archive.len());
    for index in 0..archive.len() {
        let Some(raw) = archive.name_for_index(index) else {
            continue;
        };
        if let Some(name) = normalized(raw) {
            entries.insert(name, (index, raw.ends_with('/') || raw.ends_with('\\')));
        }
    }
    let (scan, metadata_index) = scan_entries(entries, info);
    let value = if let Some(index) = metadata_index {
        let entry = archive.by_index(index)?;
        ensure!(
            entry.size() <= METADATA_LIMIT,
            "pack.mcmeta 超过 1 MiB，无法快速识别"
        );
        metadata(entry)?
    } else {
        None
    };
    scan.finish(info, value);
    Ok(())
}

pub(crate) fn probe_pack(path: &Path) -> PackInfo {
    let mut info = PackInfo {
        ok: false,
        path: path.display().to_string(),
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into(),
        src_format: None,
        version_label: None,
        file_count: 0,
        texture_count: 0,
        has_mcmeta: false,
        has_assets: false,
        description: String::new(),
        layout: None,
        suggested_target: LATEST_VERSION.into(),
        error: None,
        features: Default::default(),
    };
    let result = if path.is_dir() {
        directory(path, &mut info)
    } else {
        archive(path, &mut info)
    };
    if let Err(error) = result {
        info.error = Some(format!("{error:#}"));
        info.ok = false;
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, OpenOptions};
    use std::io::{Seek, SeekFrom, Write};
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    fn zip(path: &Path, prefix: &str, files: &[(&str, &[u8])]) {
        let mut writer = ZipWriter::new(File::create(path).unwrap());
        for (name, bytes) in files {
            writer
                .start_file(format!("{prefix}{name}"), SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn wrapped_zip_and_directory_agree_on_metadata_and_features() {
        let work = tempfile::tempdir().unwrap();
        let source = work.path().join("folder");
        let files: &[(&str, &[u8])] = &[
            (
                "pack.mcmeta",
                br#"{"pack":{"min_format":[69,0],"description":{"text":"sample"}}}"#,
            ),
            ("assets/minecraft/textures/item/sword.png", b"texture"),
            (
                "assets/minecraft/optifine/sky/world0/sky1.properties",
                b"source=sky.png",
            ),
            ("assets/fabricskyboxes/sky/cloud.json", b"{}"),
            ("assets/nuit/sky/stars.json", b"{}"),
            ("assets/minecraft/shaders/core/rendertype.vsh", b"shader"),
            ("assets/minecraft/post_effect/blur.json", b"{}"),
            ("fastpack/variants/Custom/assets/a.txt", b"variant"),
            ("readme.txt", b"readme"),
        ];
        for (name, bytes) in files {
            let path = source.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        let archive = work.path().join("pack.zip");
        zip(&archive, "wrapped/", files);
        let folder = probe_pack(&source);
        let zipped = probe_pack(&archive);
        assert!(zipped.ok);
        assert_eq!(zipped.src_format, Some(69));
        assert_eq!(zipped.file_count, files.len());
        assert_eq!(zipped.features.sky_count(), 3);
        assert_eq!(zipped.features.version_variants, ["Custom"]);
        let mut folder = serde_json::to_value(folder).unwrap();
        let mut zipped = serde_json::to_value(zipped).unwrap();
        for value in [&mut folder, &mut zipped] {
            value.as_object_mut().unwrap().remove("path");
            value.as_object_mut().unwrap().remove("name");
        }
        assert_eq!(folder, zipped);
    }

    #[test]
    fn skips_texture_payloads_even_when_their_crc_is_invalid() {
        let work = tempfile::tempdir().unwrap();
        let archive = work.path().join("pack.zip");
        zip(
            &archive,
            "",
            &[
                (
                    "pack.mcmeta",
                    br#"{"pack":{"pack_format":1,"description":"legacy"}}"#,
                ),
                ("assets/minecraft/textures/items/sword.png", b"unchanged"),
            ],
        );
        let offset = {
            let mut zip = ZipArchive::new(File::open(&archive).unwrap()).unwrap();
            zip.by_index_raw(1).unwrap().data_start()
        };
        let mut output = OpenOptions::new().write(true).open(&archive).unwrap();
        output.seek(SeekFrom::Start(offset)).unwrap();
        output.write_all(b"!").unwrap();
        drop(output);
        let info = probe_pack(&archive);
        assert!(info.ok, "{:?}", info.error);
        assert_eq!(info.texture_count, 1);
        assert_eq!(info.src_format, Some(1));
    }

    #[test]
    fn ignores_unsafe_entries_and_detects_legacy_without_metadata() {
        let work = tempfile::tempdir().unwrap();
        let archive = work.path().join("pack.zip");
        zip(
            &archive,
            "",
            &[
                ("../pack.mcmeta", br#"{"pack":{"pack_format":69}}"#),
                ("assets\\minecraft\\textures\\blocks\\stone.png", b"texture"),
            ],
        );
        let info = probe_pack(&archive);
        assert!(info.ok);
        assert!(!info.has_mcmeta);
        assert_eq!(info.file_count, 1);
        assert_eq!(info.src_format, Some(1));
        assert!(!work.path().join("pack.mcmeta").exists());
    }

    #[test]
    fn rejects_oversized_metadata_without_reading_the_whole_entry() {
        let work = tempfile::tempdir().unwrap();
        let archive = work.path().join("pack.zip");
        let bytes = vec![b' '; METADATA_LIMIT as usize + 1];
        zip(&archive, "", &[("pack.mcmeta", &bytes)]);
        let info = probe_pack(&archive);
        assert!(!info.ok);
        assert!(info.error.unwrap().contains("1 MiB"));
    }

    #[test]
    fn utf8_bom_format_is_shared_with_conversion_detection() {
        let work = tempfile::tempdir().unwrap();
        let metadata = "\u{feff}{\"pack\":{\"min_format\":[69,0]}}";
        fs::write(work.path().join("pack.mcmeta"), metadata).unwrap();
        assert_eq!(probe_pack(work.path()).src_format, Some(69));
        assert_eq!(crate::converter::detect_format(work.path()), Some(69));
    }
}
