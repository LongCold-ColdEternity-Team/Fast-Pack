use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs as disk;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Node {
    path: PathBuf,
    bytes: Option<Arc<[u8]>>,
}

pub(crate) struct Store {
    root: PathBuf,
    nodes: HashMap<PathBuf, Node>,
    source: Option<zip::ZipArchive<Cursor<Arc<[u8]>>>>,
    originals: HashMap<usize, (usize, Arc<[u8]>)>,
}

thread_local! {
    static ACTIVE: RefCell<Option<Arc<Mutex<Store>>>> = const { RefCell::new(None) };
}

pub(crate) struct Mount(Option<Arc<Mutex<Store>>>);

pub(crate) fn context() -> Option<Arc<Mutex<Store>>> {
    ACTIVE.with(|active| active.borrow().clone())
}

pub(crate) fn enter_context(context: Option<Arc<Mutex<Store>>>) -> Mount {
    Mount(ACTIVE.with(|active| active.replace(context)))
}

impl Mount {
    pub(crate) fn for_pack(source: &Path, root: &Path) -> io::Result<Option<Self>> {
        const BUDGET: u64 = 256 * 1024 * 1024;
        let mut total = 0_u64;
        if source.is_dir() {
            for entry in walkdir::WalkDir::new(source).min_depth(1) {
                let entry = entry.map_err(io::Error::other)?;
                if entry.file_type().is_file() {
                    total = total.saturating_add(entry.metadata().map_err(io::Error::other)?.len());
                    if total > BUDGET {
                        return Ok(None);
                    }
                }
            }
        } else {
            // RAR is decoded sequentially to the temporary directory, including
            // solid archives, without buffering every unpacked file in RAM.
            if crate::archive::is_rar(source)? {
                return Ok(None);
            }
            if disk::metadata(source)?.len() > BUDGET {
                return Ok(None);
            }
            let mut zip = zip::ZipArchive::new(crate::archive::ArchiveReader::new(
                disk::File::open(source)?,
            )?)
            .map_err(io::Error::other)?;
            for index in 0..zip.len() {
                total =
                    total.saturating_add(zip.by_index_raw(index).map_err(io::Error::other)?.size());
                if total > BUDGET {
                    return Ok(None);
                }
            }
        }
        Ok(Some(Self::new(root)))
    }
    pub(crate) fn new(root: &Path) -> Self {
        let mut store = Store {
            root: key(root),
            nodes: HashMap::new(),
            source: None,
            originals: HashMap::new(),
        };
        store.directories(root);
        let current = Arc::new(Mutex::new(store));
        Self(ACTIVE.with(|active| active.replace(Some(current))))
    }
}

pub(crate) fn mounted(path: &Path) -> bool {
    active(path).is_some()
}

pub(crate) fn source_archive(root: &Path) -> Option<zip::ZipArchive<Cursor<Arc<[u8]>>>> {
    active(root).and_then(|store| store.lock().unwrap().source.clone())
}

pub(crate) fn set_source(root: &Path, source: zip::ZipArchive<Cursor<Arc<[u8]>>>) {
    if let Some(store) = active(root) {
        store.lock().unwrap().source = Some(source);
    }
}

pub(crate) fn retain_original(path: &Path, index: usize, bytes: Arc<[u8]>) {
    if let Some(store) = active(path) {
        store
            .lock()
            .unwrap()
            .originals
            .insert(bytes.as_ptr() as usize, (index, bytes));
    }
}

pub(crate) type OutputEntry = (String, Arc<[u8]>, Option<usize>);

pub(crate) fn output_entries(root: &Path) -> Option<Vec<OutputEntry>> {
    let store = active(root)?;
    let store = store.lock().unwrap();
    let root_key = key(root);
    let mut result: Vec<_> = store
        .nodes
        .iter()
        .filter_map(|(name, node)| {
            if !name.starts_with(&root_key) {
                return None;
            }
            let bytes = node.bytes.as_ref()?.clone();
            let relative = node
                .path
                .strip_prefix(root)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            let index = store
                .originals
                .get(&(bytes.as_ptr() as usize))
                .map(|(index, _)| *index);
            Some((relative, bytes, index))
        })
        .collect();
    result.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    Some(result)
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if active(path).is_some() {
        return write(path, bytes);
    }
    let mut temporary = tempfile::NamedTempFile::new_in(
        path.parent()
            .ok_or_else(|| io::Error::other("缺少父目录"))?,
    )?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

impl Drop for Mount {
    fn drop(&mut self) {
        ACTIVE.with(|active| active.replace(self.0.take()));
    }
}

fn key(path: &Path) -> PathBuf {
    if cfg!(windows)
        && !path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return PathBuf::from(path.to_string_lossy().to_lowercase());
    }
    let mut output = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                output.pop();
            }
            value => output.push(value.as_os_str()),
        }
    }
    if cfg!(windows) {
        PathBuf::from(output.to_string_lossy().to_lowercase())
    } else {
        output
    }
}

