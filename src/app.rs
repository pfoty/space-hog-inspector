//! The Space Hog Inspector window: header, treemap, side panel, status bar,
//! start screen and dialogs, drawn in the Neo-Noir Cyberpunk style.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, Frame, Id, Key, LayerId, Layout, Margin, Order, Painter, Pos2, Rect,
    RichText, Role, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, WidgetInfo, pos2, vec2,
};

use crate::ai::{self, Agent, Assessment, Facts, Provider, Verdict};
use crate::scanner::{self, Kind, Node, ProtectedPaths, Progress, ScanMsg, ScanOptions, format_size};
use crate::settings::{self, Settings};
use crate::theme::{
    self, BG, BORDER, Button, CYAN, CYAN_DEEP, DIM, GREEN, GREY, Icon, MAGENTA, MAGENTA_DEEP, MAGENTA_SOFT,
    MUTED, PANEL, RAISED, SURFACE, TEXT, TRACK, Tone, YELLOW, YELLOW_DEEP, body, body_medium, body_semibold,
    display, display_bold, mono, mono_medium,
};
use crate::treemap::squarify;

const ROW_H: f32 = 40.0;
/// Smallest area, in square points, an item gets its own rectangle.
const MIN_ITEM_AREA: f32 = 12.0;
/// Menu-bar item id for Settings…
pub const MENU_SETTINGS: &str = "settings";
/// Lines kept in the scan telemetry log.
const LOG_LINES: usize = 5;

#[derive(PartialEq, Eq, Clone, Copy)]
enum SideTab {
    Contents,
    LargestFiles,
    Ai,
}

/// UI events gathered while drawing and applied afterwards, so drawing
/// code can borrow the tree immutably.
enum Action {
    Select(Vec<usize>),
    ZoomTo(Vec<usize>),
    Up,
    Reveal(Vec<usize>),
    Open(Vec<usize>),
    CopyPath(Vec<usize>),
    AskTrash(Vec<usize>),
    /// Ask the AI agent about an item; `true` skips the cached answer.
    AskAi(Vec<usize>, bool),
    CancelAi,
    OpenSettings,
}

struct ActiveScan {
    rx: Receiver<ScanMsg>,
    progress: Arc<Progress>,
    started: Instant,
    /// Recently read folders, newest last.
    log: VecDeque<String>,
}

struct TrashRequest {
    path: Vec<usize>,
    fs_path: PathBuf,
    name: String,
    size: u64,
    is_dir: bool,
    kind: Kind,
    files: u64,
}

/// The item an AI question is about. Names, not indices, so it survives
/// the tree being re-sorted.
#[derive(Clone)]
struct AiSubject {
    names: Vec<Box<str>>,
    fs_path: PathBuf,
    facts: Facts,
    provider: Provider,
}

struct AiJob {
    subject: AiSubject,
    rx: Receiver<Result<Assessment, String>>,
    cancel: Arc<AtomicBool>,
    started: Instant,
}

struct AiAnswer {
    subject: AiSubject,
    result: Result<Assessment, String>,
}

struct LargestCache {
    view: Vec<usize>,
    generation: u64,
    /// (absolute index path, name, size, parent folder relative to view)
    rows: Vec<(Vec<usize>, Box<str>, u64, String)>,
}

/// How the path bar names the scan root: the folders above it, which
/// aren't part of the scan, and the root's own label.
struct RootCrumbs {
    above: Vec<String>,
    label: String,
}

pub struct SpaceHogApp {
    root_path: Option<PathBuf>,
    root_crumbs: RootCrumbs,
    tree: Option<Node>,
    /// Index path from the tree root to the folder filling the treemap.
    view: Vec<usize>,
    selected: Option<Vec<usize>>,
    scan: Option<ActiveScan>,
    /// Bumped whenever the tree changes, to invalidate caches.
    generation: u64,
    settings: Settings,
    /// Last settings written to disk, to save only on change.
    saved_settings: Settings,
    show_settings: bool,
    /// Whether this app has Full Disk Access (checked at start, before each
    /// scan, and on demand from Settings).
    full_disk_access: bool,
    /// Name of the startup disk, e.g. "Macintosh HD".
    startup_name: String,
    /// Kept alive so the menu bar stays installed.
    _menu: muda::Menu,
    menu_events: Receiver<muda::MenuEvent>,
    filter: String,
    side_tab: SideTab,
    largest: Option<LargestCache>,
    trash_request: Option<TrashRequest>,
    message: Option<(String, bool)>,
    last_scan: Option<(Duration, bool)>,
    error_count: u64,
    error_paths: Vec<(PathBuf, String)>,
    show_errors: bool,
    /// Installed AI agents (found at start and on "Detect again").
    ai_agents: Vec<Agent>,
    ai_job: Option<AiJob>,
    ai_answer: Option<AiAnswer>,
    /// Answers by (path, size) for this session, so asking again is instant
    /// until the item changes.
    ai_cache: HashMap<(PathBuf, u64), (Provider, Assessment)>,
}

