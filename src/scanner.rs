//! Parallel directory scanner that builds an in-memory size tree.
//!
//! Sizes are *allocated* bytes on disk (`st_blocks * 512`), which is what
//! actually consumes space, rather than the apparent file length. Hard-linked
//! files and directories reached twice (for example through APFS firmlinks
//! when scanning `/`) are only counted once.

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;

use rayon::prelude::*;

/// Maximum number of unreadable paths remembered for display.
const MAX_ERRORS_KEPT: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Folder,
    App,
    Video,
    Audio,
    Image,
    Archive,
    Document,
    Code,
    System,
    Other,
}

impl Kind {
    pub const LEGEND: [Kind; 10] = [
        Kind::Folder,
        Kind::App,
        Kind::Video,
        Kind::Audio,
        Kind::Image,
        Kind::Archive,
        Kind::Document,
        Kind::Code,
        Kind::System,
        Kind::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Folder => "Folder",
            Kind::App => "App / bundle",
            Kind::Video => "Video",
            Kind::Audio => "Audio",
            Kind::Image => "Image",
            Kind::Archive => "Archive / disk image",
            Kind::Document => "Document",
            Kind::Code => "Code / data",
            Kind::System => "System / library",
            Kind::Other => "Other",
        }
    }

    pub fn for_file(name: &str) -> Kind {
        let ext = match name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
            _ => return Kind::Other,
        };
        match ext.as_str() {
            "mp4" | "mov" | "m4v" | "mkv" | "avi" | "wmv" | "flv" | "webm" | "mpg" | "mpeg"
            | "3gp" | "mts" | "m2ts" | "prores" | "braw" | "r3d" => Kind::Video,
            "mp3" | "m4a" | "aac" | "wav" | "aif" | "aiff" | "flac" | "ogg" | "opus" | "wma"
            | "caf" | "alac" => Kind::Audio,
            "jpg" | "jpeg" | "png" | "gif" | "heic" | "heif" | "tif" | "tiff" | "bmp" | "webp"
            | "raw" | "cr2" | "cr3" | "nef" | "arw" | "dng" | "psd" | "svg" | "ico" | "icns" => {
                Kind::Image
            }
            "zip" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "tar" | "dmg" | "iso" | "img"
            | "pkg" | "sparseimage" | "sparsebundle" | "zst" | "lz4" | "xip" => Kind::Archive,
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "pages" | "numbers"
            | "key" | "txt" | "rtf" | "md" | "csv" | "epub" | "odt" | "ods" => Kind::Document,
            "rs" | "c" | "h" | "cpp" | "hpp" | "m" | "mm" | "swift" | "py" | "js" | "ts"
            | "tsx" | "jsx" | "java" | "kt" | "go" | "rb" | "php" | "html" | "css" | "json"
            | "xml" | "yaml" | "yml" | "toml" | "sql" | "sqlite" | "db" | "rlib" | "o" | "a"
            | "class" | "jar" | "wasm" | "pyc" => Kind::Code,
            "dylib" | "so" | "framework" | "kext" | "plist" | "log" | "cache" | "dat" | "bin"
            | "vmdk" | "vdi" | "qcow2" | "car" | "nib" | "strings" => Kind::System,
            _ => Kind::Other,
        }
    }

    pub fn for_dir(name: &str) -> Kind {
        let lower = name.to_ascii_lowercase();
        let bundle = [
            ".app", ".bundle", ".framework", ".plugin", ".appex", ".kext", ".photoslibrary",
            ".musiclibrary", ".xcarchive", ".sparsebundle", ".logicx", ".fcpbundle",
        ];
        if bundle.iter().any(|ext| lower.ends_with(ext)) {
            Kind::App
        } else {
            Kind::Folder
        }
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub name: Box<str>,
    /// Allocated bytes on disk, including all descendants.
    pub size: u64,
    /// Number of files at or below this node.
    pub files: u64,
    pub is_dir: bool,
    pub kind: Kind,
    /// Children sorted by size, largest first.
    pub children: Vec<Node>,
}

impl Node {
    pub fn file(name: impl Into<Box<str>>, size: u64) -> Node {
        let name = name.into();
        let kind = Kind::for_file(&name);
        Node { name, size, files: 1, is_dir: false, kind, children: Vec::new() }
    }

    pub fn dir(name: impl Into<Box<str>>, mut children: Vec<Node>) -> Node {
        let name = name.into();
        let kind = Kind::for_dir(&name);
        sort_children(&mut children);
        let size = children.iter().map(|c| c.size).sum();
        let files = children.iter().map(|c| c.files).sum();
        Node { name, size, files, is_dir: true, kind, children }
    }

