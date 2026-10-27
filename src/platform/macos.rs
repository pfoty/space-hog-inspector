//! macOS: disk usage from `stat`, volumes from `statfs`, Finder and the
//! Trash, Full Disk Access, the native menu bar, and where settings and AI
//! agents live.

use std::ffi::OsString;
use std::fs::{self, Metadata};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};

use eframe::egui;
use muda::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};

use super::{FileFacts, VolumeInfo};

/// The system's name, for messages.
pub const OS_NAME: &str = "macOS";

/// macOS asks for permission folder by folder unless the app has Full Disk
/// Access, so the app offers that setup and can skip prompting folders.
pub const PRIVACY_PROMPTS: bool = true;

// ----------------------------------------------------------- folders --

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Where this app keeps its settings.
pub fn app_data_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join("Library/Application Support/Space Hog Inspector"))
}

/// Where this app keeps files it can recreate.
pub fn app_cache_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join("Library/Caches/Space Hog Inspector"))
}

/// The top of the startup disk.
pub fn system_root() -> PathBuf {
    PathBuf::from("/")
}

// -------------------------------------------------------- disk usage --

/// Size on disk (allocated blocks, not length), link count, and identity
/// as (device, inode), all from `lstat`.
pub fn file_facts(_path: &Path, md: &Metadata) -> FileFacts {
    FileFacts { size: md.blocks() * 512, links: md.nlink(), id: Some((md.dev(), md.ino())), volume: Some(md.dev()) }
}

/// Volumes other than `root`'s own that count as the same disk.
///
/// The boot disk is a read-only system volume plus a data volume joined by
/// firmlinks (/Users, /Applications, ...), so both count when scanning from
/// the top.
pub fn same_disk_volumes(root: &Path) -> Vec<u64> {
    if root == Path::new("/")
        && let Ok(md) = fs::metadata("/System/Volumes/Data")
    {
        vec![md.dev()]
    } else {
        Vec::new()
    }
}

// ----------------------------------------------------------- volumes --

/// Space, mount point and file system of the volume holding `path`.
pub fn volume_info(path: &Path) -> Option<VolumeInfo> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: c_path is a valid NUL-terminated string and stat is a valid out-pointer.
    if unsafe { libc::statfs(c_path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    // SAFETY: statfs fills these as NUL-terminated C strings.
    let mount = unsafe { std::ffi::CStr::from_ptr(stat.f_mntonname.as_ptr()) }.to_string_lossy().into_owned();
    let fs_type = unsafe { std::ffi::CStr::from_ptr(stat.f_fstypename.as_ptr()) }.to_string_lossy().into_owned();
    let block = stat.f_bsize as u64;
    Some(VolumeInfo { free: stat.f_bavail * block, total: stat.f_blocks * block, mount: PathBuf::from(mount), fs_type })
}

/// The startup disk's name as Finder shows it, from the /Volumes entry
/// that points back at `/`.
pub fn startup_volume_name() -> String {
    fs::read_dir("/Volumes")
        .into_iter()
        .flatten()
        .flatten()
        .find(|e| fs::read_link(e.path()).is_ok_and(|t| t == Path::new("/")))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .unwrap_or_else(|| "Macintosh HD".to_string())
}

pub fn volume_display_name(mount: &Path, startup_name: &str) -> String {
    if mount == Path::new("/") || mount == Path::new("/System/Volumes/Data") {
        startup_name.to_string()
    } else {
        mount.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| mount.display().to_string())
    }
}

// ------------------------------------------------- Finder and Trash --

/// Shows the item selected in a Finder window.
pub fn reveal(path: &Path) -> std::io::Result<()> {
    Command::new("open").arg("-R").arg(path).status().map(|_| ())
}

/// Opens the item with its default app.
pub fn open(path: &Path) -> std::io::Result<()> {
    Command::new("open").arg(path).status().map(|_| ())
}

pub fn move_to_trash(path: &Path) -> Result<(), String> {
    // The crate's default asks Finder via AppleScript, which blocks on an
    // Automation permission prompt; NSFileManager needs no permission.
    use trash::macos::{DeleteMethod, TrashContextExtMacos};
    let mut trash_ctx = trash::TrashContext::default();
    trash_ctx.set_delete_method(DeleteMethod::NsFileManager);
    trash_ctx.delete(path).map_err(|e| e.to_string())
}

// -------------------------------------------------- Full Disk Access --

pub const FULL_DISK_ACCESS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";

/// Whether this app currently has Full Disk Access.
///
/// macOS has no API to ask directly, so this tries to read a file that only
/// apps with Full Disk Access may open. Reading it never shows a prompt.
pub fn has_full_disk_access() -> bool {
    let Some(home) = home_dir() else { return false };
    let probes = [
        home.join("Library/Application Support/com.apple.TCC/TCC.db"),
        home.join("Library/Safari/Bookmarks.plist"),
        PathBuf::from("/Library/Application Support/com.apple.TCC/TCC.db"),
    ];
    probes.iter().any(|p| fs::File::open(p).is_ok())
}