impl SpaceHogApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        initial: Option<PathBuf>,
        menu: muda::Menu,
        menu_events: Receiver<muda::MenuEvent>,
    ) -> Self {
        theme::install(&cc.egui_ctx);
        let settings = Settings::load();
        let startup_name = startup_volume_name();
        let mut app = SpaceHogApp {
            root_path: None,
            root_crumbs: RootCrumbs { above: Vec::new(), label: String::new() },
            tree: None,
            view: Vec::new(),
            selected: None,
            scan: None,
            generation: 0,
            saved_settings: settings.clone(),
            settings,
            show_settings: false,
            full_disk_access: settings::has_full_disk_access(),
            startup_name,
            _menu: menu,
            menu_events,
            filter: String::new(),
            side_tab: SideTab::Contents,
            largest: None,
            trash_request: None,
            message: None,
            last_scan: None,
            error_count: 0,
            error_paths: Vec::new(),
            show_errors: false,
            ai_agents: ai::detect(),
            ai_job: None,
            ai_answer: None,
            ai_cache: HashMap::new(),
        };
        if let Some(path) = initial {
            app.start_scan(path, &cc.egui_ctx);
        }
        app
    }

    fn start_scan(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.stop_scan();
        let path = path.canonicalize().unwrap_or(path);
        let progress = Arc::new(Progress::default());
        let (tx, rx) = mpsc::channel();
        self.full_disk_access = settings::has_full_disk_access();
        let skip = (self.settings.skip_protected && !self.full_disk_access)
            .then(|| std::env::var_os("HOME").map(|h| ProtectedPaths::for_home(Path::new(&h))))
            .flatten();
        let options = ScanOptions { cross_volumes: self.settings.cross_volumes, skip };
        let thread_progress = progress.clone();
        let thread_path = path.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            scanner::scan(&thread_path, options, &thread_progress, &tx);
            repaint.request_repaint();
        });
        self.root_crumbs = root_crumbs(&path, &self.startup_name);
        self.tree = Some(Node::dir(path.display().to_string(), Vec::new()));
        self.root_path = Some(path);
        self.view.clear();
        self.selected = None;
        self.largest = None;
        self.message = None;
        self.last_scan = None;
        self.error_count = 0;
        self.error_paths.clear();
        self.generation += 1;
        self.scan = Some(ActiveScan { rx, progress, started: Instant::now(), log: VecDeque::new() });
    }

    fn stop_scan(&mut self) {
        if let Some(scan) = &self.scan {
            scan.progress.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Pulls finished entries from the scan thread into the tree.
    fn poll_scan(&mut self, ctx: &egui::Context) {
        let Some(scan) = &mut self.scan else { return };
        if let Ok(current) = scan.progress.current.lock() {
            let line = tilde(&current);
            if !current.as_os_str().is_empty() && scan.log.back() != Some(&line) {
                scan.log.push_back(line);
                if scan.log.len() > LOG_LINES {
                    scan.log.pop_front();
                }
            }
        }
        let mut done = false;
        let deadline = Instant::now() + Duration::from_millis(12);
        while Instant::now() < deadline {
            match scan.rx.try_recv() {
                Ok(ScanMsg::Entry(node)) => {
                    if let Some(tree) = &mut self.tree {
                        let idx = tree.insert_sorted(node);
                        // Keep paths into the root pointing at the same items.
                        for path in [Some(&mut self.view), self.selected.as_mut()].into_iter().flatten() {
                            if let Some(first) = path.first_mut()
                                && *first >= idx
                            {
                                *first += 1;
                            }
                        }
                        self.generation += 1;
                    }
                }
                Ok(ScanMsg::Done) | Err(mpsc::TryRecvError::Disconnected) => {
                    done = true;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        if done {
            let cancelled = scan.progress.cancel.load(Ordering::Relaxed);
            self.last_scan = Some((scan.started.elapsed(), cancelled));
            self.error_count = scan.progress.errors.load(Ordering::Relaxed);
            self.error_paths = std::mem::take(&mut *scan.progress.error_paths.lock().unwrap());
            self.scan = None;
            self.largest = None;
        } else {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn scanning(&self) -> bool {
        self.scan.is_some()
    }

    fn fs_path(&self, path: &[usize]) -> Option<PathBuf> {
        let tree = self.tree.as_ref()?;
        let mut out = self.root_path.clone()?;
        for name in tree.names_along(path) {
            out.push(&*name);
        }
        Some(out)
    }

    fn choose_folder(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new().set_title("Choose a folder or disk to scan");
        if let Some(dir) = &self.root_path {
            dialog = dialog.set_directory(dir);
        }
        if let Some(dir) = dialog.pick_folder() {
            self.start_scan(dir, ctx);
        }
    }

    fn apply(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::Select(path) => self.selected = Some(path),
            Action::ZoomTo(path) => {
                let is_dir = self.tree.as_ref().and_then(|t| t.at(&path)).is_some_and(|n| n.is_dir);
                self.view = if is_dir { path.clone() } else { path[..path.len().saturating_sub(1)].to_vec() };
                self.selected = Some(path);
            }
            Action::Up => {
                if let Some(child) = self.view.pop() {
                    let mut sel = self.view.clone();
                    sel.push(child);
                    self.selected = Some(sel);
                }
            }
            Action::Reveal(path) => {
                if let Some(p) = self.fs_path(&path) {
                    self.run_open(Command::new("open").arg("-R").arg(p));
                }
            }
            Action::Open(path) => {
                if let Some(p) = self.fs_path(&path) {
                    self.run_open(Command::new("open").arg(p));
                }
            }
            Action::CopyPath(path) => {
                if let Some(p) = self.fs_path(&path) {
                    ctx.copy_text(p.display().to_string());
                    self.message = Some((format!("Copied {}", p.display()), false));
                }
            }
            Action::AskTrash(path) => {
                let node = self.tree.as_ref().and_then(|t| t.at(&path));
                if let (Some(node), Some(fs_path)) = (node, self.fs_path(&path))
                    && !path.is_empty()
                {
                    self.trash_request = Some(TrashRequest {
                        name: node.name.to_string(),
                        size: node.size,
                        is_dir: node.is_dir,
                        kind: node.kind,
                        files: node.files,
                        fs_path,
                        path,
                    });
                }
            }
            Action::AskAi(path, fresh) => self.ask_ai(&path, fresh, ctx),
            Action::CancelAi => {
                if let Some(job) = self.ai_job.take() {
                    job.cancel.store(true, Ordering::Relaxed);
                }
            }
            Action::OpenSettings => self.open_settings(),
        }
    }

    fn ai_agent(&self) -> Option<&Agent> {
        let provider = self.settings.ai_provider?;
        self.ai_agents.iter().find(|a| a.provider == provider)
    }

    /// Starts asking the chosen agent about the item at `path`, or shows the
    /// cached answer.
    fn ask_ai(&mut self, path: &[usize], fresh: bool, ctx: &egui::Context) {
        let Some(provider) = self.settings.ai_provider else {
            self.message = Some(("Choose an AI agent in Settings first.".into(), true));
            return;
        };
        let Some(agent) = self.ai_agent().cloned() else {
            self.message = Some((format!("{} isn't installed on this Mac.", provider.label()), true));
            return;
        };
        let (Some(tree), Some(fs_path)) = (self.tree.as_ref(), self.fs_path(path)) else { return };
        let Some(node) = tree.at(path) else { return };
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let subject =
            AiSubject { names: tree.names_along(path), facts: Facts::gather(node, &fs_path, home.as_deref()), fs_path, provider };
        self.side_tab = SideTab::Ai;
        let key = (subject.fs_path.clone(), subject.facts.size);
        if !fresh && let Some((cached_by, answer)) = self.ai_cache.get(&key).cloned() {
            self.ai_answer = Some(AiAnswer { subject: AiSubject { provider: cached_by, ..subject }, result: Ok(answer) });
            return;
        }
        if let Some(job) = self.ai_job.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = cancel.clone();
        let facts = subject.facts.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(ai::ask(&agent, &facts, &thread_cancel));
            repaint.request_repaint();
        });
        self.ai_answer = None;
        self.ai_job = Some(AiJob { subject, rx, cancel, started: Instant::now() });
    }

    /// Picks up the agent's answer when it arrives.
    fn poll_ai(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.ai_job else { return };
        let result = match job.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Disconnected) => Err(format!("{} stopped unexpectedly.", job.subject.provider.label())),
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(200));
                return;
            }
        };
        let job = self.ai_job.take().unwrap();
        if let Ok(answer) = &result {
            let key = (job.subject.fs_path.clone(), job.subject.facts.size);
            self.ai_cache.insert(key, (job.subject.provider, answer.clone()));
        }
        self.ai_answer = Some(AiAnswer { subject: job.subject, result });
    }

    /// Where the AI answer's item is in the current tree, if it's still there.
    fn ai_subject_path(&self, subject: &AiSubject) -> Option<Vec<usize>> {
        let path = self.tree.as_ref()?.path_of_names(&subject.names);
        (path.len() == subject.names.len() && self.fs_path(&path).as_ref() == Some(&subject.fs_path)).then_some(path)
    }

    fn run_open(&mut self, cmd: &mut Command) {
        if let Err(e) = cmd.status() {
            self.message = Some((format!("Couldn't run open: {e}"), true));
        }
    }

    fn move_to_trash(&mut self, request: TrashRequest) {
        // The crate's default asks Finder via AppleScript, which blocks on an
        // Automation permission prompt; NSFileManager needs no permission.
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        let mut trash_ctx = trash::TrashContext::default();
        trash_ctx.set_delete_method(DeleteMethod::NsFileManager);
        if let Err(e) = trash_ctx.delete(&request.fs_path) {
            self.message = Some((format!("Couldn't move {} to the Trash: {e}", request.name), true));
            return;
        }
        if let Some(tree) = &mut self.tree {
            let view_names = tree.names_along(&self.view);
            tree.remove(&request.path);
            self.view = tree.path_of_names(&view_names);
        }
        self.selected = None;
        self.generation += 1;
        self.ai_cache.remove(&(request.fs_path.clone(), request.size));
        if self.ai_answer.as_ref().is_some_and(|a| a.subject.fs_path.starts_with(&request.fs_path)) {
            self.ai_answer = None;
        }
        self.message = Some((
            format!(
                "Moved {} to the Trash. Its {} is freed when you empty the Trash.",
                request.name,
                format_size(request.size)
            ),
            false,
        ));
    }

    // ------------------------------------------------------------ header --

    fn header(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        theme::framed(ui, SURFACE, Stroke::new(1.0, BORDER), 18.0, None, Margin::symmetric(16, 10), |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let (logo, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                theme::icon(ui.painter(), logo, Icon::Logo, CYAN);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new("SPACE//HOG").font(display_bold(17.0)).color(TEXT).extra_letter_spacing(1.4));
                    ui.label(RichText::new("INSPECTOR v1").font(mono(10.0)).color(MUTED).extra_letter_spacing(2.0));
                });
                let (rule, _) = ui.allocate_exact_size(vec2(1.0, 40.0), Sense::hover());
                ui.painter().rect_filled(rule, 0.0, BORDER);

                if let Some(scan) = &self.scan {
                    let stop = Button::new("Stop scan", Tone::Outline(MAGENTA)).icon(Icon::Stop).shortcut("Esc").show(ui);
                    let (total, done) = (
                        scan.progress.top_total.load(Ordering::Relaxed),
                        scan.progress.top_done.load(Ordering::Relaxed),
                    );
                    if stop.clicked() {
                        self.stop_scan();
                    }
                    theme::caption(ui, "NODES", MUTED);
                    let count = format!("{done}/{total}");
                    let width = (ui.available_width() - 120.0).max(80.0);
                    let (bar, _) = ui.allocate_exact_size(vec2(width, 12.0), Sense::hover());
                    node_bar(ui.painter(), bar, done, total);
                    ui.label(RichText::new(count).font(mono(13.0)).color(CYAN));
                } else {
                    if Button::new("Choose folder", Tone::Primary(MAGENTA))
                        .icon(Icon::Folder)
                        .shortcut("⌘O")
                        .show(ui)
                        .clicked()
                    {
                        self.choose_folder(&ctx);
                    }
                    let rescan = Button::new("Rescan", Tone::Outline(CYAN))
                        .icon(Icon::Rescan)
                        .shortcut("⌘R")
                        .enabled(self.root_path.is_some())
                        .show(ui);
                    if rescan.clicked()
                        && let Some(p) = self.root_path.clone()
                    {
                        self.start_scan(p, &ctx);
                    }
                    theme::framed(ui, BG, Stroke::new(1.0, BORDER), 0.0, None, Margin::symmetric(14, 6), |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            theme::caption(ui, "DEPTH", MUTED);
                            ui.add(egui::Slider::new(&mut self.settings.max_depth, 1..=12).show_value(false))
                                .on_hover_text("How many nested levels the treemap draws");
                            ui.label(RichText::new(format!("{:02}", self.settings.max_depth)).font(mono(13.0)).color(CYAN));
                        });
                    });
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::icon_button(ui, Icon::Gear, 40.0, "Settings (⌘,)").clicked() {
                        self.open_settings();
                    }
                });
            });
        });
    }

    fn path_bar(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let Some(tree) = &self.tree else { return };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let up = Button::new("Up", Tone::Ghost).icon(Icon::Up).height(36.0).enabled(!self.view.is_empty());
            if up.show(ui).on_hover_text("Backspace or ⌘↑").clicked() {
                actions.push(Action::Up);
            }
            ui.label(RichText::new(">").font(mono(13.0)).color(MUTED));
            // Long prefixes keep their first and last two parts.
            let above = &self.root_crumbs.above;
            let shown: Vec<&str> = if above.len() > 4 {
                let n = above.len();
                vec![above[0].as_str(), "…", above[n - 2].as_str(), above[n - 1].as_str()]
            } else {
                above.iter().map(String::as_str).collect()
            };
            for name in shown {
                ui.label(RichText::new(name).font(mono(13.0)).color(MUTED));
                ui.label(RichText::new("/").font(mono(13.0)).color(DIM));
            }
            let mut node = tree;
            for depth in 0..=self.view.len() {
                if depth > 0 {
                    ui.label(RichText::new("/").font(mono(13.0)).color(DIM));
                    node = &node.children[self.view[depth - 1]];
                }
                let name = if depth == 0 { self.root_crumbs.label.as_str() } else { &node.name };
                if crumb(ui, name, depth == self.view.len()).clicked() && depth < self.view.len() {
                    actions.push(Action::ZoomTo(self.view[..depth].to_vec()));
                }
            }
            let (cursor, _) = ui.allocate_exact_size(vec2(8.0, 16.0), Sense::hover());
            ui.painter().rect_filled(cursor, 0.0, CYAN);
        });
    }

    // -------------------------------------------------------- status bar --

    fn status_bar(&mut self, ui: &mut Ui) {
        let framed = theme::framed(ui, SURFACE, Stroke::new(1.0, BORDER), 0.0, None, Margin::symmetric(16, 8), |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (state, color) = match (&self.scan, self.last_scan) {
                    (Some(_), _) => ("SCANNING", CYAN),
                    (None, Some((_, true))) => ("STOPPED EARLY", YELLOW),
                    (None, Some(_)) => ("SCAN COMPLETE", GREEN),
                    (None, None) => ("READY", MUTED),
                };
                let (light, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                theme::status_light(ui.painter(), light.center(), color);
                ui.label(RichText::new(state).font(mono(12.0)).color(color).extra_letter_spacing(1.0));
                ui.add_space(10.0);

                if let Some(scan) = &self.scan {
                    let p = &scan.progress;
                    ui.label(RichText::new(format_size(p.bytes.load(Ordering::Relaxed))).font(mono(12.0)).color(TEXT));
                    ui.label(RichText::new("so far in").font(mono(12.0)).color(MUTED));
                    ui.label(RichText::new(group(p.files.load(Ordering::Relaxed))).font(mono(12.0)).color(TEXT));
                    ui.label(RichText::new("files").font(mono(12.0)).color(MUTED));
                } else if let (Some(tree), Some((elapsed, _))) = (&self.tree, self.last_scan) {
                    ui.label(RichText::new(format_size(tree.size)).font(mono(12.0)).color(TEXT));
                    ui.label(RichText::new("in").font(mono(12.0)).color(MUTED));
                    ui.label(RichText::new(format!("{:.1} s", elapsed.as_secs_f32())).font(mono(12.0)).color(TEXT));
                }
                ui.add_space(10.0);

                let volume_path = self.root_path.clone().unwrap_or_else(|| PathBuf::from("/"));
                if let Some(vol) = volume_info(&volume_path) {
                    ui.label(RichText::new(volume_display_name(&vol.mount, &self.startup_name).to_uppercase()).font(mono(12.0)).color(MUTED));
                    let (bar, _) = ui.allocate_exact_size(vec2(180.0, 8.0), Sense::hover());
                    usage_bar(ui.painter(), bar, vol.used_fraction());
                    ui.label(RichText::new(format_size(vol.free)).font(mono(12.0)).color(TEXT));
                    ui.label(RichText::new("free of").font(mono(12.0)).color(MUTED));
                    ui.label(RichText::new(format_size(vol.total)).font(mono(12.0)).color(TEXT));
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.scan.is_none() && self.error_count > 0 {
                        let label = format!("! {} {}", self.error_count, plural(self.error_count, "item skipped or unreadable", "items skipped or unreadable"));
                        if Button::new(&label, Tone::Outline(YELLOW)).mono().small().show(ui).clicked() {
                            self.show_errors = true;
                        }
                    }
                    if let Some((message, is_error)) = self.message.clone() {
                        if Button::new("Dismiss", Tone::Ghost).small().show(ui).clicked() {
                            self.message = None;
                        }
                        let color = if is_error { MAGENTA } else { TEXT };
                        ui.add(egui::Label::new(RichText::new(message).font(body(12.0)).color(color)).truncate());
                    }
                });
            });
        });
        let r = framed.response.rect;
        ui.painter().line_segment([r.left_top(), r.right_top()], Stroke::new(1.0, CYAN));
    }

    // -------------------------------------------------------- side panel --

    fn side_panel(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        theme::framed(ui, SURFACE, Stroke::new(1.0, BORDER), 0.0, None, 14, |ui| {
            ui.set_min_size(ui.available_size());
            if self.scan.is_some() {
                self.telemetry(ui);
                return;
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let w = ui.available_width() / 3.0;
                for (label, tab) in [("CONTENTS", SideTab::Contents), ("LARGEST", SideTab::LargestFiles), ("AI", SideTab::Ai)] {
                    if theme::tab(ui, label, self.side_tab == tab, w).clicked() {
                        self.side_tab = tab;
                    }
                }
            });
            ui.add_space(4.0);
            if self.side_tab == SideTab::Ai {
                self.ai_panel(ui, actions);
                return;
            }
            theme::caption(ui, "FILTER BY NAME", MUTED);
            theme::framed(ui, BG, Stroke::new(1.0, BORDER), 0.0, None, Margin::symmetric(12, 9), |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(">").font(mono(14.0)).color(CYAN));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.filter)
                            .frame(Frame::NONE)
                            .font(mono(14.0))
                            .text_color(TEXT)
                            .hint_text(RichText::new("node_modules").font(mono(14.0)).color(DIM))
                            .desired_width(f32::INFINITY),
                    );
                });
            });
            ui.add_space(2.0);

            let Some(tree) = &self.tree else { return };
            let Some(view_node) = tree.at(&self.view) else { return };
            let filter = self.filter.to_lowercase();
            let matches = |name: &str| filter.is_empty() || name.to_lowercase().contains(&filter);
            let scanning = self.scan.is_some();
            let ai_ready = self.ai_agent().is_some();

            match self.side_tab {
                SideTab::Contents => {
                    let rows: Vec<usize> =
                        (0..view_node.children.len()).filter(|&i| matches(&view_node.children[i].name)).collect();
                    if rows.is_empty() {
                        ui.label(RichText::new("No items.").color(MUTED));
                    }
                    egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, ROW_H, rows.len(), |ui, range| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for &i in &rows[range] {
                            let child = &view_node.children[i];
                            let mut path = self.view.clone();
                            path.push(i);
                            let detail = if child.is_dir { format!("{} files", group(child.files)) } else { String::new() };
                            item_row(ui, tree, &path, child, view_node.size, &detail, self.selected.as_ref(), scanning, ai_ready, actions);
                        }
                    });
                }
                SideTab::LargestFiles => {
                    let stale =
                        self.largest.as_ref().is_none_or(|c| c.view != self.view || c.generation != self.generation);
                    if stale {
                        let rows = view_node
                            .largest_files(300)
                            .into_iter()
                            .map(|(rel, n)| {
                                let mut abs = self.view.clone();
                                abs.extend_from_slice(&rel);
                                let parent = view_node.names_along(&rel[..rel.len() - 1]).join("/");
                                (abs, n.name.clone(), n.size, parent)
                            })
                            .collect();
                        self.largest = Some(LargestCache { view: self.view.clone(), generation: self.generation, rows });
                    }
                    let cache = self.largest.as_ref().unwrap();
                    let rows: Vec<&(Vec<usize>, Box<str>, u64, String)> =
                        cache.rows.iter().filter(|r| matches(&r.1)).collect();
                    if rows.is_empty() {
                        ui.label(RichText::new("No files.").color(MUTED));
                    }
                    egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, ROW_H, rows.len(), |ui, range| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for (path, _, _, parent) in &rows[range] {
                            if let Some(node) = tree.at(path) {
                                let detail = if parent.is_empty() { "./".to_string() } else { format!("{parent}/") };
                                item_row(ui, tree, path, node, view_node.size, &detail, self.selected.as_ref(), scanning, ai_ready, actions);
                            }
                        }
                    });
                }
                // Drawn above, before the filter box.
                SideTab::Ai => {}
            }
        });
    }

    /// The AI tab: what gets sent, the question in progress, and the answer.
    fn ai_panel(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let wrapped = |ui: &mut Ui, text: &str, font: egui::FontId, color: Color32| {
            ui.add(egui::Label::new(RichText::new(text).font(font).color(color)).wrap());
        };
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 12.0;
            let Some(provider) = self.settings.ai_provider else {
                theme::caption(ui, "// AI ASSISTANT", MUTED);
                wrapped(
                    ui,
                    "Ask the AI agent you already use, Claude Code or Codex, whether an item is safe to remove. \
                     Choose one in Settings to turn this on.",
                    body(14.0),
                    TEXT,
                );
                if Button::new("Open Settings", Tone::Outline(CYAN)).small().show(ui).clicked() {
                    actions.push(Action::OpenSettings);
                }
                return;
            };
            let name = provider.label();
            if self.ai_agent().is_none() {
                theme::framed(ui, YELLOW_DEEP, Stroke::new(1.0, YELLOW), 0.0, None, 14, |ui| {
                    ui.set_width(ui.available_width());
                    wrapped(ui, &format!("{name} isn't installed on this Mac. Install it, or choose another agent in Settings."), body(13.0), YELLOW);
                });
            }

            if let Some(job) = &self.ai_job {
                theme::framed(ui, BG, Stroke::new(1.0, CYAN), 0.0, Some(CYAN), 16, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 8.0;
                    theme::caption(ui, &format!("// ASKING {}", name.to_uppercase()), CYAN);
                    ai_subject_header(ui, &job.subject);
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().color(CYAN));
                        let t = job.started.elapsed().as_secs_f32();
                        ui.label(RichText::new(format!("T+{:02}:{:04.1}", (t / 60.0) as u32, t % 60.0)).font(mono(12.0)).color(MUTED));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if Button::new("Cancel", Tone::Ghost).small().show(ui).clicked() {
                                actions.push(Action::CancelAi);
                            }
                        });
                    });
                });
            } else if let Some(answer) = &self.ai_answer {
                let item_path = self.ai_subject_path(&answer.subject);
                ai_subject_header(ui, &answer.subject);
                match &answer.result {
                    Err(e) => {
                        theme::framed(ui, YELLOW_DEEP, Stroke::new(1.0, YELLOW), 0.0, None, 14, |ui| {
                            ui.set_width(ui.available_width());
                            wrapped(ui, e, body(13.0), YELLOW);
                        });
                    }
                    Ok(a) => {
                        verdict_badge(ui, a.verdict);
                        theme::caption(ui, "WHAT IT IS", MUTED);
                        wrapped(ui, &a.what_it_is, body(14.0), TEXT);
                        if !a.risks.is_empty() {
                            theme::caption(ui, "RISKS", MUTED);
                            for risk in &a.risks {
                                wrapped(ui, &format!("•  {risk}"), body(13.0), TEXT);
                            }
                        }
                        theme::caption(ui, "COMES BACK ON ITS OWN", MUTED);
                        let again = if a.regenerates { "Yes. macOS or an app recreates it when needed." } else { "No. Once removed, it stays gone." };
                        wrapped(ui, again, body(13.0), TEXT);
                        if !a.safer_way.trim().is_empty() {
                            theme::caption(ui, "SAFER WAY", MUTED);
                            wrapped(ui, &a.safer_way, body(13.0), TEXT);
                        }
                    }
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let can_trash = item_path.as_ref().is_some_and(|p| !p.is_empty()) && !self.scanning();
                    if Button::new("Move to Trash…", Tone::Outline(MAGENTA)).small().enabled(can_trash).show(ui).clicked()
                        && let Some(p) = &item_path
                    {
                        actions.push(Action::AskTrash(p.clone()));
                    }
                    let can_ask = item_path.is_some() && self.ai_agent().is_some() && !self.scanning();
                    if Button::new("Ask again", Tone::Ghost).small().enabled(can_ask).show(ui).clicked()
                        && let Some(p) = item_path
                    {
                        actions.push(Action::AskAi(p, true));
                    }
                });
                wrapped(
                    ui,
                    &format!("Answered by {}. AI advice can be wrong, so check before you remove anything.", answer.subject.provider.label()),
                    body(12.0),
                    MUTED,
                );
                let (rule, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                ui.painter().rect_filled(rule, 0.0, BORDER);
            }

            theme::caption(ui, "// HOW IT WORKS", MUTED);
            wrapped(
                ui,
                &format!(
                    "Right-click any item and choose Ask AI… to get an answer here. {name} gets its path (with your home folder shown as ~), \
                     size, type, age, and the names and sizes of the {} largest items inside. It never gets file \
                     contents. It answers using your own {name} sign-in.",
                    ai::TOP_ITEMS
                ),
                body(13.0),
                MUTED,
            );
        });
    }

    /// The side panel while a scan runs: live counters and a short log.
    fn telemetry(&self, ui: &mut Ui) {
        let Some(scan) = &self.scan else { return };
        let p = &scan.progress;
        ui.spacing_mut().item_spacing.y = 12.0;
        theme::caption(ui, "// TELEMETRY", MUTED);
        theme::framed(ui, BG, Stroke::new(1.0, CYAN), 0.0, Some(CYAN), 16, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            theme::caption(ui, "MEASURED", MUTED);
            let size = format_size(p.bytes.load(Ordering::Relaxed));
            let (value, unit) = size.rsplit_once(' ').unwrap_or((&size, ""));
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 50.0), Sense::hover());
            let painter = ui.painter();
            let num = theme::glow_text(painter, row.left_center(), Align2::LEFT_CENTER, value, mono_medium(42.0), CYAN, 0.0);
            theme::text(painter, pos2(num.right() + 8.0, num.bottom() - 8.0), Align2::LEFT_BOTTOM, unit, mono(18.0), TEXT, 0.0);
            let t = scan.started.elapsed().as_secs_f32();
            ui.label(RichText::new(format!("T+{:02}:{:04.1}", (t / 60.0) as u32, t % 60.0)).font(mono(12.0)).color(MUTED));
        });

        let half = (ui.available_width() - 8.0) / 2.0;
        let tiles = [
            ("FILES", group(p.files.load(Ordering::Relaxed)), BORDER, TEXT),
            ("FOLDERS", group(p.dirs.load(Ordering::Relaxed)), BORDER, TEXT),
            ("CORES", format!("{} active", rayon::current_num_threads()), BORDER, TEXT),
            ("SKIPPED", group(p.errors.load(Ordering::Relaxed)), YELLOW, YELLOW),
        ];
        for pair in tiles.chunks(2) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for (label, value, edge, color) in pair {
                    ui.allocate_ui(vec2(half, 64.0), |ui| {
                        theme::framed(ui, BG, Stroke::new(1.0, *edge), 0.0, None, Margin::symmetric(12, 10), |ui| {
                            ui.set_width(half - 24.0);
                            ui.spacing_mut().item_spacing.y = 4.0;
                            let label_color = if *edge == YELLOW { YELLOW } else { MUTED };
                            theme::caption(ui, label, label_color);
                            ui.label(RichText::new(value).font(mono(18.0)).color(*color));
                        });
                    });
                }
            });
        }

        theme::framed(ui, BG, Stroke::new(1.0, BORDER), 0.0, None, 12, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            if scan.log.is_empty() {
                ui.label(RichText::new("> starting…").font(mono(12.0)).color(MUTED));
            }
            let last = scan.log.len().saturating_sub(1);
            for (i, line) in scan.log.iter().enumerate() {
                let color = if i == last { CYAN } else { MUTED };
                ui.add(egui::Label::new(RichText::new(format!("> read {line}")).font(mono(12.0)).color(color)).truncate());
            }
        });
        ui.label(
            RichText::new("Finished folders land on the map right away, so you can explore them while the rest are measured.")
                .font(body(13.0))
                .color(MUTED),
        );
    }

    // ------------------------------------------------------ start screen --

    fn start_screen(&mut self, ui: &mut Ui) {
        let avail = ui.available_rect_before_wrap();
        let volume = volume_info(Path::new("/"));
        if avail.width() >= 960.0 {
            let gap = 20.0;
            let left_w = ((avail.width() - gap) * 0.45).floor();
            let left = Rect::from_min_size(avail.min, vec2(left_w, avail.height()));
            let right = Rect::from_min_max(pos2(left.right() + gap, avail.top()), avail.max);
            ui.scope_builder(UiBuilder::new().max_rect(left), |ui| self.volume_card(ui, volume.as_ref(), left.size()));
            ui.scope_builder(UiBuilder::new().max_rect(right), |ui| {
                ui.add_space(((right.height() - 600.0) / 2.0).max(0.0));
                self.targets(ui);
            });
        } else {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                self.volume_card(ui, volume.as_ref(), vec2(avail.width(), 560.0));
                ui.add_space(20.0);
                self.targets(ui);
            });
        }
    }

    fn volume_card(&self, ui: &mut Ui, volume: Option<&VolumeInfo>, size: egui::Vec2) {
        theme::framed(ui, PANEL, Stroke::new(1.0, BORDER), 0.0, None, 28, |ui| {
            let inner = size - vec2(56.0, 56.0);
            ui.set_min_size(inner);
            ui.set_max_width(inner.x);
            ui.horizontal(|ui| {
                theme::caption(ui, "// VOLUME", MUTED);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let label = match volume {
                        Some(v) => format!("{} · {}", self.startup_name.to_uppercase(), v.fs_type.to_uppercase()),
                        None => self.startup_name.to_uppercase(),
                    };
                    theme::caption(ui, &label, MUTED);
                });
            });
            let Some(v) = volume else {
                ui.label(RichText::new("Couldn't read the startup disk.").color(MUTED));
                return;
            };
            let ring = (inner.x.min(inner.y - 170.0)).clamp(200.0, 380.0);
            ui.add_space(((inner.y - ring - 150.0) / 2.0).max(8.0));
            ui.vertical_centered(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(ring, ring), Sense::hover());
                capacity_ring(ui.painter(), rect, v.used_fraction());
            });
            ui.add_space(20.0);
            let tiles = [
                ("USED", v.total.saturating_sub(v.free), MAGENTA),
                ("FREE", v.free, BORDER),
                ("TOTAL", v.total, BORDER),
            ];
            let width = inner.x.min(460.0);
            ui.horizontal(|ui| {
                ui.add_space((inner.x - width) / 2.0);
                ui.spacing_mut().item_spacing.x = 8.0;
                let tile_w = (width - 16.0) / 3.0;
                for (label, value, edge) in tiles {
                    theme::framed(ui, Color32::TRANSPARENT, Stroke::new(1.0, edge), 0.0, None, Margin::symmetric(10, 10), |ui| {
                        ui.set_width(tile_w - 20.0);
                        ui.vertical_centered(|ui| {
                            ui.spacing_mut().item_spacing.y = 4.0;
                            theme::caption(ui, label, if edge == MAGENTA { MAGENTA_SOFT } else { MUTED });
                            ui.label(RichText::new(format_size(value)).font(mono(17.0)).color(TEXT));
                        });
                    });
                }
            });
        });
    }

    fn targets(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let width = ui.available_width().min(640.0);
        ui.set_max_width(width);
        ui.spacing_mut().item_spacing.y = 16.0;
        ui.label(RichText::new("> SELECT TARGET_").font(mono(12.0)).color(CYAN).extra_letter_spacing(1.7));
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(RichText::new("FIND WHAT'S EATING").font(display_bold(48.0)).color(TEXT));
            let painter = ui.painter().clone();
            let g = theme::galley(&painter, "YOUR DISK.", display_bold(48.0), 0.0, f32::INFINITY);
            let (rect, _) = ui.allocate_exact_size(g.size(), Sense::hover());
            theme::glow_text(&painter, rect.min, Align2::LEFT_TOP, "YOUR DISK.", display_bold(48.0), MAGENTA, 0.0);
        });
        ui.label(
            RichText::new("Pick something to scan. Folders become boxes sized by the space they take, so the heavy ones light up.")
                .font(body(16.0))
                .color(MUTED),
        );

        let home = std::env::var_os("HOME").map(PathBuf::from);
        let downloads = home.as_ref().map(|h| h.join("Downloads"));
        let card_w = (width - 12.0) / 2.0;
        let mut pick: Option<PathBuf> = None;
        let mut choose = false;
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let home_label = home.as_deref().map(|p| p.display().to_string()).unwrap_or_default();
            if target_card(ui, card_w, Icon::Home, MAGENTA, "HOME FOLDER", &home_label, Some("SUGGESTED"), CardStyle::Suggested).clicked() {
                pick = home.clone();
            }
            if target_card(ui, card_w, Icon::Disk, CYAN, "WHOLE DISK", &self.startup_name, None, CardStyle::Normal).clicked() {
                pick = Some(PathBuf::from("/"));
            }
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let dl_label = downloads.as_deref().map(|p| p.display().to_string()).unwrap_or_default();
            if target_card(ui, card_w, Icon::Download, CYAN, "DOWNLOADS", &dl_label, None, CardStyle::Normal).clicked() {
                pick = downloads.clone();
            }
            if target_card(ui, card_w, Icon::FolderPlus, CYAN, "CHOOSE A FOLDER…", "⌘O", None, CardStyle::Dashed).clicked() {
                choose = true;
            }
        });
        if let Some(p) = pick {
            self.start_scan(p, &ctx);
        }
        if choose {
            self.choose_folder(&ctx);
        }

        if self.full_disk_access {
            ui.horizontal(|ui| {
                let (light, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                theme::status_light(ui.painter(), light.center(), GREEN);
                ui.label(
                    RichText::new("FULL DISK ACCESS GRANTED · every folder can be scanned without prompts")
                        .font(mono(12.0))
                        .color(MUTED),
                );
            });
        } else {
            let mut setup = false;
            theme::framed(ui, YELLOW_DEEP, Stroke::new(1.0, YELLOW), 0.0, None, Margin::symmetric(16, 12), |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("!").font(mono_medium(14.0)).color(YELLOW));
                    let rest = if self.settings.skip_protected {
                        "Folders that would make macOS ask for permission are skipped."
                    } else {
                        "macOS may ask for permission folder by folder."
                    };
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        setup = Button::new("Set up access", Tone::Outline(YELLOW)).height(36.0).size(13.0).show(ui).clicked();
                        ui.with_layout(Layout::left_to_right(Align::Center).with_main_wrap(true), |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.label(RichText::new("Full Disk Access is off.").font(body_semibold(14.0)).color(YELLOW));
                            ui.label(RichText::new(rest).font(body(14.0)).color(TEXT));
                        });
                    });
                });
            });
            if setup {
                self.open_settings();
            }
        }
    }

    // ---------------------------------------------------------- treemap --

    fn treemap_section(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        theme::framed(ui, PANEL, Stroke::new(1.0, BORDER), 0.0, None, 12, |ui| {
            ui.set_min_size(ui.available_size());
            ui.spacing_mut().item_spacing.y = 10.0;
            let (title, right, right_color) = match (&self.tree, self.tree.as_ref().and_then(|t| t.at(&self.view))) {
                (_, Some(node)) if self.scan.is_some() && self.view.is_empty() => {
                    (String::from("// SECTOR MAP · MAPPING"), format!("{} · LIVE", format_size(node.size)), CYAN)
                }
                (_, Some(node)) => {
                    let place = self.fs_path(&self.view).map(|p| tilde(&p)).unwrap_or_default();
                    (
                        format!("// SECTOR MAP · {place}"),
                        format!("{} · {} {}", format_size(node.size), group(node.children.len() as u64), plural(node.children.len() as u64, "NODE", "NODES")),
                        MUTED,
                    )
                }
                _ => (String::new(), String::new(), MUTED),
            };
            // Lay out the readout first so a long path truncates instead of
            // running into it.
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 16.0), Layout::right_to_left(Align::Center), |ui| {
                theme::caption(ui, &right, right_color);
                ui.add_space(16.0);
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(egui::Label::new(RichText::new(title).font(mono(11.0)).color(MUTED).extra_letter_spacing(1.5)).truncate());
                });
            });
            let height = ui.available_height() - 28.0;
            self.treemap(ui, actions, height.max(80.0));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 18.0;
                for (label, kind) in theme::LEGEND {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let (sw, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                        let st = theme::kind_style(kind);
                        ui.painter().rect(sw, 0.0, st.fill, Stroke::new(1.0, st.edge), StrokeKind::Inside);
                        theme::caption(ui, label, MUTED);
                    });
                }
            });
        });
    }

    fn treemap(&mut self, ui: &mut Ui, actions: &mut Vec<Action>, height: f32) {
        let (response, painter) = ui.allocate_painter(vec2(ui.available_width(), height), Sense::click());
        let Some(tree) = &self.tree else { return };
        let Some(view_node) = tree.at(&self.view) else {
            self.view.clear();
            return;
        };
        let bounds = response.rect;
        painter.rect_filled(bounds, 0.0, BG);
        if view_node.size == 0 {
            let msg = if self.scanning() { "MAPPING_" } else { "EMPTY OR UNREADABLE" };
            theme::text(&painter, bounds.center(), Align2::CENTER_CENTER, msg, mono(13.0), if self.scanning() { CYAN } else { MUTED }, 2.0);
            return;
        }

        let mut draw = TreemapDraw {
            painter: &painter,
            pointer: response.hover_pos(),
            max_depth: self.settings.max_depth,
            hit: None,
            selected: self.selected.as_deref(),
            selected_rect: None,
        };
        let mut path = self.view.clone();
        draw.node(view_node, bounds, 0, &mut path);
        let (hit, selected_rect) = (draw.hit.take(), draw.selected_rect);

        if let Some(r) = selected_rect {
            theme::edge_glow(&painter, r.shrink(1.0), CYAN);
            painter.rect_stroke(r.shrink(1.0), 0.0, Stroke::new(2.0, CYAN), StrokeKind::Inside);
        }
        if let Some((_, r)) = &hit {
            let r = r.shrink(1.5);
            theme::edge_glow(&painter, r, MAGENTA);
            painter.rect_filled(r, 0.0, theme::glow_color(MAGENTA, 0.08));
            painter.rect_stroke(r, 0.0, Stroke::new(1.5, MAGENTA), StrokeKind::Inside);
        }

        if response.clicked() {
            match &hit {
                Some((p, _)) => actions.push(Action::Select(p.clone())),
                None => self.selected = None,
            }
        }
        if response.double_clicked()
            && let Some((p, _)) = &hit
        {
            actions.push(Action::ZoomTo(p.clone()));
        }
        let target_id = Id::unique("context_target");
        if response.secondary_clicked()
            && let Some((p, _)) = &hit
        {
            ui.ctx().data_mut(|d| d.insert_temp(target_id, p.clone()));
            actions.push(Action::Select(p.clone()));
        }

        let menu_open = response.context_menu_opened();
        if !menu_open
            && let (Some((p, _)), Some(pointer)) = (&hit, response.hover_pos())
            && let Some(node) = tree.at(p)
        {
            let full = self.fs_path(p).map(|f| f.display().to_string()).unwrap_or_default();
            hover_card(ui.ctx(), pointer, node, &full, view_node.size);
        }

        let scanning = self.scanning();
        let ai_ready = self.ai_agent().is_some();
        let target: Option<Vec<usize>> = ui.ctx().data(|d| d.get_temp(target_id));
        response.context_menu(|ui| {
            if let Some(target) = &target {
                context_menu_items(ui, tree, target, scanning, ai_ready, actions);
            }
        });
    }
}

