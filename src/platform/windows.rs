//! Windows: disk usage from file handles, drives from the volume APIs,
//! Explorer and the Recycle Bin, and where settings and AI agents live.
//!
//! There's no Full Disk Access here and no per-folder permission prompts:
//! folders this account can't read are reported as unreadable instead.

use std::ffi::{OsStr, OsString};
use std::fs::Metadata;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};

use eframe::egui;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, FileStandardInfo,
    GetDiskFreeSpaceExW, GetFileInformationByHandle, GetFileInformationByHandleEx, GetVolumeInformationW,
    GetVolumePathNameW, OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use super::{FileFacts, VolumeInfo};

/// The system's name, for messages.
pub const OS_NAME: &str = "Windows";

/// Windows has no per-folder permission prompts to avoid, so there's no
/// Full Disk Access setup or "skip protected folders" setting.
pub const PRIVACY_PROMPTS: bool = false;

// ----------------------------------------------------------- folders --

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(PathBuf::from)
}

/// Where this app keeps its settings (roaming app data).
pub fn app_data_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Space Hog Inspector"))
}

/// Where this app keeps files it can recreate (local app data).
pub fn app_cache_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Space Hog Inspector"))
}

/// The top of the system drive, usually `C:\`.
pub fn system_root() -> PathBuf {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    PathBuf::from(format!("{drive}\\"))
}

// -------------------------------------------------------- disk usage --

/// What one handle query says about a file.
struct HandleInfo {
    allocated: u64,
    links: u32,
    volume: u32,
    index: u64,
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

/// Opens the item itself (not what a link points to) without reading or
/// changing it, and asks for its size on disk, link count and file id.
/// Opening for attributes only doesn't download cloud-only files.
fn handle_info(path: &Path) -> Option<HandleInfo> {
    let name = wide(path.as_os_str());
    // SAFETY: name is NUL-terminated; the handle is closed below.
    let handle: HANDLE = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    // SAFETY: both structs are plain data, filled by the calls on a valid handle.
    let mut by_handle: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let mut standard: FILE_STANDARD_INFO = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        GetFileInformationByHandle(handle, &mut by_handle) != 0
            && GetFileInformationByHandleEx(
                handle,
                FileStandardInfo,
                (&mut standard as *mut FILE_STANDARD_INFO).cast(),
                std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
            ) != 0
    };
    // SAFETY: handle came from CreateFileW and isn't used afterwards.
    unsafe { CloseHandle(handle) };
    ok.then(|| HandleInfo {
        allocated: standard.AllocationSize.max(0) as u64,
        links: by_handle.nNumberOfLinks,
        volume: by_handle.dwVolumeSerialNumber,
        index: (u64::from(by_handle.nFileIndexHigh) << 32) | u64::from(by_handle.nFileIndexLow),
    })
}

/// Size on disk (cloud-only files take almost none), link count, and
/// identity as (volume serial, file index), from one handle. Falls back to
/// the file's length, unknown identity, when the item can't be opened.
pub fn file_facts(path: &Path, md: &Metadata) -> FileFacts {
    match handle_info(path) {
        Some(i) => FileFacts {
            size: i.allocated,
            links: u64::from(i.links),
            id: Some((u64::from(i.volume), i.index)),
            volume: Some(u64::from(i.volume)),
        },
        None => FileFacts { size: md.len(), links: 1, id: None, volume: None },
    }
}

/// Volumes other than `root`'s own that count as the same disk: none here.
pub fn same_disk_volumes(_root: &Path) -> Vec<u64> {
    Vec::new()
}

// ----------------------------------------------------------- volumes --

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    OsString::from_wide(&buf[..end]).to_string_lossy().into_owned()
}

