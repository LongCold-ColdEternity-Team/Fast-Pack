use anyhow::{Context, Result, ensure};
use rars::{Archive, ArchiveReadOptions, ArchiveReader, ExtractionDecision};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

pub(crate) fn is_rar(source: &Path) -> io::Result<bool> {
    let mut signature = Vec::with_capacity(8);
    File::open(source)?.take(8).read_to_end(&mut signature)?;
    Ok(
        signature.starts_with(b"Rar!\x1a\x07\x00")
            || signature.starts_with(b"Rar!\x1a\x07\x01\x00"),
    )
}

pub(crate) fn rar_options() -> ArchiveReadOptions<'static> {
    ArchiveReadOptions::new()
        .with_max_header_count(1_000_000)
        .with_max_header_bytes(64 * 1024 * 1024)
        .with_rar50_buffered_decode_limit(8 * 1024 * 1024)
        .with_rar50_dictionary_size_limit(256 * 1024 * 1024)
        .with_max_reader_workspace_bytes(512 * 1024 * 1024)
}

pub(crate) fn open_rar(source: &Path) -> Result<Archive> {
    let archive = ArchiveReader::read_reader_with_options(
        super::ArchiveReader::new(File::open(source)?)?,
        rar_options(),
    )
    .context("无法读取 RAR 材质包；加密包或分卷包请先完整解压后导入文件夹")?;
    for member in archive.members() {
        ensure!(
            std::str::from_utf8(&member.meta.name).is_ok(),
            "RAR 文件名编码无法识别，请先解压后导入文件夹"
        );
        ensure!(
            !member.meta.is_encrypted,
            "RAR 材质包包含加密文件，请先解密后导入文件夹"
        );
        ensure!(
            !member.meta.is_split_before && !member.meta.is_split_after,
            "RAR 材质包包含分卷文件，请先完整解压所有分卷后导入文件夹"
        );
    }
    Ok(archive)
}

pub(crate) fn rar_link(member: &rars::ArchiveMemberMeta) -> bool {
    member.is_redirection
        || member.file_attr & 0o170000 == 0o120000
            && matches!(
                (member.family, member.host_os),
                (rars::ArchiveFamily::Rar15To40, Some(3))
                    | (rars::ArchiveFamily::Rar50Plus, Some(1))
            )
}

pub(crate) fn rar_name(member: &rars::ArchiveMemberMeta) -> Option<String> {
    let name = crate::probe::normalized(std::str::from_utf8(&member.name).ok()?)?;
    // Win32 removes trailing dots/spaces and interprets device names even below
    // a directory. Refuse those aliases before joining the temporary root.
    for part in name.split('/') {
        let stem = part.split('.').next().unwrap().to_ascii_uppercase();
        if part.contains('\0')
            || part.ends_with(['.', ' '])
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || stem
                .strip_prefix("COM")
                .or_else(|| stem.strip_prefix("LPT"))
                .is_some_and(|number| {
                    matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
                })
        {
            return None;
        }
    }
    Some(name)
}

pub(crate) fn extract_rar(source: &Path, base: &Path) -> Result<()> {
    let archive = open_rar(source)?;
    archive
        .extract_with_control(rar_options(), |member| {
            if rar_link(&member.meta) {
                return Ok(ExtractionDecision::Skip);
            }
            let Some(name) = rar_name(&member.meta) else {
                return Ok(ExtractionDecision::Skip);
            };
            let target = base.join(name);
            if member.meta.is_directory {
                std::fs::create_dir_all(target)?;
                return Ok(ExtractionDecision::Skip);
            }
            std::fs::create_dir_all(target.parent().unwrap())?;
            Ok(ExtractionDecision::Extract(Box::new(File::create(target)?)))
        })
        .context("解压 RAR 材质包失败，未生成转换结果")?;
    Ok(())
}