impl eframe::App for SpaceHogApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_scan(&ctx);
        self.poll_ai(&ctx);
        while let Ok(event) = self.menu_events.try_recv() {
            if event.id.as_ref() == MENU_SETTINGS {
                self.open_settings();
            }
        }
        let mut actions = Vec::new();

        if !ctx.egui_wants_keyboard_input() {
            if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::Comma)) {
                self.open_settings();
            }
            let (up, open, rescan, escape) = ctx.input(|i| {
                (
                    i.key_pressed(Key::Backspace) || (i.modifiers.command && i.key_pressed(Key::ArrowUp)),
                    i.modifiers.command && i.key_pressed(Key::O),
                    i.modifiers.command && i.key_pressed(Key::R),
                    i.key_pressed(Key::Escape),
                )
            });
            if up {
                actions.push(Action::Up);
            }
            if open {
                self.choose_folder(&ctx);
            }
            if rescan
                && !self.scanning()
                && let Some(p) = self.root_path.clone()
            {
                self.start_scan(p, &ctx);
            }
            // Escape closes the topmost window; with none open it stops a
            // scan. It never navigates, since it also closes menus.
            if escape && self.trash_request.is_none() {
                if self.show_settings {
                    self.show_settings = false;
                } else if self.show_errors {
                    self.show_errors = false;
                } else if self.scanning() {
                    self.stop_scan();
                }
            }
        }

        egui::Panel::top("header")
            .frame(Frame::NONE.fill(BG).inner_margin(Margin { left: 16, right: 16, top: 14, bottom: 6 }))
            .show_separator_line(false)
            .show(ui, |ui| {
                self.header(ui);
                if self.tree.is_some() {
                    ui.add_space(6.0);
                    self.path_bar(ui, &mut actions);
                }
            });
        egui::Panel::bottom("status")
            .frame(Frame::NONE.fill(BG).inner_margin(Margin { left: 16, right: 16, top: 6, bottom: 14 }))
            .show_separator_line(false)
            .show(ui, |ui| self.status_bar(ui));
        if self.tree.is_some() {
            egui::Panel::right("side")
                .frame(Frame::NONE.fill(BG).inner_margin(Margin { left: 0, right: 16, top: 6, bottom: 6 }))
                .show_separator_line(false)
                .resizable(true)
                .default_size(360.0)
                .min_size(280.0)
                .show(ui, |ui| self.side_panel(ui, &mut actions));
            egui::CentralPanel::default()
                .frame(Frame::NONE.fill(BG).inner_margin(Margin { left: 16, right: 12, top: 6, bottom: 6 }))
                .show(ui, |ui| self.treemap_section(ui, &mut actions));
        } else {
            egui::CentralPanel::default()
                .frame(Frame::NONE.fill(BG).inner_margin(Margin { left: 16, right: 16, top: 14, bottom: 6 }))
                .show(ui, |ui| self.start_screen(ui));
        }

        for action in actions {
            self.apply(action, &ctx);
        }

        self.trash_dialog(&ctx);
        if self.show_errors {
            self.errors_window(&ctx);
        }
        if self.show_settings {
            self.settings_window(&ctx);
        }
        if self.settings != self.saved_settings {
            self.save_settings();
        }
    }
}