fn native_path(path: &Path) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(path.to_string_lossy().replace('/', "\\"))
    } else {
        path.into()
    }
}

fn active(path: &Path) -> Option<Arc<Mutex<Store>>> {
    ACTIVE
        .with(|value| value.borrow().as_ref().cloned())
        .filter(|store| key(path).starts_with(&store.lock().unwrap().root))
}

fn missing(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("文件不存在: {}", path.display()),
    )
}

impl Store {
    fn directories(&mut self, path: &Path) {
        if self.nodes.contains_key(&key(path)) {
            return;
        }
        for parent in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
            let name = key(parent);
            if name.starts_with(&self.root) {
                self.nodes.entry(name).or_insert_with(|| Node {
                    path: native_path(parent),
                    bytes: None,
                });
            }
        }
    }

    fn put(&mut self, path: &Path, bytes: Arc<[u8]>) {
        if let Some(parent) = path.parent() {
            self.directories(parent);
        }
        let name = key(path);
        let path = self
            .nodes
            .get(&name)
            .map(|n| n.path.clone())
            .unwrap_or_else(|| native_path(path));
        self.nodes.insert(
            name,
            Node {
                path,
                bytes: Some(bytes),
            },
        );
    }
}

pub(crate) trait PathExt {
    fn pack_exists(&self) -> bool;
    fn pack_is_file(&self) -> bool;
    fn pack_is_dir(&self) -> bool;
    fn pack_canonicalize(&self) -> io::Result<PathBuf>;
}

impl PathExt for Path {
    fn pack_exists(&self) -> bool {
        if let Some(store) = active(self) {
            store.lock().unwrap().nodes.contains_key(&key(self))
        } else {
            self.exists()
        }
    }
    fn pack_is_file(&self) -> bool {
        if let Some(store) = active(self) {
            store
                .lock()
                .unwrap()
                .nodes
                .get(&key(self))
                .is_some_and(|n| n.bytes.is_some())
        } else {
            self.is_file()
        }
    }
    fn pack_is_dir(&self) -> bool {
        if let Some(store) = active(self) {
            store
                .lock()
                .unwrap()
                .nodes
                .get(&key(self))
                .is_some_and(|n| n.bytes.is_none())
        } else {
            self.is_dir()
        }
    }
    fn pack_canonicalize(&self) -> io::Result<PathBuf> {
        if let Some(store) = active(self) {
            store
                .lock()
                .unwrap()
                .nodes
                .get(&key(self))
                .map(|n| n.path.clone())
                .ok_or_else(|| missing(self))
        } else {
            self.canonicalize()
        }
    }
}

pub(crate) fn read_shared(path: impl AsRef<Path>) -> io::Result<Arc<[u8]>> {
    let path = path.as_ref();
    if let Some(store) = active(path) {
        store
            .lock()
            .unwrap()
            .nodes
            .get(&key(path))
            .and_then(|n| n.bytes.clone())
            .ok_or_else(|| missing(path))
    } else {
        disk::read(path).map(Arc::from)
    }
}

pub(crate) fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    read_shared(path).map(|bytes| bytes.to_vec())
}

pub(crate) fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    String::from_utf8(read(path)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub(crate) fn write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    write_shared(path.as_ref(), Arc::from(bytes.as_ref()))
}

pub(crate) fn write_shared(path: &Path, bytes: Arc<[u8]>) -> io::Result<()> {
    if let Some(store) = active(path) {
        store.lock().unwrap().put(path, bytes);
        Ok(())
    } else {
        disk::write(path, bytes.as_ref())
    }
}

pub(crate) fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    if let Some(store) = active(path) {
        store.lock().unwrap().directories(path);
        Ok(())
    } else {
        disk::create_dir_all(path)
    }
}