/// The `.app` bundle this program runs from, if it runs from one.
fn app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // .../Name.app/Contents/MacOS/binary
    let bundle = exe.parent()?.parent()?.parent()?;
    (bundle.extension()? == "app").then(|| bundle.to_path_buf())
}

pub fn open_full_disk_access_settings() -> std::io::Result<()> {
    Command::new("open").arg(FULL_DISK_ACCESS_URL).status().map(|_| ())
}

/// Shows the app bundle (or the binary, when run with `cargo run`) in Finder
/// so it can be dragged into the Full Disk Access list.
pub fn reveal_app_in_finder() -> std::io::Result<()> {
    let target = app_bundle().or_else(|| std::env::current_exe().ok()).unwrap_or_default();
    Command::new("open").arg("-R").arg(target).status().map(|_| ())
}

/// Starts a fresh copy of the app. The caller should then close this one;
/// macOS only applies a new Full Disk Access grant to a new process.
pub fn launch_new_instance() -> std::io::Result<()> {
    match app_bundle() {
        Some(bundle) => Command::new("open").arg("-n").arg(bundle).spawn().map(|_| ()),
        None => Command::new(std::env::current_exe()?).spawn().map(|_| ()),
    }
}

// --------------------------------------------------- window and menu --

/// How a Command-key shortcut is written here, e.g. "⌘O".
pub fn shortcut(key: &str) -> String {
    format!("⌘{key}")
}

/// Lets a console program started from this app open normally (macOS
/// never gives it a window).
pub fn hide_console(_cmd: &mut Command) {}

/// The icon eframe sets on the window at launch. Empty, so eframe doesn't
/// replace the bundle's AppIcon.icns with its own default icon.
pub fn window_icon() -> egui::IconData {
    egui::IconData::default()
}

/// Keeps the native menu bar installed while it's alive.
pub struct MenuBar {
    _menu: Menu,
}

/// Replaces winit's default menu bar with the app's own. Call once the app
/// is running. Returns the ids of chosen menu items as they arrive.
pub fn install_menu_bar(settings_id: &str, repaint: egui::Context) -> (MenuBar, Receiver<String>) {
    let menu = build_menu_bar(settings_id);
    let (tx, rx) = mpsc::channel();
    muda::MenuEvent::set_event_handler(Some(move |event: muda::MenuEvent| {
        let _ = tx.send(event.id.as_ref().to_string());
        repaint.request_repaint();
    }));
    (MenuBar { _menu: menu }, rx)
}

fn build_menu_bar(settings_id: &str) -> Menu {
    let about = AboutMetadata {
        name: Some("Space Hog Inspector".into()),
        version: Some(env!("CARGO_PKG_VERSION").into()),
        ..Default::default()
    };
    let settings = MenuItem::with_id(settings_id, "Settings…", true, "CmdOrCtrl+Comma".parse().ok());
    let app_menu = Submenu::with_items(
        "Space Hog Inspector",
        true,
        &[
            &PredefinedMenuItem::about(None, Some(about)),
            &PredefinedMenuItem::separator(),
            &settings,
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::services(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::hide(None),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::quit(None),
        ],
    )
    .expect("app menu");
    let window_menu = Submenu::with_items(
        "Window",
        true,
        &[&PredefinedMenuItem::minimize(None), &PredefinedMenuItem::fullscreen(None)],
    )
    .expect("window menu");
    let menu = Menu::with_items(&[&app_menu, &window_menu]).expect("menu bar");
    menu.init_for_nsapp();
    window_menu.set_as_windows_menu_for_nsapp();
    menu
}

// --------------------------------------------------------- AI agents --

/// Usual install locations of an agent's command-line tool. An app opened
/// from Finder doesn't get the shell's PATH, so these are checked directly.
pub fn agent_candidates(binary: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = home_dir() {
        out.push(home.join(".local/bin").join(binary));
        if binary == "claude" {
            out.push(home.join(".claude/local/claude"));
        }
    }
    out.push(Path::new("/opt/homebrew/bin").join(binary));
    out.push(Path::new("/usr/local/bin").join(binary));
    out
}

pub fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Asks the user's login shell where `binary` is, for installs outside the
/// usual locations (for example under nvm).
pub fn find_with_shell(binary: &str) -> Option<PathBuf> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let out = Command::new(shell)
        .args(["-lc", &format!("command -v {binary}")])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let line = String::from_utf8_lossy(&out.stdout).lines().last()?.trim().to_string();
    let path = PathBuf::from(line);
    (path.is_absolute() && is_executable(&path)).then_some(path)
}

/// PATH for running an agent: its own folder and the usual tool folders
/// first, then the inherited PATH.
pub fn agent_path(exe: &Path) -> OsString {
    let mut path = exe.parent().map(|p| p.display().to_string()).unwrap_or_default();
    path += ":/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin";
    if let Ok(inherited) = std::env::var("PATH") {
        path = format!("{path}:{inherited}");
    }
    path.into()
}