// ----------------------------------------------------------- dialogs --

impl SpaceHogApp {
    fn open_settings(&mut self) {
        self.show_settings = true;
        self.full_disk_access = settings::has_full_disk_access();
    }

    fn save_settings(&mut self) {
        match self.settings.save() {
            Ok(()) => self.saved_settings = self.settings.clone(),
            Err(e) => {
                self.message = Some((format!("Couldn't save settings: {e}"), true));
                // Don't retry every frame.
                self.saved_settings = self.settings.clone();
            }
        }
    }

    fn trash_dialog(&mut self, ctx: &egui::Context) {
        let Some(request) = &self.trash_request else { return };
        let mut confirmed = false;
        let mut cancelled = false;
        let modal = egui::Modal::new(Id::unique("trash_modal"))
            .frame(Frame::NONE)
            .backdrop_color(Color32::from_black_alpha(200))
            .show(ctx, |ui| {
                theme::framed(ui, SURFACE, Stroke::new(1.0, MAGENTA), 22.0, Some(MAGENTA), 26, |ui| {
                    ui.set_width(520.0);
                    ui.spacing_mut().item_spacing.y = 16.0;
                    ui.horizontal(|ui| {
                        theme::caption(ui, "// CONFIRM ACTION", MAGENTA);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| theme::caption(ui, "REVERSIBLE", MUTED));
                    });
                    ui.label(
                        RichText::new(format!("Move “{}” to the Trash?", request.name)).font(display(22.0)).color(TEXT),
                    );
                    theme::framed(ui, BG, Stroke::new(1.0, BORDER), 0.0, None, Margin::symmetric(16, 14), |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 8.0;
                        ui.label(RichText::new(request.fs_path.display().to_string()).font(mono(12.0)).color(MUTED));
                        ui.horizontal(|ui| {
                            let st = theme::kind_style(if request.is_dir { Kind::Folder } else { request.kind });
                            let (sw, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                            let edge = if request.is_dir { GREY } else { st.edge };
                            ui.painter().rect(sw, 0.0, st.fill, Stroke::new(1.0, edge), StrokeKind::Inside);
                            let what = if request.is_dir {
                                format!("Folder · {} {}", group(request.files), plural(request.files, "file", "files"))
                            } else {
                                request.kind.label().to_string()
                            };
                            ui.label(RichText::new(what).font(body(13.0)).color(TEXT));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let g = theme::galley(ui.painter(), format_size(request.size), mono_medium(24.0), 0.0, f32::INFINITY);
                                let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
                                theme::glow_text(ui.painter(), r.min, Align2::LEFT_TOP, format_size(request.size), mono_medium(24.0), CYAN, 0.0);
                            });
                        });
                        if let Some((provider, answer)) = self.ai_cache.get(&(request.fs_path.clone(), request.size)) {
                            let color = verdict_color(answer.verdict);
                            ui.horizontal(|ui| {
                                let (light, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                                theme::status_light(ui.painter(), light.center(), color);
                                theme::caption(ui, &format!("{} SAYS: {}", provider.label().to_uppercase(), answer.verdict.label()), color);
                            });
                        }
                    });
                    ui.label(
                        RichText::new("Nothing is deleted for good. You can put it back from the Trash in Finder. The space is freed when you empty the Trash.")
                            .font(body(14.0))
                            .color(MUTED),
                    );
                    // Inside a row, so the right-aligned buttons don't claim
                    // the rest of the dialog's height.
                    ui.horizontal(|ui| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            if Button::new("Move to Trash", Tone::Primary(MAGENTA)).height(44.0).show(ui).clicked() {
                                confirmed = true;
                            }
                            if Button::new("Cancel", Tone::Ghost).height(44.0).show(ui).clicked() {
                                cancelled = true;
                            }
                        });
                    });
                });
            });
        if confirmed {
            let request = self.trash_request.take().unwrap();
            self.move_to_trash(request);
        } else if cancelled || modal.should_close() {
            self.trash_request = None;
        }
    }

    fn errors_window(&mut self, ctx: &egui::Context) {
        let mut close = false;
        let mut open_settings = false;
        egui::Window::new("skipped")
            .title_bar(false)
            .frame(Frame::NONE)
            .resizable(false)
            .auto_sized()
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                theme::framed(ui, SURFACE, Stroke::new(1.0, YELLOW), 18.0, None, 24, |ui| {
                    ui.set_width(660.0);
                    ui.spacing_mut().item_spacing.y = 12.0;
                    ui.horizontal(|ui| {
                        theme::caption(ui, "// SKIPPED OR UNREADABLE", YELLOW);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            theme::caption(ui, &group(self.error_count), YELLOW);
                        });
                    });
                    let why = if self.full_disk_access {
                        if self.error_count == 1 {
                            "it couldn't be read (the reason is shown below it)"
                        } else {
                            "they couldn't be read (the reason is shown below each one)"
                        }
                    } else {
                        "some were skipped to avoid macOS permission prompts and others were blocked by \
                         macOS privacy protection"
                    };
                    let listed = if self.error_count as usize > self.error_paths.len() {
                        format!(" The first {} are listed.", self.error_paths.len())
                    } else {
                        String::new()
                    };
                    ui.label(
                        RichText::new(format!(
                            "{} {} left out of the totals because {why}.{listed}",
                            group(self.error_count),
                            plural(self.error_count, "item was", "items were"),
                        ))
                        .font(body(14.0))
                        .color(MUTED),
                    );
                    theme::framed(ui, BG, Stroke::new(1.0, BORDER), 0.0, None, 12, |ui| {
                        ui.set_width(ui.available_width());
                        egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 6.0;
                            for (path, err) in &self.error_paths {
                                ui.label(RichText::new(tilde(path)).font(mono(12.0)).color(TEXT));
                                ui.label(RichText::new(format!("  {err}")).font(mono(11.0)).color(MUTED));
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            close = Button::new("Close", Tone::Ghost).height(40.0).show(ui).clicked();
                            if !self.full_disk_access {
                                open_settings = Button::new("Set up Full Disk Access", Tone::Outline(YELLOW)).height(40.0).show(ui).clicked();
                            }
                        });
                    });
                });
            });
        if close {
            self.show_errors = false;
        }
        if open_settings {
            self.show_errors = false;
            self.open_settings();
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut close = false;
        let mut relaunch = false;
        egui::Window::new("settings")
            .title_bar(false)
            .frame(Frame::NONE)
            .resizable(false)
            .auto_sized()
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                theme::framed(ui, PANEL, Stroke::new(1.0, BORDER), 0.0, Some(CYAN), 32, |ui| {
                    let width = 696.0;
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing.y = 18.0;
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("SETTINGS").font(display_bold(26.0)).color(TEXT).extra_letter_spacing(2.0));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new("⌘,").font(mono(12.0)).color(MUTED));
                        });
                    });
                    let (rule, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
                    ui.painter().rect_filled(rule, 0.0, BORDER);
                    // Scrolls when the window is too short for every section.
                    let max_h = (ctx.content_rect().height() - 260.0).max(200.0);
                    egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 18.0;
                        let (edge, status) = if self.full_disk_access { (GREEN, "GRANTED") } else { (YELLOW, "NOT GRANTED") };
                        theme::framed(ui, BG, Stroke::new(1.0, edge), 18.0, None, 20, |ui| {
                            ui.set_width(width - 40.0);
                            ui.spacing_mut().item_spacing.y = 14.0;
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("FULL DISK ACCESS").font(display(17.0)).color(TEXT).extra_letter_spacing(0.7));
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if Button::new("Check again", Tone::Ghost).small().show(ui).clicked() {
                                        self.full_disk_access = settings::has_full_disk_access();
                                    }
                                    // A fixed slot inside the right-to-left row keeps the
                                    // badge's reading order and its natural width.
                                    ui.allocate_ui_with_layout(vec2(140.0, 30.0), Layout::right_to_left(Align::Center), |ui| {
                                        ui.allocate_ui_with_layout(vec2(140.0, 30.0), Layout::left_to_right(Align::Center), |ui| {
                                            theme::framed(ui, Color32::TRANSPARENT, Stroke::new(1.0, edge), 0.0, None, Margin::symmetric(10, 6), |ui| {
                                                ui.horizontal(|ui| {
                                                    let (light, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                                                    theme::status_light(ui.painter(), light.center(), edge);
                                                    theme::caption(ui, status, edge);
                                                });
                                            });
                                        });
                                    });
                                });
                            });
                            let about = if self.full_disk_access {
                                "Space Hog Inspector can read every folder, and macOS won't ask for permission folder by folder."
                            } else {
                                "Without it, macOS asks separately before the app can read Desktop, Documents, Downloads, \
                                 iCloud Drive, other apps' data and external drives, and hides Mail, Messages and Safari \
                                 data completely. Turning it on once covers all of them."
                            };
                            ui.label(RichText::new(about).font(body(14.0)).color(MUTED));
                            if !self.full_disk_access {
                                let card_w = (width - 40.0 - 20.0) / 3.0;
                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing.x = 10.0;
                                    let steps = [
                                        ("[01]", "Open the Full Disk Access list", "Open System Settings", true),
                                        ("[02]", "Switch on Space Hog Inspector, or drag it into the list", "Show app in Finder", false),
                                        ("[03]", "Relaunch so macOS applies the change", "Relaunch", false),
                                    ];
                                    for (i, (num, text, action, primary)) in steps.into_iter().enumerate() {
                                        theme::framed(ui, SURFACE, Stroke::new(1.0, BORDER), 0.0, None, 14, |ui| {
                                            ui.set_width(card_w - 28.0);
                                            ui.spacing_mut().item_spacing.y = 10.0;
                                            ui.label(RichText::new(num).font(mono(12.0)).color(CYAN));
                                            ui.allocate_ui(vec2(card_w - 28.0, 36.0), |ui| {
                                                ui.set_min_height(36.0);
                                                ui.label(RichText::new(text).font(body(13.0)).color(TEXT));
                                            });
                                            let tone = if primary { Tone::Primary(CYAN) } else { Tone::Outline(CYAN) };
                                            if Button::new(action, tone).size(12.0).width(card_w - 28.0).show(ui).clicked() {
                                                match i {
                                                    0 => {
                                                        if let Err(e) = settings::open_full_disk_access_settings() {
                                                            self.message = Some((format!("Couldn't open System Settings: {e}"), true));
                                                        }
                                                    }
                                                    1 => {
                                                        if let Err(e) = settings::reveal_app_in_finder() {
                                                            self.message = Some((format!("Couldn't open Finder: {e}"), true));
                                                        }
                                                    }
                                                    _ => relaunch = true,
                                                }
                                            }
                                        });
                                    }
                                });
                            }
                        });

                        theme::framed(ui, SURFACE, Stroke::new(1.0, BORDER), 0.0, None, Margin::symmetric(20, 14), |ui| {
                            ui.set_width(width - 40.0);
                            ui.spacing_mut().item_spacing.y = 12.0;
                            theme::caption(ui, "// SCANNING", MUTED);
                            let skip_note = if self.full_disk_access {
                                "Full Disk Access is on, so nothing needs to be skipped. A folder you pick yourself is never skipped."
                            } else {
                                "Only while Full Disk Access is off. Skipped folders are listed in the status bar. A folder you pick yourself is never skipped."
                            };
                            setting_switch(ui, "Skip folders that would make macOS ask for permission", skip_note, &mut self.settings.skip_protected);
                            setting_switch(
                                ui,
                                "Include other mounted volumes",
                                "Follow into external drives and network shares found inside the scanned folder.",
                                &mut self.settings.cross_volumes,
                            );
                            ui.horizontal(|ui| {
                                ui.allocate_ui(vec2(width - 40.0 - 260.0, 48.0), |ui| {
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 4.0;
                                        ui.label(RichText::new("Default treemap depth").font(body_medium(15.0)).color(TEXT));
                                        ui.label(RichText::new("How many nested levels the treemap draws.").font(body(13.0)).color(MUTED));
                                    });
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.allocate_ui_with_layout(vec2(220.0, 40.0), Layout::left_to_right(Align::Center), |ui| {
                                        theme::framed(ui, BG, Stroke::new(1.0, BORDER), 0.0, None, Margin::symmetric(14, 8), |ui| {
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = 12.0;
                                                ui.add(egui::Slider::new(&mut self.settings.max_depth, 1..=12).show_value(false));
                                                ui.label(RichText::new(format!("{:02}", self.settings.max_depth)).font(mono(13.0)).color(CYAN));
                                            });
                                        });
                                    });
                                });
                            });
                        });

                        theme::framed(ui, SURFACE, Stroke::new(1.0, BORDER), 0.0, None, Margin::symmetric(20, 14), |ui| {
                            ui.set_width(width - 40.0);
                            ui.spacing_mut().item_spacing.y = 12.0;
                            ui.horizontal(|ui| {
                                theme::caption(ui, "// AI ASSISTANT", MUTED);
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if Button::new("Detect again", Tone::Ghost).small().show(ui).clicked() {
                                        self.ai_agents = ai::detect();
                                    }
                                });
                            });
                            ui.label(
                                RichText::new(
                                    "Ask the AI agent you already use whether an item is safe to remove. It answers with \
                                     your own sign-in and plan, and gets each item's path, size, type, age and largest \
                                     contents, never file contents.",
                                )
                                .font(body(13.0))
                                .color(MUTED),
                            );
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 10.0;
                                let choices = [(None, "Off")].into_iter().chain(Provider::ALL.map(|p| (Some(p), p.label())));
                                for (choice, label) in choices {
                                    let installed = choice.is_none_or(|p| self.ai_agents.iter().any(|a| a.provider == p));
                                    let tone = if self.settings.ai_provider == choice { Tone::Primary(CYAN) } else { Tone::Ghost };
                                    if Button::new(label, tone).small().width(150.0).enabled(installed).show(ui).clicked() {
                                        self.settings.ai_provider = choice;
                                    }
                                }
                            });
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 4.0;
                                for provider in Provider::ALL {
                                    let found = self.ai_agents.iter().find(|a| a.provider == provider);
                                    let (status, color) = match found {
                                        Some(a) => (tilde(&a.exe), TEXT),
                                        None => ("not installed".to_string(), DIM),
                                    };
                                    ui.horizontal(|ui| {
                                        ui.allocate_ui_with_layout(vec2(110.0, 18.0), Layout::left_to_right(Align::Center), |ui| {
                                            ui.set_width(110.0);
                                            ui.label(RichText::new(provider.label()).font(body_medium(13.0)).color(TEXT));
                                        });
                                        ui.label(RichText::new(status).font(mono(12.0)).color(color));
                                    });
                                }
                            });
                            if self.settings.ai_provider == Some(Provider::Codex) {
                                ui.label(
                                    RichText::new(
                                        "Codex can't have its tools switched off. It runs in its read-only sandbox, but it \
                                         can still look at files on your disk while answering, using this app's Full Disk \
                                         Access.",
                                    )
                                    .font(body(13.0))
                                    .color(YELLOW),
                                );
                            }
                        });
                    });

                    ui.horizontal(|ui| {
                        let path = Settings::file().map(|p| tilde(&p)).unwrap_or_default();
                        ui.label(RichText::new(path).font(mono(11.0)).color(MUTED));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            close = Button::new("Done", Tone::Outline(TEXT)).height(44.0).show(ui).clicked();
                        });
                    });
                });
            });
        if close {
            self.show_settings = false;
        }
        if relaunch {
            self.save_settings();
            match settings::launch_new_instance() {
                Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Err(e) => self.message = Some((format!("Couldn't relaunch: {e}"), true)),
            }
        }
    }
}

