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
                .warnings
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
