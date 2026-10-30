use fast_pack::{ConversionOptions, compat, convert_pack, converter::load_pack, probe_pack};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

const FIXTURES: &[(&str, &[u8])] = &[
    ("rar4", include_bytes!("fixtures/rar/rar4.rar")),
    ("rar5-solid", include_bytes!("fixtures/rar/rar5-solid.rar")),
];

fn contents(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut zip = ZipArchive::new(File::open(path).unwrap()).unwrap();
    (0..zip.len())
        .map(|index| {
            let mut entry = zip.by_index(index).unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            (entry.name().to_owned(), bytes)
        })
        .collect()
}

#[test]
fn renamed_rar_packs_can_be_probed_loaded_and_previewed() {
    let work = tempfile::tempdir().unwrap();
    for (name, bytes) in FIXTURES {
        for suffix in ["zip", "rar"] {
            let source = work.path().join(format!("§d{name}.{suffix}"));
            fs::write(&source, bytes).unwrap();
            let info = probe_pack(&source);
            assert!(info.ok, "{:?}", info.error);
            assert_eq!(info.src_format, Some(6));
            assert_eq!(info.file_count, 6);
            assert_eq!(info.texture_count, 3);
            let (_unpacked, root) = load_pack(&source).unwrap();
            assert_eq!(
                fs::read(root.join("author/作者.txt")).unwrap(),
                b"kept verbatim"
            );
            let expected = compat::sky::preview(&root).unwrap().unwrap();
            let actual = compat::sky::preview(&source).unwrap().unwrap();
            assert_eq!(actual.names, expected.names);
            assert_eq!(actual.image, expected.image);
            let crosshairs = fast_pack::crosshair::import(&source).unwrap();
            assert_eq!(crosshairs.len(), 1);
            assert!(crosshairs[0].name.contains(name));
            assert_eq!(fs::read(source).unwrap(), *bytes);
        }
    }
}

#[test]
fn rar_conversion_matches_the_same_resources_in_a_real_zip() {
    let work = tempfile::tempdir().unwrap();
    for (name, bytes) in FIXTURES {
        let source = work.path().join(format!("{name}.zip"));
        fs::write(&source, bytes).unwrap();
        let (_unpacked, root) = load_pack(&source).unwrap();
        let real_zip = work.path().join(format!("{name}-real.zip"));
        let mut zip = ZipWriter::new(File::create(&real_zip).unwrap());
        for entry in walkdir::WalkDir::new(&root) {
            let entry = entry.unwrap();
            if entry.file_type().is_file() {
                zip.start_file(
                    entry
                        .path()
                        .strip_prefix(&root)
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
        for target in ["1.8.9", "1.20.1", "1.21.4"] {
            let options = ConversionOptions {
                tgt_format: fast_pack::pack_format_of(target).unwrap(),
                target_version: Some(target.into()),
                auto_client_setup: false,
                heypixel_compat: false,
                ..Default::default()
            };
            let output = work.path().join(format!("{name}-{target}-rar.zip"));
            let reference = work.path().join(format!("{name}-{target}-zip.zip"));
            convert_pack(&source, &output, &options, |_, _| {}).unwrap();
            convert_pack(&real_zip, &reference, &options, |_, _| {}).unwrap();
            assert_eq!(contents(&output), contents(&reference), "{name}, {target}");
        }
        assert_eq!(fs::read(source).unwrap(), *bytes);
    }
}

// Independent stored RAR4 headers let us exercise names and failure cases that
// archive writers normally reject. Header checksums follow the RAR4 format.
fn block(out: &mut Vec<u8>, kind: u8, flags: u16, body: &[u8]) {
    let mut header = vec![kind];
    header.extend(flags.to_le_bytes());
    header.extend(((7 + body.len()) as u16).to_le_bytes());
    header.extend(body);
    out.extend((rars::crc32::crc32(&header) as u16).to_le_bytes());
    out.extend(header);
}

fn stored_rar(entries: &[(&str, &[u8], u16, u32)]) -> Vec<u8> {
    let mut out = b"Rar!\x1a\x07\0".to_vec();
    block(&mut out, 0x73, 0, &[0; 6]);
    for (name, data, flags, attributes) in entries {
        let mut header = Vec::new();
        header.extend((data.len() as u32).to_le_bytes());
        header.extend((data.len() as u32).to_le_bytes());
        header.push(3); // Unix attributes, including legacy symbolic links.
        header.extend(rars::crc32::crc32(data).to_le_bytes());
        header.extend(0_u32.to_le_bytes());
        header.extend([20, 0x30]); // Version 2.0, stored.
        header.extend((name.len() as u16).to_le_bytes());
        header.extend(attributes.to_le_bytes());
        header.extend(name.as_bytes());
        block(&mut out, 0x74, 0x8000 | flags, &header);
        out.extend_from_slice(data);
    }
    block(&mut out, 0x7b, 0, &[]);
    out
}

#[test]
fn rar_extraction_skips_traversal_and_links() {
    let work = tempfile::tempdir().unwrap();
    let source = work.path().join("unsafe.zip");
    let bytes = stored_rar(&[
        ("pack.mcmeta", br#"{"pack":{"pack_format":6}}"#, 0, 0),
        ("../escape", b"outside", 0, 0),
        (".. /escape", b"windows alias", 0, 0),
        ("C:/absolute", b"absolute", 0, 0),
        ("assets/NUL.txt", b"device alias", 0, 0),
        ("assets/link", b"../escape", 0, 0o120777),
        ("assets/kept.txt", b"safe", 0, 0),
    ]);
    fs::write(&source, bytes).unwrap();
    let info = probe_pack(&source);
    assert!(info.ok, "{:?}", info.error);
    assert_eq!(info.file_count, 2);
    let (_work, root) = load_pack(&source).unwrap();
    assert_eq!(fs::read(root.join("assets/kept.txt")).unwrap(), b"safe");
    assert!(!root.parent().unwrap().join("escape").exists());
    assert!(!root.join("assets/link").exists());
}

#[test]
fn encrypted_and_split_rar_members_have_actionable_errors() {
    let work = tempfile::tempdir().unwrap();
    for (flag, expected) in [(4, "加密"), (1, "分卷"), (2, "分卷")] {
        let source = work.path().join(format!("unsupported-{flag}.zip"));
        fs::write(&source, stored_rar(&[("pack.mcmeta", b"{}", flag, 0)])).unwrap();
        let info = probe_pack(&source);
        assert!(!info.ok);
        assert!(
            info.error.as_deref().unwrap().contains(expected),
            "{:?}",
            info.error
        );
        assert!(load_pack(&source).is_err());
    }
}

#[test]
fn corrupt_rar_payload_does_not_replace_an_existing_output() {
    let work = tempfile::tempdir().unwrap();
    let source = work.path().join("corrupt.zip");
    let output = work.path().join("existing.zip");
    let mut bytes = stored_rar(&[
        ("pack.mcmeta", br#"{"pack":{"pack_format":6}}"#, 0, 0),
        ("assets/kept.txt", b"verified payload", 0, 0),
    ]);
    let offset = bytes
        .windows(16)
        .position(|part| part == b"verified payload")
        .unwrap();
    bytes[offset] ^= 1;
    fs::write(&source, &bytes).unwrap();
    fs::write(&output, b"previous output").unwrap();
    let result = convert_pack(&source, &output, &ConversionOptions::default(), |_, _| {});
    assert!(result.is_err());
    assert_eq!(fs::read(output).unwrap(), b"previous output");
    assert_eq!(fs::read(source).unwrap(), bytes);
}