/// One labelled on/off setting with a description, separated by a rule.
fn setting_switch(ui: &mut Ui, title: &str, note: &str, value: &mut bool) {
    let width = ui.available_width();
    ui.horizontal(|ui| {
        ui.allocate_ui(vec2(width - 80.0, 48.0), |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.label(RichText::new(title).font(body_medium(15.0)).color(TEXT));
                ui.label(RichText::new(note).font(body(13.0)).color(MUTED));
            });
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| theme::switch(ui, value, title));
    });
    let (rule, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
    ui.painter().rect_filled(rule, 0.0, BORDER);
}

// ------------------------------------------------------------- drawing --

struct TreemapDraw<'a> {
    painter: &'a Painter,
    pointer: Option<Pos2>,
    max_depth: usize,
    /// Deepest item under the pointer and its rectangle.
    hit: Option<(Vec<usize>, Rect)>,
    selected: Option<&'a [usize]>,
    selected_rect: Option<Rect>,
}

impl TreemapDraw<'_> {
    fn node(&mut self, node: &Node, rect: Rect, depth: usize, path: &mut Vec<usize>) {
        if rect.width() < 1.0 || rect.height() < 1.0 {
            return;
        }
        if self.pointer.is_some_and(|p| rect.contains(p)) {
            self.hit = Some((path.clone(), rect));
        }
        if self.selected == Some(path.as_slice()) {
            self.selected_rect = Some(rect);
        }
        // A small gap between neighbours keeps the boxes readable.
        let gap = if depth == 0 { 0.0 } else if rect.width() > 12.0 && rect.height() > 12.0 { 1.5 } else { 0.5 };
        let r = rect.shrink(gap);
        if node.is_dir {
            self.folder(node, r, depth, path);
        } else {
            self.file(node, r);
        }
    }

    fn folder(&mut self, node: &Node, rect: Rect, depth: usize, path: &mut Vec<usize>) {
        if depth == 0 {
            // The folder being viewed fills the map; the path bar already
            // names it, so lay its contents out directly.
            self.children(node, rect, depth, path);
            return;
        }
        let is_app = node.kind == Kind::App;
        let (fill, edge) = if is_app {
            let st = theme::kind_style(Kind::App);
            (st.fill, st.edge.gamma_multiply(0.7))
        } else {
            (SURFACE, BORDER)
        };
        self.painter.rect(rect, 0.0, fill, Stroke::new(1.0, edge), StrokeKind::Inside);

        let header_h = if depth <= 1 { 24.0 } else { 18.0 };
        let has_header = rect.width() >= 40.0 && rect.height() >= header_h + 12.0;
        let inner = if has_header {
            let header = Rect::from_min_size(rect.min, vec2(rect.width(), header_h));
            let clip = self.painter.with_clip_rect(header.shrink(1.0));
            let size_font = mono(if depth <= 1 { 11.0 } else { 10.0 });
            let name_font = display(if depth <= 1 { 12.0 } else { 11.0 });
            let size = theme::galley(self.painter, format_size(node.size), size_font, 0.0, f32::INFINITY);
            let name = theme::galley(self.painter, node.name.to_uppercase(), name_font, 0.9, f32::INFINITY);
            let fits_size = name.size().x + size.size().x + 28.0 < header.width();
            clip.galley(pos2(header.left() + 8.0, header.center().y - name.size().y / 2.0), name, if is_app { theme::kind_style(Kind::App).text } else { TEXT });
            if fits_size {
                clip.galley(pos2(header.right() - 8.0 - size.size().x, header.center().y - size.size().y / 2.0), size, MUTED);
            }
            self.painter.line_segment([header.left_bottom(), header.right_bottom()], Stroke::new(1.0, edge));
            Rect::from_min_max(rect.min + vec2(2.0, header_h + 2.0), rect.max - vec2(2.0, 2.0))
        } else {
            rect.shrink(1.5)
        };
        if inner.width() < 2.0 || inner.height() < 2.0 || node.children.is_empty() {
            return;
        }
        if depth + 1 >= self.max_depth {
            // Depth limit reached: show the contents as one block.
            self.painter.rect_filled(inner, 0.0, PANEL);
            if has_header && inner.height() > 16.0 && inner.width() > 60.0 {
                let label = format!("{} {}", group(node.children.len() as u64), plural(node.children.len() as u64, "ITEM", "ITEMS"));
                theme::text(&self.painter.with_clip_rect(inner), inner.min + vec2(8.0, 6.0), Align2::LEFT_TOP, label, mono(10.0), MUTED, 1.0);
            }
            return;
        }
        self.children(node, inner, depth, path);
    }

    fn children(&mut self, node: &Node, inner: Rect, depth: usize, path: &mut Vec<usize>) {
        let sizes: Vec<u64> = node.children.iter().map(|c| c.size).collect();
        let layout = squarify(&sizes, inner, MIN_ITEM_AREA);
        for (i, r) in layout.rects.iter().enumerate() {
            path.push(i);
            self.node(&node.children[i], *r, depth + 1, path);
            path.pop();
        }
        if let Some(rest) = layout.rest {
            let rest = rest.shrink(1.0);
            self.painter.rect_filled(rest, 0.0, PANEL);
            theme::dashed_rect(self.painter, rest, Stroke::new(1.0, DIM));
            if rest.width() > 70.0 && rest.height() > 18.0 {
                let count = node.children.len() - layout.rects.len();
                let label = format!("{} SMALL {}", group(count as u64), plural(count as u64, "ITEM", "ITEMS"));
                theme::text(&self.painter.with_clip_rect(rest), rest.min + vec2(6.0, 5.0), Align2::LEFT_TOP, label, mono(10.0), MUTED, 1.0);
            }
        }
    }

    fn file(&mut self, node: &Node, rect: Rect) {
        let st = theme::kind_style(node.kind);
        self.painter.rect(rect, 0.0, st.fill, Stroke::new(1.0, st.edge), StrokeKind::Inside);
        if rect.width() >= 44.0 && rect.height() >= 18.0 {
            let clip = self.painter.with_clip_rect(rect.shrink(2.0));
            if rect.height() >= 36.0 {
                theme::text(&clip, rect.min + vec2(8.0, 7.0), Align2::LEFT_TOP, node.name.to_string(), body_medium(12.0), st.text, 0.0);
                theme::text(&clip, rect.min + vec2(8.0, 23.0), Align2::LEFT_TOP, format_size(node.size), mono(11.0), st.text, 0.0);
            } else {
                let label = format!("{} {}", node.name, format_size(node.size));
                theme::text(&clip, pos2(rect.left() + 6.0, rect.center().y), Align2::LEFT_CENTER, label, body_medium(11.0), st.text, 0.0);
            }
        }
    }
}