    /// Follows a path of child indices from this node.
    pub fn at(&self, path: &[usize]) -> Option<&Node> {
        let mut node = self;
        for &i in path {
            node = node.children.get(i)?;
        }
        Some(node)
    }

    /// Inserts a child keeping size order and totals; returns its index.
    pub fn insert_sorted(&mut self, child: Node) -> usize {
        let idx = self.children.partition_point(|c| c.size >= child.size);
        self.size += child.size;
        self.files += child.files;
        self.children.insert(idx, child);
        idx
    }

    /// Removes the node at `path`, subtracting its size from every ancestor.
    pub fn remove(&mut self, path: &[usize]) -> Option<Node> {
        let (&first, rest) = path.split_first()?;
        let removed = if rest.is_empty() {
            (first < self.children.len()).then(|| self.children.remove(first))?
        } else {
            let removed = self.children.get_mut(first)?.remove(rest)?;
            // The child shrank: move it right until size order holds again.
            let mut i = first;
            while i + 1 < self.children.len() && self.children[i].size < self.children[i + 1].size
            {
                self.children.swap(i, i + 1);
                i += 1;
            }
            removed
        };
        self.size -= removed.size;
        self.files -= removed.files;
        Some(removed)
    }

    /// Names of the nodes along an index path (stable across re-sorting).
    pub fn names_along(&self, path: &[usize]) -> Vec<Box<str>> {
        let mut node = self;
        let mut names = Vec::with_capacity(path.len());
        for &i in path {
            let Some(child) = node.children.get(i) else { break };
            names.push(child.name.clone());
            node = child;
        }
        names
    }

    /// Resolves names back to an index path, stopping at the first miss.
    pub fn path_of_names(&self, names: &[Box<str>]) -> Vec<usize> {
        let mut node = self;
        let mut path = Vec::with_capacity(names.len());
        for name in names {
            let Some(i) = node.children.iter().position(|c| c.name == *name) else { break };
            path.push(i);
            node = &node.children[i];
        }
        path
    }

    /// The `limit` largest files at or below this node, largest first, with
    /// their index paths relative to this node.
    pub fn largest_files(&self, limit: usize) -> Vec<(Vec<usize>, &Node)> {
        let mut out: Vec<(Vec<usize>, &Node)> = Vec::new();
        let mut stack: Vec<(Vec<usize>, &Node)> = vec![(Vec::new(), self)];
        while let Some((path, node)) = stack.pop() {
            for (i, child) in node.children.iter().enumerate() {
                // Children are sorted, so once one is smaller than the current
                // cut-off none of its later siblings can qualify either.
                if out.len() >= limit && child.size <= out[limit - 1].1.size {
                    break;
                }
                let mut child_path = path.clone();
                child_path.push(i);
                if child.is_dir {
                    stack.push((child_path, child));
                } else {
                    let pos = out.partition_point(|(_, n)| n.size >= child.size);
                    out.insert(pos, (child_path, child));
                    out.truncate(limit);
                }
            }
        }
        out
    }
}

fn sort_children(children: &mut [Node]) {
    children.sort_unstable_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
}

/// Live counters shared between the scan threads and the UI.
#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub errors: AtomicU64,
    pub cancel: AtomicBool,
    pub current: Mutex<PathBuf>,
    pub error_paths: Mutex<Vec<(PathBuf, String)>>,
}

impl Progress {
    fn record_error(&self, path: &Path, err: &std::io::Error) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut list) = self.error_paths.lock()
            && list.len() < MAX_ERRORS_KEPT
        {
            list.push((path.to_path_buf(), err.to_string()));
        }
    }
}

pub struct ScanOptions {
    /// Descend into other mounted volumes (external drives, network shares).
    pub cross_volumes: bool,
}

/// Messages sent from the scan to the UI as the root's entries complete.
pub enum ScanMsg {
    /// A finished direct child of the scan root.
    Entry(Node),
    Done,
}

struct Ctx<'a> {
    progress: &'a Progress,
    allowed_devs: Vec<u64>,
    cross_volumes: bool,
    seen_inodes: Mutex<HashSet<(u64, u64)>>,
}

impl Ctx<'_> {
    /// Returns true the first time a (device, inode) pair is seen.
    fn first_visit(&self, md: &fs::Metadata) -> bool {
        self.seen_inodes.lock().map(|mut s| s.insert((md.dev(), md.ino()))).unwrap_or(true)
    }

    fn device_allowed(&self, dev: u64) -> bool {
        self.cross_volumes || self.allowed_devs.contains(&dev)
    }
}