pub(crate) fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<u64> {
    let bytes = read_shared(from)?;
    let length = bytes.len() as u64;
    write_shared(to.as_ref(), bytes)?;
    Ok(length)
}

pub(crate) fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    if let Some(store) = active(path) {
        store
            .lock()
            .unwrap()
            .nodes
            .remove(&key(path))
            .map(|_| ())
            .ok_or_else(|| missing(path))
    } else {
        disk::remove_file(path)
    }
}

pub(crate) fn remove_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    if let Some(store) = active(path) {
        let mut store = store.lock().unwrap();
        let name = key(path);
        if store
            .nodes
            .keys()
            .any(|p| p != &name && p.starts_with(&name))
        {
            return Err(io::Error::other("文件夹非空"));
        }
        store
            .nodes
            .remove(&name)
            .map(|_| ())
            .ok_or_else(|| missing(path))
    } else {
        disk::remove_dir(path)
    }
}

pub(crate) fn remove_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    if let Some(store) = active(path) {
        let mut store = store.lock().unwrap();
        let name = key(path);
        store.nodes.retain(|p, _| !p.starts_with(&name));
        Ok(())
    } else {
        disk::remove_dir_all(path)
    }
}

pub(crate) fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    let from = from.as_ref();
    let to = to.as_ref();
    if let Some(store) = active(from) {
        let mut store = store.lock().unwrap();
        let from_key = key(from);
        if store
            .nodes
            .get(&from_key)
            .is_some_and(|node| node.bytes.is_some())
        {
            let mut node = store.nodes.remove(&from_key).unwrap();
            node.path = native_path(to);
            if let Some(parent) = to.parent() {
                store.directories(parent);
            }
            store.nodes.insert(key(to), node);
            return Ok(());
        }
        if !store.nodes.contains_key(&from_key) {
            return Err(missing(from));
        }
        let names: Vec<_> = store
            .nodes
            .keys()
            .filter(|name| name.starts_with(&from_key))
            .cloned()
            .collect();
        let mut moved = Vec::new();
        for name in names {
            let mut node = store.nodes.remove(&name).unwrap();
            let relative: PathBuf = node
                .path
                .components()
                .skip(from.components().count())
                .collect();
            node.path = if relative.as_os_str().is_empty() {
                native_path(to)
            } else {
                native_path(&to.join(relative))
            };
            moved.push((key(&node.path), node));
        }
        if let Some(parent) = to.parent() {
            store.directories(parent);
        }
        store.nodes.extend(moved);
        Ok(())
    } else {
        disk::rename(from, to)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct FileType {
    file: bool,
    directory: bool,
    link: bool,
}
impl FileType {
    pub(crate) fn is_file(self) -> bool {
        self.file
    }
    pub(crate) fn is_dir(self) -> bool {
        self.directory
    }
    pub(crate) fn is_symlink(self) -> bool {
        self.link
    }
}

pub(crate) struct Metadata {
    length: u64,
    kind: FileType,
}
impl Metadata {
    pub(crate) fn len(&self) -> u64 {
        self.length
    }
    pub(crate) fn file_type(&self) -> FileType {
        self.kind
    }
}

pub(crate) fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    let path = path.as_ref();
    if let Some(store) = active(path) {
        let store = store.lock().unwrap();
        let node = store.nodes.get(&key(path)).ok_or_else(|| missing(path))?;
        Ok(Metadata {
            length: node.bytes.as_ref().map_or(0, |b| b.len() as u64),
            kind: FileType {
                file: node.bytes.is_some(),
                directory: node.bytes.is_none(),
                link: false,
            },
        })
    } else {
        let value = disk::symlink_metadata(path)?;
        Ok(Metadata {
            length: value.len(),
            kind: FileType {
                file: value.is_file(),
                directory: value.is_dir(),
                link: value.file_type().is_symlink(),
            },
        })
    }
}

pub(crate) fn symlink_metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    metadata(path)
}

pub(crate) struct ReadEntry(PathBuf);
impl ReadEntry {
    pub(crate) fn path(&self) -> PathBuf {
        self.0.clone()
    }
    pub(crate) fn file_name(&self) -> std::ffi::OsString {
        self.0.file_name().unwrap_or_default().into()
    }
    pub(crate) fn file_type(&self) -> io::Result<FileType> {
        Ok(metadata(&self.0)?.kind)
    }
}