/// The "target lock" readout that follows the pointer over the treemap.
fn hover_card(ctx: &egui::Context, pointer: Pos2, node: &Node, full_path: &str, view_size: u64) {
    let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::unique("hover_card")));
    let width = 300.0;
    let pad = vec2(16.0, 14.0);
    let inner_w = width - pad.x * 2.0;
    let path = theme::galley(&painter, full_path, mono(12.0), 0.0, inner_w);
    let name = theme::galley(&painter, node.name.to_string(), display(16.0), 0.0, inner_w);
    let detail = if node.is_dir {
        format!(
            "{} {} · double-click to zoom in · right-click for actions",
            group(node.files),
            plural(node.files, "file", "files")
        )
    } else {
        "Right-click for actions".to_string()
    };
    let detail = theme::galley(&painter, detail, body(11.0), 0.0, inner_w);
    let height = pad.y * 2.0 + 14.0 + 6.0 + name.size().y + 6.0 + path.size().y + 8.0 + 1.0 + 8.0 + 28.0 + 6.0 + detail.size().y;

    let screen = ctx.content_rect();
    let mut pos = pointer + vec2(18.0, 18.0);
    if pos.x + width > screen.right() - 8.0 {
        pos.x = pointer.x - width - 18.0;
    }
    if pos.y + height > screen.bottom() - 8.0 {
        pos.y = pointer.y - height - 18.0;
    }
    let card = Rect::from_min_size(pos, vec2(width, height));
    painter.add(theme::glow_shape(card, CYAN, 0.18, 26.0));
    painter.add(theme::chamfer_shape(card, 14.0, BG, Stroke::new(1.0, CYAN)));

    let (tag, tag_color) = if node.is_dir {
        let color = if node.kind == Kind::App { theme::kind_style(Kind::App).edge } else { CYAN };
        (theme::kind_tag(node.kind), color)
    } else {
        (theme::kind_tag(node.kind), theme::kind_style(node.kind).edge)
    };
    let mut y = card.top() + pad.y;
    let x = card.left() + pad.x;
    theme::text(&painter, pos2(x, y), Align2::LEFT_TOP, format!("■ {tag}"), mono(11.0), tag_color, 1.5);
    theme::text(&painter, pos2(card.right() - pad.x, y), Align2::RIGHT_TOP, "TARGET LOCK", mono(11.0), MUTED, 1.5);
    y += 14.0 + 6.0;
    let name_h = name.size().y;
    painter.galley(pos2(x, y), name, TEXT);
    y += name_h + 6.0;
    let path_h = path.size().y;
    painter.galley(pos2(x, y), path, MUTED);
    y += path_h + 8.0;
    painter.extend(Shape::dashed_line(&[pos2(x, y), pos2(card.right() - pad.x, y)], Stroke::new(1.0, BORDER), 4.0, 3.0));
    y += 1.0 + 8.0;
    theme::glow_text(&painter, pos2(x, y + 14.0), Align2::LEFT_CENTER, format_size(node.size), mono_medium(22.0), CYAN, 0.0);
    let pct = 100.0 * node.size as f64 / view_size.max(1) as f64;
    theme::text(&painter, pos2(card.right() - pad.x, y + 16.0), Align2::RIGHT_CENTER, format!("{pct:.1}%"), mono(12.0), MUTED, 0.0);
    y += 28.0 + 6.0;
    painter.galley(pos2(x, y), detail, MUTED);
}