/// Scans `root`, streaming each direct child to `tx` as soon as it is
/// complete so the UI can draw the tree while the scan is still running.
pub fn scan(root: &Path, options: ScanOptions, progress: &Progress, tx: &Sender<ScanMsg>) {
    let mut allowed_devs = Vec::new();
    if let Ok(md) = fs::metadata(root) {
        allowed_devs.push(md.dev());
    }
    // On macOS the boot disk is a read-only system volume plus a data volume
    // joined by firmlinks (/Users, /Applications, ...). Treat both as the
    // same disk when scanning from the top.
    if root == Path::new("/")
        && let Ok(md) = fs::metadata("/System/Volumes/Data")
    {
        allowed_devs.push(md.dev());
    }
    let ctx = Ctx {
        progress,
        allowed_devs,
        cross_volumes: options.cross_volumes,
        seen_inodes: Mutex::new(HashSet::new()),
    };
    if let Ok(md) = fs::symlink_metadata(root) {
        ctx.first_visit(&md);
    }

    let entries = match read_entries(root, &ctx) {
        Some(e) => e,
        None => {
            let _ = tx.send(ScanMsg::Done);
            return;
        }
    };
    let (files, dirs) = entries;
    for node in files {
        let _ = tx.send(ScanMsg::Entry(node));
    }
    progress.dirs.fetch_add(1, Ordering::Relaxed);
    dirs.into_par_iter().for_each_with(tx.clone(), |tx, (path, name)| {
        if let Some(node) = scan_dir(&path, name, &ctx) {
            let _ = tx.send(ScanMsg::Entry(node));
        }
    });
    let _ = tx.send(ScanMsg::Done);
}

/// Synchronous helper that returns the complete tree (used by tests).
#[cfg(test)]
pub fn scan_to_tree(root: &Path, options: ScanOptions) -> Node {
    let (tx, rx) = std::sync::mpsc::channel();
    let progress = Progress::default();
    scan(root, options, &progress, &tx);
    drop(tx);
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let children = rx
        .into_iter()
        .filter_map(|m| match m {
            ScanMsg::Entry(n) => Some(n),
            ScanMsg::Done => None,
        })
        .collect();
    Node::dir(name, children)
}

type Entries = (Vec<Node>, Vec<(PathBuf, String)>);

/// Reads one directory: returns its files as finished nodes and its
/// subdirectories as paths still to be scanned.
fn read_entries(path: &Path, ctx: &Ctx) -> Option<Entries> {
    if ctx.progress.cancel.load(Ordering::Relaxed) {
        return None;
    }
    if let Ok(mut current) = ctx.progress.current.try_lock() {
        current.clear();
        current.push(path);
    }
    let read = match fs::read_dir(path) {
        Ok(r) => r,
        Err(e) => {
            ctx.progress.record_error(path, &e);
            return None;
        }
    };
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for entry in read {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                ctx.progress.record_error(path, &e);
                continue;
            }
        };
        let child_path = entry.path();
        // symlink_metadata: never follow symlinks.
        let md = match fs::symlink_metadata(&child_path) {
            Ok(m) => m,
            Err(e) => {
                ctx.progress.record_error(&child_path, &e);
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if md.is_dir() {
            if ctx.device_allowed(md.dev()) && ctx.first_visit(&md) {
                dirs.push((child_path, name));
            }
            continue;
        }
        if md.nlink() > 1 && !md.is_symlink() && !ctx.first_visit(&md) {
            continue;
        }
        let size = md.blocks() * 512;
        ctx.progress.files.fetch_add(1, Ordering::Relaxed);
        ctx.progress.bytes.fetch_add(size, Ordering::Relaxed);
        files.push(Node::file(name, size));
    }
    Some((files, dirs))
}