/// The drive or mount folder holding `path`, e.g. `C:\`.
fn volume_root(path: &Path) -> Option<PathBuf> {
    let name = wide(path.as_os_str());
    let mut buf = [0u16; 1024];
    // SAFETY: name is NUL-terminated and buf's length is passed.
    let ok = unsafe { GetVolumePathNameW(name.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } != 0;
    ok.then(|| PathBuf::from(from_wide(&buf)))
}

/// The volume's label and file system, e.g. ("Data", "NTFS").
fn volume_label(root: &Path) -> Option<(String, String)> {
    let name = wide(root.as_os_str());
    let mut label = [0u16; 261];
    let mut fs_name = [0u16; 261];
    // SAFETY: name is NUL-terminated; buffer lengths are passed; the
    // optional out-pointers are null.
    let ok = unsafe {
        GetVolumeInformationW(
            name.as_ptr(),
            label.as_mut_ptr(),
            label.len() as u32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs_name.as_mut_ptr(),
            fs_name.len() as u32,
        )
    } != 0;
    ok.then(|| (from_wide(&label), from_wide(&fs_name)))
}

/// Free space (for this account), total space, drive and file system of
/// the volume holding `path`.
pub fn volume_info(path: &Path) -> Option<VolumeInfo> {
    let mount = volume_root(path)?;
    let name = wide(mount.as_os_str());
    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: name is NUL-terminated; the last out-pointer is optional.
    if unsafe { GetDiskFreeSpaceExW(name.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) } == 0 {
        return None;
    }
    let fs_type = volume_label(&mount).map(|(_, fs)| fs).unwrap_or_default();
    Some(VolumeInfo { free, total, mount, fs_type })
}

/// The system drive's name as Explorer shows it, e.g. "Local Disk (C:)".
pub fn startup_volume_name() -> String {
    drive_name(&system_root())
}

/// "Label (X:)", or "Local Disk (X:)" when the drive has no label.
fn drive_name(mount: &Path) -> String {
    let letter = mount.display().to_string().trim_end_matches('\\').to_string();
    let label = volume_label(mount).map(|(l, _)| l).filter(|l| !l.is_empty());
    format!("{} ({letter})", label.as_deref().unwrap_or("Local Disk"))
}

pub fn volume_display_name(mount: &Path, startup_name: &str) -> String {
    if mount == system_root() { startup_name.to_string() } else { drive_name(mount) }
}

// ------------------------------------------ Explorer and Recycle Bin --

/// Shows the item selected in an Explorer window.
pub fn reveal(path: &Path) -> std::io::Result<()> {
    // Explorer parses its own command line, so pass the switch as written.
    let mut arg = OsString::from("/select,\"");
    arg.push(path.as_os_str());
    arg.push("\"");
    Command::new("explorer.exe").raw_arg(arg).spawn().map(|_| ())
}

/// Opens the item with its default app.
pub fn open(path: &Path) -> std::io::Result<()> {
    let file = wide(path.as_os_str());
    let verb = wide(OsStr::new("open"));
    // SAFETY: both strings are NUL-terminated; the other arguments are optional.
    let result = unsafe {
        ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL)
    };
    // Values above 32 mean success.
    if result as isize > 32 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

pub fn move_to_trash(path: &Path) -> Result<(), String> {
    trash::delete(path).map_err(|e| e.to_string())
}

// ---------------------------------------- Full Disk Access (macOS only) --

/// Every folder this account may read is readable without prompts.
pub fn has_full_disk_access() -> bool {
    true
}

fn unsupported() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Unsupported, "not available on Windows")
}

pub fn open_full_disk_access_settings() -> std::io::Result<()> {
    Err(unsupported())
}

pub fn reveal_app_in_finder() -> std::io::Result<()> {
    Err(unsupported())
}

/// Starts a fresh copy of the app.
pub fn launch_new_instance() -> std::io::Result<()> {
    Command::new(std::env::current_exe()?).spawn().map(|_| ())
}

// --------------------------------------------------- window and menu --

/// How a Ctrl-key shortcut is written here, e.g. "Ctrl+O".
pub fn shortcut(key: &str) -> String {
    format!("Ctrl+{key}")
}

/// The window and taskbar icon.
pub fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../../assets/icon-256.png")).unwrap_or_default()
}

/// No native menu bar on Windows: it would be a plain Win32 bar that
/// clashes with the app's look. Settings stays on the gear and Ctrl+,.
pub struct MenuBar;

pub fn install_menu_bar(_settings_id: &str, _repaint: egui::Context) -> (MenuBar, Receiver<String>) {
    let (_tx, rx) = mpsc::channel();
    (MenuBar, rx)
}

// --------------------------------------------------------- AI agents --

/// Usual install locations of an agent's command-line tool: the native
/// installer's folder, then npm's global folder.
pub fn agent_candidates(binary: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = home_dir() {
        out.push(home.join(".local\\bin").join(format!("{binary}.exe")));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let npm = PathBuf::from(appdata).join("npm");
        out.push(npm.join(format!("{binary}.exe")));
        out.push(npm.join(format!("{binary}.cmd")));
    }
    out
}

pub fn is_executable(path: &Path) -> bool {
    let runnable = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["exe", "cmd", "bat", "com"].contains(&e.to_ascii_lowercase().as_str()));
    runnable && path.is_file()
}

/// Asks Windows where `binary` is on the PATH, preferring a real `.exe`:
/// Rust can't pass every argument safely to a `.cmd` script.
pub fn find_with_shell(binary: &str) -> Option<PathBuf> {
    let mut cmd = Command::new("where.exe");
    cmd.arg(binary).stdin(Stdio::null()).stderr(Stdio::null());
    hide_console(&mut cmd);
    let out = cmd.output().ok()?;
    let found: Vec<PathBuf> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| PathBuf::from(l.trim()))
        .filter(|p| is_executable(p))
        .collect();
    let is_exe = |p: &&PathBuf| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    found.iter().find(is_exe).or(found.first()).cloned()
}

/// PATH for running an agent: its own folder first, then the inherited PATH.
pub fn agent_path(exe: &Path) -> OsString {
    let mut dirs: Vec<PathBuf> = exe.parent().map(Path::to_path_buf).into_iter().collect();
    if let Some(inherited) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&inherited));
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

/// Stops a console program started from this window app from opening a
/// console window of its own.
pub fn hide_console(cmd: &mut Command) {
    cmd.creation_flags(CREATE_NO_WINDOW);
}