#[derive(Clone, Copy, PartialEq)]
enum CardStyle {
    Suggested,
    Normal,
    Dashed,
}

/// A large start-screen target button.
#[allow(clippy::too_many_arguments)]
fn target_card(
    ui: &mut Ui,
    width: f32,
    ic: Icon,
    icon_color: Color32,
    title: &str,
    subtitle: &str,
    tag: Option<&str>,
    style: CardStyle,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 132.0), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, title));
    let painter = ui.painter();
    let hovered = response.hovered();
    match style {
        CardStyle::Suggested => {
            theme::glow(painter, rect, MAGENTA, if hovered { 0.4 } else { 0.25 }, 24.0);
            painter.add(theme::chamfer_shape(rect, 16.0, MAGENTA_DEEP, Stroke::new(1.0, MAGENTA)));
        }
        CardStyle::Normal => {
            painter.rect(rect, 0.0, if hovered { RAISED } else { SURFACE }, Stroke::new(1.0, if hovered { CYAN } else { BORDER }), StrokeKind::Inside);
        }
        CardStyle::Dashed => {
            if hovered {
                painter.rect_filled(rect, 0.0, RAISED);
            }
            theme::dashed_rect(painter, rect, Stroke::new(1.0, if hovered { CYAN } else { DIM }));
        }
    }
    let clip = painter.with_clip_rect(rect.shrink(4.0));
    let x = rect.left() + 18.0;
    theme::icon(&clip, Rect::from_min_size(pos2(x, rect.top() + 18.0), vec2(22.0, 22.0)), ic, icon_color);
    if let Some(tag) = tag {
        theme::text(&clip, pos2(rect.right() - 18.0, rect.top() + 22.0), Align2::RIGHT_TOP, tag, mono(11.0), MAGENTA_SOFT, 1.5);
    }
    theme::text(&clip, pos2(x, rect.top() + 58.0), Align2::LEFT_TOP, title, display(18.0), TEXT, 0.7);
    theme::text(&clip, pos2(x, rect.top() + 88.0), Align2::LEFT_TOP, subtitle, mono(12.0), MUTED, 0.0);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// One breadcrumb: the current folder is boxed in glowing cyan.
fn verdict_color(verdict: Verdict) -> Color32 {
    match verdict {
        Verdict::Safe => GREEN,
        Verdict::LikelySafe => CYAN,
        Verdict::Caution => YELLOW,
        Verdict::Keep => MAGENTA,
    }
}

/// The verdict as a neon status badge.
fn verdict_badge(ui: &mut Ui, verdict: Verdict) {
    let color = verdict_color(verdict);
    theme::framed(ui, BG, Stroke::new(1.0, color), 0.0, Some(color), Margin::symmetric(12, 8), |ui| {
        ui.horizontal(|ui| {
            let (light, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
            theme::status_light(ui.painter(), light.center(), color);
            ui.label(RichText::new(verdict.label()).font(display(15.0)).color(color).extra_letter_spacing(1.2));
        });
    });
}

/// The asked-about item's name, size and path.
fn ai_subject_header(ui: &mut Ui, subject: &AiSubject) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.horizontal(|ui| {
            let name = subject.names.last().map(|n| n.to_string()).unwrap_or_else(|| subject.facts.path.clone());
            let size = format_size(subject.facts.size);
            let size_w = theme::galley(ui.painter(), size.clone(), mono(13.0), 0.0, f32::INFINITY).size().x;
            // A fixed slot for the name, so truncating it leaves room for the size.
            let name_w = (ui.available_width() - size_w - 12.0).max(40.0);
            ui.allocate_ui_with_layout(vec2(name_w, 22.0), Layout::left_to_right(Align::Center), |ui| {
                ui.set_width(name_w);
                ui.add(egui::Label::new(RichText::new(name).font(display(16.0)).color(TEXT)).truncate());
            });
            ui.label(RichText::new(size).font(mono(13.0)).color(CYAN));
        });
        ui.add(egui::Label::new(RichText::new(&subject.facts.path).font(mono(11.0)).color(MUTED)).wrap());
    });
}