fn scan_dir(path: &Path, name: String, ctx: &Ctx) -> Option<Node> {
    let Some((mut children, dirs)) = read_entries(path, ctx) else {
        // Unreadable or cancelled: still show it, as an empty folder.
        return Some(Node::dir(name, Vec::new()));
    };
    ctx.progress.dirs.fetch_add(1, Ordering::Relaxed);
    let sub: Vec<Node> =
        dirs.into_par_iter().filter_map(|(p, n)| scan_dir(&p, n, ctx)).collect();
    children.extend(sub);
    Some(Node::dir(name, children))
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    // Decimal units, matching Finder.
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shi-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(path: &Path, len: usize) {
        let mut f = fs::File::create(path).unwrap();
        f.write_all(&vec![7u8; len]).unwrap();
    }

    #[test]
    fn scans_sizes_and_sorts() {
        let dir = temp_dir("sizes");
        fs::create_dir_all(dir.join("big/inner")).unwrap();
        fs::create_dir_all(dir.join("small")).unwrap();
        write_file(&dir.join("big/inner/a.mov"), 400_000);
        write_file(&dir.join("big/b.zip"), 100_000);
        write_file(&dir.join("small/c.txt"), 10);
        write_file(&dir.join("top.bin"), 50_000);

        let tree = scan_to_tree(&dir, ScanOptions { cross_volumes: false });
        assert_eq!(tree.files, 4);
        assert_eq!(&*tree.children[0].name, "big");
        assert!(tree.children[0].size >= 500_000);
        assert_eq!(tree.size, tree.children.iter().map(|c| c.size).sum::<u64>());
        let inner = &tree.children[0].children[0];
        assert_eq!(&*inner.name, "inner");
        assert_eq!(inner.children[0].kind, Kind::Video);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hard_links_count_once_and_symlinks_are_not_followed() {
        let dir = temp_dir("links");
        write_file(&dir.join("orig.dat"), 200_000);
        fs::hard_link(dir.join("orig.dat"), dir.join("link.dat")).unwrap();
        fs::create_dir_all(dir.join("sub")).unwrap();
        write_file(&dir.join("sub/x.bin"), 300_000);
        std::os::unix::fs::symlink(dir.join("sub"), dir.join("sublink")).unwrap();

        let tree = scan_to_tree(&dir, ScanOptions { cross_volumes: false });
        let data_files = tree.children.iter().filter(|c| c.name.ends_with(".dat")).count();
        assert_eq!(data_files, 1, "hard link counted twice");
        let sub = tree.children.iter().find(|c| &*c.name == "sub").unwrap();
        let link = tree.children.iter().find(|c| &*c.name == "sublink").unwrap();
        assert!(!link.is_dir);
        assert!(link.size < sub.size);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn remove_updates_ancestors() {
        let mut root = Node::dir(
            "root",
            vec![
                Node::dir("a", vec![Node::file("x", 100), Node::file("y", 50)]),
                Node::file("z", 30),
            ],
        );
        let removed = root.remove(&[0, 0]).unwrap();
        assert_eq!(&*removed.name, "x");
        assert_eq!(root.size, 80);
        assert_eq!(root.files, 2);
        assert_eq!(root.children[0].size, 50);
        assert!(root.remove(&[5]).is_none());
    }

    #[test]
    fn remove_restores_sibling_order() {
        let mut root = Node::dir(
            "root",
            vec![Node::dir("a", vec![Node::file("x", 100), Node::file("y", 5)]), Node::file("z", 30)],
        );
        let names = root.names_along(&[0, 1]);
        root.remove(&[0, 0]).unwrap();
        let order: Vec<&str> = root.children.iter().map(|c| &*c.name).collect();
        assert_eq!(order, vec!["z", "a"]);
        assert_eq!(root.path_of_names(&names), vec![1, 0]);
    }

    #[test]
    fn insert_sorted_keeps_order() {
        let mut root = Node::dir("r", vec![Node::file("a", 100), Node::file("b", 10)]);
        assert_eq!(root.insert_sorted(Node::file("c", 50)), 1);
        assert_eq!(root.size, 160);
        let sizes: Vec<u64> = root.children.iter().map(|c| c.size).collect();
        assert_eq!(sizes, vec![100, 50, 10]);
    }

    #[test]
    fn largest_files_finds_deep_files() {
        let root = Node::dir(
            "r",
            vec![
                Node::dir("a", vec![Node::file("deep", 500), Node::file("s", 1)]),
                Node::file("mid", 200),
                Node::file("tiny", 2),
            ],
        );
        let top = root.largest_files(2);
        let names: Vec<&str> = top.iter().map(|(_, n)| &*n.name).collect();
        assert_eq!(names, vec!["deep", "mid"]);
        assert_eq!(top[0].0, vec![0, 0]);
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1_500), "1.5 KB");
        assert_eq!(format_size(250_000_000_000), "250 GB");
    }

    #[test]
    fn classifies_kinds() {
        assert_eq!(Kind::for_file("Movie.MOV"), Kind::Video);
        assert_eq!(Kind::for_file(".zshrc"), Kind::Other);
        assert_eq!(Kind::for_dir("Safari.app"), Kind::App);
    }
}