pub(crate) fn read_dir(
    path: impl AsRef<Path>,
) -> io::Result<std::vec::IntoIter<io::Result<ReadEntry>>> {
    let path = path.as_ref();
    let mut entries = if let Some(store) = active(path) {
        let store = store.lock().unwrap();
        let name = key(path);
        if !store.nodes.get(&name).is_some_and(|n| n.bytes.is_none()) {
            return Err(missing(path));
        }
        store
            .nodes
            .iter()
            .filter(|(p, _)| p.parent() == Some(name.as_path()))
            .map(|(_, n)| Ok(ReadEntry(n.path.clone())))
            .collect::<Vec<_>>()
    } else {
        disk::read_dir(path)?
            .map(|e| e.map(|e| ReadEntry(e.path())))
            .collect()
    };
    entries.sort_by(|a, b| {
        a.as_ref()
            .ok()
            .map(|e| e.0.as_os_str())
            .cmp(&b.as_ref().ok().map(|e| e.0.as_os_str()))
    });
    Ok(entries.into_iter())
}

pub(crate) struct WalkEntry {
    path: PathBuf,
    kind: FileType,
}
impl WalkEntry {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn into_path(self) -> PathBuf {
        self.path
    }
    pub(crate) fn file_name(&self) -> &OsStr {
        self.path.file_name().unwrap_or_default()
    }
    pub(crate) fn file_type(&self) -> FileType {
        self.kind
    }
    pub(crate) fn metadata(&self) -> io::Result<Metadata> {
        metadata(&self.path)
    }
}

pub(crate) struct WalkDir {
    root: PathBuf,
    minimum: usize,
}
impl WalkDir {
    pub(crate) fn sort_by_file_name(self) -> Self {
        self
    }
    pub(crate) fn follow_links(self, _: bool) -> Self {
        self
    }
    pub(crate) fn new(path: impl AsRef<Path>) -> Self {
        Self {
            root: path.as_ref().into(),
            minimum: 0,
        }
    }
    pub(crate) fn min_depth(mut self, depth: usize) -> Self {
        self.minimum = depth;
        self
    }
}
impl IntoIterator for WalkDir {
    type Item = io::Result<WalkEntry>;
    type IntoIter = std::vec::IntoIter<Self::Item>;
    fn into_iter(self) -> Self::IntoIter {
        let mut entries = if let Some(store) = active(&self.root) {
            let store = store.lock().unwrap();
            let root = key(&self.root);
            if !store.nodes.contains_key(&root) {
                vec![Err(missing(&self.root))]
            } else {
                store
                    .nodes
                    .iter()
                    .filter_map(|(name, node)| {
                        let relative = name.strip_prefix(&root).ok()?;
                        if relative.components().count() < self.minimum {
                            return None;
                        }
                        Some(Ok(WalkEntry {
                            path: node.path.clone(),
                            kind: FileType {
                                file: node.bytes.is_some(),
                                directory: node.bytes.is_none(),
                                link: false,
                            },
                        }))
                    })
                    .collect()
            }
        } else {
            walkdir::WalkDir::new(self.root)
                .min_depth(self.minimum)
                .into_iter()
                .map(|entry| {
                    entry
                        .map(|entry| WalkEntry {
                            kind: FileType {
                                file: entry.file_type().is_file(),
                                directory: entry.file_type().is_dir(),
                                link: entry.file_type().is_symlink(),
                            },
                            path: entry.into_path(),
                        })
                        .map_err(io::Error::other)
                })
                .collect()
        };
        entries.sort_by(|a, b| {
            a.as_ref()
                .ok()
                .map(|e| e.path.as_os_str())
                .cmp(&b.as_ref().ok().map(|e| e.path.as_os_str()))
        });
        entries.into_iter()
    }
}