fn crumb(ui: &mut Ui, name: &str, current: bool) -> egui::Response {
    let painter = ui.painter().clone();
    let g = theme::galley(&painter, name, mono(13.0), 0.0, f32::INFINITY);
    let pad = if current { vec2(8.0, 4.0) } else { vec2(0.0, 4.0) };
    let (rect, response) = ui.allocate_exact_size(g.size() + pad * 2.0, if current { Sense::hover() } else { Sense::click() });
    response.widget_info(|| WidgetInfo::labeled(if current { Role::Label } else { Role::Link }, true, name));
    if current {
        theme::glow(&painter, rect, CYAN, 0.3, 12.0);
        painter.rect_stroke(rect, 0.0, Stroke::new(1.0, CYAN), StrokeKind::Inside);
        painter.galley(rect.min + pad, g, CYAN);
    } else {
        let color = if response.hovered() { CYAN } else { MUTED };
        painter.galley(rect.min + pad, g, color);
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    response
}

/// The header's segmented "folders done" bar.
fn node_bar(painter: &Painter, rect: Rect, done: u64, total: u64) {
    let segments = total.clamp(1, 24) as usize;
    let filled = if total == 0 { 0 } else { (done as f64 / total as f64 * segments as f64).floor() as usize };
    let gap = 3.0;
    let w = (rect.width() - gap * (segments - 1) as f32) / segments as f32;
    for i in 0..segments {
        let r = Rect::from_min_size(pos2(rect.left() + i as f32 * (w + gap), rect.top()), vec2(w, rect.height()));
        if i < filled {
            theme::glow(painter, r, CYAN, 0.4, 8.0);
            painter.rect_filled(r, 0.0, CYAN);
        } else if i == filled && done < total {
            painter.rect(r, 0.0, CYAN_DEEP, Stroke::new(1.0, CYAN), StrokeKind::Inside);
        } else {
            painter.rect_filled(r, 0.0, TRACK);
        }
    }
}

/// The status bar's magenta disk usage meter.
fn usage_bar(painter: &Painter, rect: Rect, used: f32) {
    painter.rect_filled(rect, 0.0, TRACK);
    let fill = Rect::from_min_size(rect.min, vec2(rect.width() * used.clamp(0.0, 1.0), rect.height()));
    theme::glow(painter, fill, MAGENTA, 0.45, 10.0);
    painter.rect_filled(fill, 0.0, MAGENTA);
}

/// The start screen's capacity ring, drawn on a 340-unit design grid.
fn capacity_ring(painter: &Painter, rect: Rect, used: f32) {
    let s = rect.width() / 340.0;
    let c = rect.center();
    let tau = std::f32::consts::TAU;
    painter.circle_stroke(c, 156.0 * s, Stroke::new(1.0, BORDER));
    painter.circle_stroke(c, 124.0 * s, Stroke::new(20.0 * s, TRACK));
    let arc = theme::arc(c, 124.0 * s, 0.0, tau * used.clamp(0.0, 1.0));
    painter.add(Shape::line(arc.clone(), Stroke::new(30.0 * s, theme::glow_color(MAGENTA, 0.18))));
    painter.add(Shape::line(arc, Stroke::new(20.0 * s, MAGENTA)));
    // Notches cut the ring into segments, like the design.
    let notches = 80;
    for i in 0..notches {
        let a = tau * i as f32 / notches as f32 - std::f32::consts::FRAC_PI_2;
        let dir = egui::Vec2::angled(a);
        painter.line_segment([c + dir * 112.0 * s, c + dir * 136.0 * s], Stroke::new(2.5 * s, BG));
    }
    let inner = theme::arc(c, 94.0 * s, 0.0, tau);
    painter.extend(Shape::dashed_line(&inner, Stroke::new(1.0, theme::glow_color(CYAN, 0.7)), 2.0 * s, 8.0 * s));
    for a in [0.0f32, 90.0, 180.0, 270.0] {
        let dir = egui::Vec2::angled(a.to_radians());
        painter.line_segment([c + dir * 146.0 * s, c + dir * 164.0 * s], Stroke::new(1.5, CYAN));
    }
    theme::text(painter, c + vec2(0.0, -8.0 * s), Align2::CENTER_CENTER, format!("{:.0}%", used * 100.0), mono_medium(50.0 * s), TEXT, 0.0);
    theme::text(painter, c + vec2(0.0, 30.0 * s), Align2::CENTER_CENTER, "CAPACITY USED", mono(12.0 * s.max(0.85)), MAGENTA, 3.0);
}

/// One clickable row in the side list.
#[allow(clippy::too_many_arguments)]
fn item_row(
    ui: &mut Ui,
    tree: &Node,
    path: &[usize],
    node: &Node,
    parent_size: u64,
    detail: &str,
    selected: Option<&Vec<usize>>,
    scanning: bool,
    ai_ready: bool,
    actions: &mut Vec<Action>,
) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, ROW_H - 2.0), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, format!("{}, {}", node.name, format_size(node.size))));
    let painter = ui.painter_at(rect.expand(12.0));
    let is_selected = selected.is_some_and(|s| s.as_slice() == path);
    if is_selected {
        theme::glow(&painter, rect, MAGENTA, 0.2, 14.0);
        painter.rect(rect, 0.0, MAGENTA_DEEP, Stroke::new(1.0, MAGENTA), StrokeKind::Inside);
    } else if response.hovered() {
        painter.rect_filled(rect, 0.0, RAISED);
    }
    let st = theme::kind_style(node.kind);
    let swatch_color = if is_selected {
        MAGENTA
    } else if node.is_dir && node.kind != Kind::App {
        GREY
    } else {
        st.edge
    };
    let top = rect.top() + 8.0;
    painter.rect_filled(Rect::from_min_size(pos2(rect.left() + 10.0, top + 5.0), vec2(8.0, 8.0)), 0.0, swatch_color);

    let size_text = format_size(node.size);
    let size_color = if is_selected { MAGENTA_SOFT } else { TEXT };
    let size_rect = theme::text(&painter, pos2(rect.right() - 10.0, top + 9.0), Align2::RIGHT_CENTER, size_text.clone(), mono(13.0), size_color, 0.0);
    let name_clip = Rect::from_min_max(pos2(rect.left() + 26.0, rect.top()), pos2(size_rect.left() - 8.0, rect.bottom()));
    let name_font = if is_selected { body_semibold(14.0) } else { body_medium(14.0) };
    let name = theme::galley(&painter, node.name.to_string(), name_font, 0.0, f32::INFINITY);
    let name_w = name.size().x;
    painter.with_clip_rect(name_clip).galley(pos2(name_clip.left(), top + 9.0 - name.size().y / 2.0), name, TEXT);
    if !detail.is_empty() && name_w + 12.0 < name_clip.width() {
        let d = Rect::from_min_max(pos2(name_clip.left() + name_w + 8.0, rect.top()), name_clip.max);
        theme::text(&painter.with_clip_rect(d), pos2(d.left(), top + 9.0), Align2::LEFT_CENTER, detail, mono(11.0), MUTED, 0.0);
    }

    let share = node.size as f32 / parent_size.max(1) as f32;
    let bar = Rect::from_min_size(pos2(rect.left() + 10.0, rect.bottom() - 9.0), vec2(width - 20.0, 3.0));
    painter.rect_filled(bar, 0.0, TRACK);
    let fill = Rect::from_min_size(bar.min, vec2(bar.width() * share, 3.0));
    painter.rect_filled(fill, 0.0, if is_selected { MAGENTA } else { GREY });

    if response.clicked() {
        actions.push(Action::Select(path.to_vec()));
    }
    if response.double_clicked() {
        actions.push(Action::ZoomTo(path.to_vec()));
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let response = response.on_hover_text(format!("{} · {:.1}% of this folder", size_text, share * 100.0));
    response.context_menu(|ui| context_menu_items(ui, tree, path, scanning, ai_ready, actions));
}

fn context_menu_items(ui: &mut Ui, tree: &Node, path: &[usize], scanning: bool, ai_ready: bool, actions: &mut Vec<Action>) {
    let Some(node) = tree.at(path) else { return };
    ui.set_min_width(200.0);
    ui.label(RichText::new(&*node.name).font(display(14.0)).color(TEXT));
    ui.label(RichText::new(format_size(node.size)).font(mono(12.0)).color(CYAN));
    ui.separator();
    let item = |ui: &mut Ui, label: &str| ui.button(RichText::new(label).font(body_medium(13.0)));
    if node.is_dir && item(ui, "Zoom In").clicked() {
        actions.push(Action::ZoomTo(path.to_vec()));
        ui.close();
    }
    if item(ui, "Reveal in Finder").clicked() {
        actions.push(Action::Reveal(path.to_vec()));
        ui.close();
    }
    if item(ui, "Open").clicked() {
        actions.push(Action::Open(path.to_vec()));
        ui.close();
    }
    if item(ui, "Copy Path").clicked() {
        actions.push(Action::CopyPath(path.to_vec()));
        ui.close();
    }
    let ask = ui.add_enabled(ai_ready && !scanning, egui::Button::new(RichText::new("Ask AI…").font(body_medium(13.0)).color(CYAN)));
    let ask = if scanning {
        ask.on_disabled_hover_text("Wait for the scan to finish")
    } else {
        ask.on_disabled_hover_text("Choose an AI agent in Settings")
    };
    if ask.clicked() {
        actions.push(Action::AskAi(path.to_vec(), false));
        ui.close();
    }
    ui.separator();
    let can_trash = !scanning && !path.is_empty();
    let label = RichText::new("Move to Trash…").font(body_medium(13.0)).color(MAGENTA);
    let trash = ui.add_enabled(can_trash, egui::Button::new(label));
    let trash = if scanning { trash.on_disabled_hover_text("Wait for the scan to finish") } else { trash };
    if trash.clicked() {
        actions.push(Action::AskTrash(path.to_vec()));
        ui.close();
    }
}

// ------------------------------------------------------------- helpers --

fn plural(n: u64, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
}

/// 1204388 → "1,204,388".
fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Path for display, with the home folder shown as `~`.
fn tilde(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME")
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() { "~".into() } else { format!("~/{}", rest.display()) };
    }
    path.display().to_string()
}

struct VolumeInfo {
    free: u64,
    total: u64,
    mount: PathBuf,
    fs_type: String,
}

impl VolumeInfo {
    fn used_fraction(&self) -> f32 {
        if self.total == 0 { 0.0 } else { 1.0 - self.free as f32 / self.total as f32 }
    }
}

/// Space, mount point and file system of the volume holding `path`.
fn volume_info(path: &Path) -> Option<VolumeInfo> {
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
fn startup_volume_name() -> String {
    std::fs::read_dir("/Volumes")
        .into_iter()
        .flatten()
        .flatten()
        .find(|e| std::fs::read_link(e.path()).is_ok_and(|t| t == Path::new("/")))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .unwrap_or_else(|| "Macintosh HD".to_string())
}

fn volume_display_name(mount: &Path, startup_name: &str) -> String {
    if mount == Path::new("/") || mount == Path::new("/System/Volumes/Data") {
        startup_name.to_string()
    } else {
        mount.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| mount.display().to_string())
    }
}

fn root_crumbs(root: &Path, startup_name: &str) -> RootCrumbs {
    let mount = volume_info(root).map(|v| v.mount).unwrap_or_else(|| PathBuf::from("/"));
    let volume = volume_display_name(&mount, startup_name);
    // Firmlinked paths like /Users live on the data volume but aren't under
    // its mount point, so fall back to the path from `/`.
    let rel = root.strip_prefix(&mount).or_else(|_| root.strip_prefix("/")).unwrap_or(root);
    let mut parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    match parts.pop() {
        Some(label) => {
            let mut above = vec![volume];
            above.extend(parts);
            RootCrumbs { above, label }
        }
        None => RootCrumbs { above: Vec::new(), label: volume },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_thousands() {
        assert_eq!(group(0), "0");
        assert_eq!(group(999), "999");
        assert_eq!(group(1_000), "1,000");
        assert_eq!(group(1_204_388), "1,204,388");
    }

    #[test]
    fn pluralises() {
        assert_eq!(plural(1, "item", "items"), "item");
        assert_eq!(plural(0, "item", "items"), "items");
        assert_eq!(plural(3, "item", "items"), "items");
    }
}