pub(crate) enum File {
    Disk(disk::File),
    Reading(Cursor<Arc<[u8]>>),
    Writing {
        buffer: Cursor<Vec<u8>>,
        path: PathBuf,
        store: Arc<Mutex<Store>>,
    },
}
impl File {
    pub(crate) fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        if active(path).is_some() {
            Ok(Self::Reading(Cursor::new(read_shared(path)?)))
        } else {
            disk::File::open(path).map(Self::Disk)
        }
    }
    pub(crate) fn create(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        if let Some(store) = active(path) {
            store.lock().unwrap().put(path, Arc::from([]));
            Ok(Self::Writing {
                buffer: Cursor::new(Vec::new()),
                path: path.into(),
                store,
            })
        } else {
            disk::File::create(path).map(Self::Disk)
        }
    }
}
impl Read for File {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Disk(f) => f.read(bytes),
            Self::Reading(f) => f.read(bytes),
            Self::Writing { buffer, .. } => buffer.read(bytes),
        }
    }
}
impl Seek for File {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        match self {
            Self::Disk(f) => f.seek(from),
            Self::Reading(f) => f.seek(from),
            Self::Writing { buffer, .. } => buffer.seek(from),
        }
    }
}
impl Write for File {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Disk(f) => f.write(bytes),
            Self::Reading(_) => Err(io::Error::new(io::ErrorKind::PermissionDenied, "只读文件")),
            Self::Writing { buffer, .. } => buffer.write(bytes),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Disk(f) => f.flush(),
            Self::Reading(_) => Ok(()),
            Self::Writing {
                buffer,
                path,
                store,
            } => {
                store
                    .lock()
                    .unwrap()
                    .put(path, Arc::from(buffer.get_ref().as_slice()));
                Ok(())
            }
        }
    }
}
impl Drop for File {
    fn drop(&mut self) {
        if let Self::Writing {
            buffer,
            path,
            store,
        } = self
        {
            store
                .lock()
                .unwrap()
                .put(path, Arc::from(buffer.get_ref().as_slice()));
        }
    }
}

pub(crate) fn image_reader(
    path: impl AsRef<Path>,
) -> io::Result<image::ImageReader<std::io::BufReader<File>>> {
    Ok(image::ImageReader::new(std::io::BufReader::new(
        File::open(path)?,
    )))
}
pub(crate) fn open_image(path: impl AsRef<Path>) -> image::ImageResult<image::DynamicImage> {
    image_reader(path)?.with_guessed_format()?.decode()
}
pub(crate) fn save_image(
    image: &image::RgbaImage,
    path: impl AsRef<Path>,
) -> image::ImageResult<()> {
    let mut output = File::create(path)?;
    image::codecs::png::PngEncoder::new(&mut output).write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgba8,
    )?;
    output.flush()?;
    Ok(())
}

use image::ImageEncoder;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_stay_in_memory_and_directory_renames_preserve_children() {
        let work = tempfile::tempdir().unwrap();
        let guard = Mount::new(work.path());
        let source = work.path().join("assets/Old/Child.PNG");
        write(&source, b"original").unwrap();
        let backup = work.path().join("fastpack/original.png");
        copy(&source, &backup).unwrap();
        write(&source, b"modified").unwrap();
        rename(
            work.path().join("assets/Old"),
            work.path().join("assets/new"),
        )
        .unwrap();
        let target = work.path().join("assets/new/Child.PNG");
        assert_eq!(read(&target).unwrap(), b"modified");
        assert_eq!(read(&backup).unwrap(), b"original");
        assert_eq!(
            read_dir(target.parent().unwrap())
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .file_name(),
            "Child.PNG"
        );
        assert!(!source.pack_exists());
        assert!(!target.exists());
        let paths: Vec<_> = WalkDir::new(work.path())
            .min_depth(1)
            .into_iter()
            .map(|entry| entry.unwrap().into_path())
            .collect();
        assert!(paths.contains(&target));
        drop(guard);
        assert!(!target.pack_exists());
    }

    #[test]
    fn image_and_file_handles_round_trip_inside_isolated_mounts() {
        let work = tempfile::tempdir().unwrap();
        let _guard = Mount::new(work.path());
        let first = work.path().join("a.txt");
        {
            let mut handle = File::create(&first).unwrap();
            handle.write_all(b"hello").unwrap();
            handle.seek(SeekFrom::Start(1)).unwrap();
            handle.write_all(b"A").unwrap();
        }
        assert_eq!(read(&first).unwrap(), b"hAllo");
        let image = image::RgbaImage::from_pixel(32, 32, image::Rgba([12, 34, 56, 78]));
        let path = work.path().join("texture.png");
        save_image(&image, &path).unwrap();
        assert_eq!(open_image(&path).unwrap().to_rgba8(), image);
        let nested = tempfile::tempdir().unwrap();
        {
            let _guard = Mount::new(nested.path());
            write(nested.path().join("b.txt"), b"different").unwrap();
            assert!(!first.pack_exists());
        }
        assert_eq!(read(first).unwrap(), b"hAllo");
    }
}
