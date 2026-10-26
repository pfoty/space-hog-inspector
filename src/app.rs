//! The Space Hog Inspector window: toolbar, treemap, item list and status bar.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Id, Key, Layout, Painter, Pos2, Rect,
    RichText, Sense, Stroke, StrokeKind, Ui, vec2,
};

use crate::scanner::{self, Kind, Node, Progress, ScanMsg, ScanOptions, format_size};
use crate::treemap::squarify;

const HEADER_H: f32 = 16.0;
const ROW_H: f32 = 22.0;
/// Smallest area, in square points, an item gets its own rectangle.
const MIN_ITEM_AREA: f32 = 12.0;
const FULL_DISK_ACCESS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";

#[derive(PartialEq, Eq, Clone, Copy)]
enum SideTab {
    Contents,
    LargestFiles,
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
}

struct ActiveScan {
    rx: Receiver<ScanMsg>,
    progress: Arc<Progress>,
    started: Instant,
}

struct TrashRequest {
    path: Vec<usize>,
    fs_path: PathBuf,
    name: String,
    size: u64,
    is_dir: bool,
}

struct LargestCache {
    view: Vec<usize>,
    generation: u64,
    /// (absolute index path, name, size, parent folder relative to view)
    rows: Vec<(Vec<usize>, Box<str>, u64, String)>,
}

pub struct SpaceHogApp {
    root_path: Option<PathBuf>,
    tree: Option<Node>,
    /// Index path from the tree root to the folder filling the treemap.
    view: Vec<usize>,
    selected: Option<Vec<usize>>,
    scan: Option<ActiveScan>,
    /// Bumped whenever the tree changes, to invalidate caches.
    generation: u64,
    max_depth: usize,
    cross_volumes: bool,
    filter: String,
    side_tab: SideTab,
    largest: Option<LargestCache>,
    trash_request: Option<TrashRequest>,
    message: Option<(String, bool)>,
    last_scan: Option<(Duration, bool)>,
    error_count: u64,
    error_paths: Vec<(PathBuf, String)>,
    show_errors: bool,
}

impl SpaceHogApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        let mut app = SpaceHogApp {
            root_path: None,
            tree: None,
            view: Vec::new(),
            selected: None,
            scan: None,
            generation: 0,
            max_depth: 6,
            cross_volumes: false,
            filter: String::new(),
            side_tab: SideTab::Contents,
            largest: None,
            trash_request: None,
            message: None,
            last_scan: None,
            error_count: 0,
            error_paths: Vec::new(),
            show_errors: false,
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
        let options = ScanOptions { cross_volumes: self.cross_volumes };
        let thread_progress = progress.clone();
        let thread_path = path.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            scanner::scan(&thread_path, options, &thread_progress, &tx);
            repaint.request_repaint();
        });
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
        self.scan = Some(ActiveScan { rx, progress, started: Instant::now() });
    }

    fn stop_scan(&mut self) {
        if let Some(scan) = &self.scan {
            scan.progress.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Pulls finished entries from the scan thread into the tree.
    fn poll_scan(&mut self, ctx: &egui::Context) {
        let Some(scan) = &self.scan else { return };
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
                        fs_path,
                        path,
                    });
                }
            }
        }
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
        self.message = Some((
            format!(
                "Moved {} to the Trash. Its {} is freed when you empty the Trash.",
                request.name,
                format_size(request.size)
            ),
            false,
        ));
    }

    // ---------------------------------------------------------------- UI --

    fn toolbar(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            if ui.button("Home").on_hover_text("Scan your home folder").clicked()
                && let Some(home) = std::env::var_os("HOME")
            {
                self.start_scan(PathBuf::from(home), &ctx);
            }
            if ui.button("Whole Disk").on_hover_text("Scan the startup disk (/)").clicked() {
                self.start_scan(PathBuf::from("/"), &ctx);
            }
            if ui.button("Choose Folder…").on_hover_text("Cmd+O").clicked() {
                self.choose_folder(&ctx);
            }
            let can_rescan = self.root_path.is_some() && !self.scanning();
            if ui.add_enabled(can_rescan, egui::Button::new("Rescan")).on_hover_text("Cmd+R").clicked()
                && let Some(p) = self.root_path.clone()
            {
                self.start_scan(p, &ctx);
            }
            if self.scanning() && ui.button("Stop").clicked() {
                self.stop_scan();
            }
            ui.separator();
            ui.checkbox(&mut self.cross_volumes, "Include other volumes")
                .on_hover_text("Also descend into external drives and network shares mounted inside the scanned folder. Applies to the next scan.");
            ui.separator();
            ui.add(egui::Slider::new(&mut self.max_depth, 1..=12).text("levels shown"));
        });

        let Some(tree) = &self.tree else { return };
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(!self.view.is_empty(), egui::Button::new("⬆ Up")).clicked() {
                actions.push(Action::Up);
            }
            let mut node = tree;
            for depth in 0..=self.view.len() {
                if depth > 0 {
                    ui.label(RichText::new("›").weak());
                    node = &node.children[self.view[depth - 1]];
                }
                let current = depth == self.view.len();
                let text = RichText::new(&*node.name);
                let text = if current { text.strong() } else { text };
                if ui.add(egui::Button::new(text).frame(false)).clicked() && !current {
                    actions.push(Action::ZoomTo(self.view[..depth].to_vec()));
                }
            }
            ui.label(RichText::new(format_size(node.size)).weak());
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if let Some(scan) = &self.scan {
                ui.spinner();
                let p = &scan.progress;
                let current = p.current.lock().map(|c| c.display().to_string()).unwrap_or_default();
                ui.label(format!(
                    "Scanning… {} files in {} folders, {} so far ({:.0}s)",
                    p.files.load(Ordering::Relaxed),
                    p.dirs.load(Ordering::Relaxed),
                    format_size(p.bytes.load(Ordering::Relaxed)),
                    scan.started.elapsed().as_secs_f32(),
                ));
                ui.add(egui::Label::new(RichText::new(current).weak()).truncate());
                return;
            }
            if let Some((message, is_error)) = &self.message {
                let text = RichText::new(message);
                ui.label(if *is_error { text.color(ui.visuals().error_fg_color) } else { text });
                if ui.small_button("Dismiss").clicked() {
                    self.message = None;
                }
                ui.separator();
            }
            if let (Some(tree), Some((elapsed, cancelled))) = (&self.tree, self.last_scan) {
                ui.label(format!(
                    "{} in {} files · scanned in {:.1}s{}",
                    format_size(tree.size),
                    tree.files,
                    elapsed.as_secs_f32(),
                    if cancelled { " (stopped early, totals incomplete)" } else { "" },
                ));
                if self.error_count > 0 {
                    ui.separator();
                    let link = ui.link(format!("{} items couldn't be read", self.error_count));
                    if link.clicked() {
                        self.show_errors = true;
                    }
                }
            }
            if let Some(root) = &self.root_path
                && let Some((free, total)) = volume_space(root)
            {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(format!("Disk: {} free of {}", format_size(free), format_size(total)));
                });
            }
        });
    }

    fn side_panel(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.side_tab, SideTab::Contents, "Contents");
            ui.selectable_value(&mut self.side_tab, SideTab::LargestFiles, "Largest Files");
        });
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter by name").desired_width(f32::INFINITY));
        egui::CollapsingHeader::new("Colour key").default_open(false).show(ui, |ui| {
            for kind in Kind::LEGEND {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                    ui.painter().rect_filled(r, 2.0, kind_color(kind));
                    ui.label(kind.label());
                });
            }
        });
        ui.separator();

        let Some(tree) = &self.tree else {
            ui.label(RichText::new("Nothing scanned yet.").weak());
            return;
        };
        let Some(view_node) = tree.at(&self.view) else { return };
        let filter = self.filter.to_lowercase();
        let matches = |name: &str| filter.is_empty() || name.to_lowercase().contains(&filter);

        match self.side_tab {
            SideTab::Contents => {
                let rows: Vec<usize> =
                    (0..view_node.children.len()).filter(|&i| matches(&view_node.children[i].name)).collect();
                if rows.is_empty() {
                    ui.label(RichText::new("No items.").weak());
                }
                egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, ROW_H, rows.len(), |ui, range| {
                    for &i in &rows[range] {
                        let child = &view_node.children[i];
                        let mut path = self.view.clone();
                        path.push(i);
                        let detail = if child.is_dir { format!("{} files", child.files) } else { String::new() };
                        item_row(ui, tree, &path, child, view_node.size, &detail, self.selected.as_ref(), self.scanning(), actions);
                    }
                });
            }
            SideTab::LargestFiles => {
                let stale = self.largest.as_ref().is_none_or(|c| c.view != self.view || c.generation != self.generation);
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
                    ui.label(RichText::new("No files.").weak());
                }
                egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, ROW_H, rows.len(), |ui, range| {
                    for (path, _, _, parent) in &rows[range] {
                        if let Some(node) = tree.at(path) {
                            let detail = if parent.is_empty() { "./".to_string() } else { format!("{parent}/") };
                            item_row(ui, tree, path, node, view_node.size, &detail, self.selected.as_ref(), self.scanning(), actions);
                        }
                    }
                });
            }
        }
    }

    fn welcome(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.25);
            ui.heading(RichText::new("Space Hog Inspector").size(30.0).strong());
            ui.add_space(6.0);
            ui.label("Find out what is eating your disk space. Pick something to scan:");
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                let width = 3.0 * 150.0 + 2.0 * ui.spacing().item_spacing.x;
                ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                let size = vec2(150.0, 36.0);
                if ui.add_sized(size, egui::Button::new("Home Folder")).clicked()
                    && let Some(home) = std::env::var_os("HOME")
                {
                    self.start_scan(PathBuf::from(home), &ctx);
                }
                if ui.add_sized(size, egui::Button::new("Whole Disk")).clicked() {
                    self.start_scan(PathBuf::from("/"), &ctx);
                }
                if ui.add_sized(size, egui::Button::new("Choose Folder…")).clicked() {
                    self.choose_folder(&ctx);
                }
            });
            ui.add_space(24.0);
            ui.label(RichText::new(
                "To see inside protected folders (Mail, Messages, Photos, other users), give this app Full Disk Access.",
            ).weak());
            if ui.link("Open Full Disk Access settings").clicked() {
                self.run_open(Command::new("open").arg(FULL_DISK_ACCESS_URL));
            }
        });
    }

    fn treemap(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click());
        let Some(tree) = &self.tree else { return };
        let Some(view_node) = tree.at(&self.view) else {
            self.view.clear();
            return;
        };
        let bounds = response.rect;
        painter.rect_filled(bounds, 0.0, Color32::from_gray(24));
        if view_node.size == 0 {
            let msg = if self.scanning() { "Scanning…" } else { "This folder is empty or couldn't be read." };
            painter.text(bounds.center(), Align2::CENTER_CENTER, msg, FontId::proportional(16.0), Color32::GRAY);
            return;
        }

        let mut draw = TreemapDraw {
            painter: &painter,
            pointer: response.hover_pos(),
            max_depth: self.max_depth,
            hit: None,
            selected: self.selected.as_deref(),
            selected_rect: None,
        };
        let mut path = self.view.clone();
        draw.node(view_node, bounds, 0, &mut path);
        let (hit, selected_rect) = (draw.hit.take(), draw.selected_rect);

        if let Some(r) = selected_rect {
            painter.rect_stroke(r, 0.0, Stroke::new(2.5, Color32::WHITE), StrokeKind::Inside);
        }
        if let Some((_, r)) = &hit {
            painter.rect_stroke(*r, 0.0, Stroke::new(1.5, Color32::from_rgb(255, 230, 120)), StrokeKind::Inside);
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
        if response.secondary_clicked()
            && let Some((p, _)) = &hit
        {
            ui.ctx().data_mut(|d| d.insert_temp(Id::unique("context_target"), p.clone()));
            actions.push(Action::Select(p.clone()));
        }

        let scanning = self.scanning();
        let view_size = view_node.size;
        let response = if let Some((p, _)) = &hit
            && let Some(node) = tree.at(p)
        {
            let full = self.fs_path(p).map(|f| f.display().to_string()).unwrap_or_default();
            response.on_hover_ui_at_pointer(|ui| {
                ui.label(RichText::new(&*node.name).strong());
                ui.label(RichText::new(full).weak());
                ui.label(format!(
                    "{}  ·  {:.1}% of this view",
                    format_size(node.size),
                    100.0 * node.size as f64 / view_size.max(1) as f64
                ));
                if node.is_dir {
                    ui.label(format!("{} files · {} items inside", node.files, node.children.len()));
                } else {
                    ui.label(node.kind.label());
                }
                ui.label(RichText::new("Double-click to zoom · right-click for actions").weak().small());
            })
        } else {
            response
        };

        let target: Option<Vec<usize>> = ui.ctx().data(|d| d.get_temp(Id::unique("context_target")));
        response.context_menu(|ui| {
            if let Some(target) = &target {
                context_menu_items(ui, tree, target, scanning, actions);
            }
        });
    }
}

impl eframe::App for SpaceHogApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_scan(&ctx);
        let mut actions = Vec::new();

        if !ctx.egui_wants_keyboard_input() {
            let (up, open, rescan) = ctx.input(|i| {
                (
                    // Not Escape: that closes menus and dialogs.
                    i.key_pressed(Key::Backspace) || (i.modifiers.command && i.key_pressed(Key::ArrowUp)),
                    i.modifiers.command && i.key_pressed(Key::O),
                    i.modifiers.command && i.key_pressed(Key::R),
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
        }

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            self.toolbar(ui, &mut actions);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(2.0);
            self.status_bar(ui);
            ui.add_space(2.0);
        });
        if self.tree.is_some() {
            egui::Panel::right("side").resizable(true).default_size(340.0).min_size(220.0).show(ui, |ui| {
                self.side_panel(ui, &mut actions);
            });
        }
        if self.tree.is_some() {
            egui::CentralPanel::no_frame().show(ui, |ui| self.treemap(ui, &mut actions));
        } else {
            egui::CentralPanel::default().show(ui, |ui| self.welcome(ui));
        }

        for action in actions {
            self.apply(action, &ctx);
        }

        if let Some(request) = &self.trash_request {
            let mut confirmed = false;
            let mut cancelled = false;
            let modal = egui::Modal::new(Id::unique("trash_modal")).show(&ctx, |ui| {
                ui.set_max_width(420.0);
                ui.heading("Move to Trash?");
                ui.add_space(6.0);
                ui.label(format!(
                    "Move the {} \"{}\" ({}) to the Trash?",
                    if request.is_dir { "folder" } else { "file" },
                    request.name,
                    format_size(request.size)
                ));
                ui.label(RichText::new(request.fs_path.display().to_string()).weak());
                ui.add_space(4.0);
                ui.label("You can put it back from the Trash in Finder until the Trash is emptied.");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Move to Trash").color(ui.visuals().error_fg_color)).clicked() {
                        confirmed = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancelled = true;
                    }
                });
            });
            if confirmed {
                let request = self.trash_request.take().unwrap();
                self.move_to_trash(request);
            } else if cancelled || modal.should_close() {
                self.trash_request = None;
            }
        }

        if self.show_errors {
            let mut open = true;
            egui::Window::new("Items that couldn't be read").open(&mut open).default_size([560.0, 360.0]).show(&ctx, |ui| {
                ui.label(format!(
                    "{} items were skipped, usually because macOS privacy protection blocked access. \
                     Their space is not included in the totals.{}",
                    self.error_count,
                    if self.error_count as usize > self.error_paths.len() {
                        format!(" The first {} are listed.", self.error_paths.len())
                    } else {
                        String::new()
                    }
                ));
                if ui.link("Open Full Disk Access settings").clicked() {
                    let _ = Command::new("open").arg(FULL_DISK_ACCESS_URL).status();
                }
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    for (path, err) in &self.error_paths {
                        ui.horizontal(|ui| {
                            ui.label(path.display().to_string());
                            ui.label(RichText::new(err).weak());
                        });
                    }
                });
            });
            self.show_errors = open;
        }
    }
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
        if node.is_dir {
            self.folder(node, rect, depth, path);
        } else {
            self.file(node, rect);
        }
    }

    fn folder(&mut self, node: &Node, rect: Rect, depth: usize, path: &mut Vec<usize>) {
        let frame = folder_color(node.kind, depth);
        self.painter.rect(rect, 0.0, frame, Stroke::new(1.0, Color32::from_gray(15)), StrokeKind::Inside);

        let has_header = rect.width() >= 36.0 && rect.height() >= HEADER_H + 10.0;
        let inner = if has_header {
            let header = Rect::from_min_size(rect.min, vec2(rect.width(), HEADER_H));
            label(self.painter, header, &format!("{}  {}", node.name, format_size(node.size)), Color32::from_gray(235), 11.5);
            Rect::from_min_max(rect.min + vec2(2.0, HEADER_H), rect.max - vec2(2.0, 2.0))
        } else {
            rect.shrink(1.0)
        };
        if inner.width() < 2.0 || inner.height() < 2.0 || node.children.is_empty() {
            return;
        }
        if depth + 1 >= self.max_depth {
            // Depth limit reached: show the contents as one solid block.
            self.painter.rect_filled(inner, 0.0, frame.gamma_multiply(0.75));
            if has_header {
                label(self.painter, inner, &format!("{} items", node.children.len()), Color32::from_gray(190), 11.0);
            }
            return;
        }
        let sizes: Vec<u64> = node.children.iter().map(|c| c.size).collect();
        let layout = squarify(&sizes, inner, MIN_ITEM_AREA);
        for (i, r) in layout.rects.iter().enumerate() {
            path.push(i);
            self.node(&node.children[i], *r, depth + 1, path);
            path.pop();
        }
        if let Some(rest) = layout.rest {
            let count = node.children.len() - layout.rects.len();
            self.painter.rect(rest, 0.0, Color32::from_gray(70), Stroke::new(1.0, Color32::from_gray(15)), StrokeKind::Inside);
            label(self.painter, rest, &format!("{count} small items"), Color32::from_gray(200), 10.5);
        }
    }

    fn file(&mut self, node: &Node, rect: Rect) {
        let base = kind_color(node.kind);
        self.painter.rect(rect, 0.0, base, Stroke::new(1.0, Color32::from_black_alpha(140)), StrokeKind::Inside);
        // A light band along the top gives each block a little depth.
        if rect.height() > 6.0 {
            let band = Rect::from_min_size(rect.min, vec2(rect.width(), (rect.height() * 0.35).min(14.0)));
            self.painter.rect_filled(band.shrink(1.0), 0.0, Color32::from_white_alpha(28));
        }
        if rect.width() >= 40.0 && rect.height() >= 14.0 {
            let text_color = Color32::from_black_alpha(230);
            if rect.height() >= 30.0 {
                label(self.painter, rect, &node.name, text_color, 11.0);
                let lower = Rect::from_min_max(rect.min + vec2(0.0, 13.0), rect.max);
                label(self.painter, lower, &format_size(node.size), text_color, 10.5);
            } else {
                label(self.painter, rect, &format!("{} {}", node.name, format_size(node.size)), text_color, 10.5);
            }
        }
    }
}

fn label(painter: &Painter, rect: Rect, text: &str, color: Color32, size: f32) {
    painter.with_clip_rect(rect.shrink(1.0)).text(
        rect.min + vec2(4.0, 2.0),
        Align2::LEFT_TOP,
        text,
        FontId::proportional(size),
        color,
    );
}

fn kind_color(kind: Kind) -> Color32 {
    match kind {
        Kind::Folder => Color32::from_rgb(92, 104, 128),
        Kind::App => Color32::from_rgb(178, 132, 222),
        Kind::Video => Color32::from_rgb(232, 104, 104),
        Kind::Audio => Color32::from_rgb(240, 160, 72),
        Kind::Image => Color32::from_rgb(110, 196, 124),
        Kind::Archive => Color32::from_rgb(222, 198, 84),
        Kind::Document => Color32::from_rgb(96, 164, 232),
        Kind::Code => Color32::from_rgb(84, 200, 196),
        Kind::System => Color32::from_rgb(166, 168, 180),
        Kind::Other => Color32::from_rgb(196, 166, 136),
    }
}

/// Folder frames get lighter with depth so nesting is easy to follow.
fn folder_color(kind: Kind, depth: usize) -> Color32 {
    let step = (depth.min(8) as f32) * 9.0;
    let (r, g, b) = if kind == Kind::App { (92.0, 64.0, 120.0) } else { (52.0, 60.0, 78.0) };
    Color32::from_rgb((r + step) as u8, (g + step) as u8, (b + step * 1.1) as u8)
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
    actions: &mut Vec<Action>,
) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::click());
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    let is_selected = selected.is_some_and(|s| s.as_slice() == path);
    if is_selected {
        painter.rect_filled(rect, CornerRadius::same(3), visuals.selection.bg_fill);
    } else if response.hovered() {
        painter.rect_filled(rect, CornerRadius::same(3), visuals.widgets.hovered.weak_bg_fill);
    }
    // Share-of-parent bar along the bottom edge.
    let share = node.size as f32 / parent_size.max(1) as f32;
    let bar = Rect::from_min_size(Pos2::new(rect.min.x + 22.0, rect.max.y - 3.0), vec2((width - 26.0) * share, 2.0));
    painter.rect_filled(bar, 0.0, kind_color(node.kind).gamma_multiply(0.8));

    let swatch = Rect::from_center_size(Pos2::new(rect.min.x + 10.0, rect.center().y - 1.0), vec2(12.0, 12.0));
    let swatch_color = if node.is_dir { folder_color(node.kind, 2) } else { kind_color(node.kind) };
    painter.rect_filled(swatch, 2.0, swatch_color);

    let text_color = visuals.text_color();
    let size_text = format_size(node.size);
    let size_w = 70.0;
    painter.text(
        Pos2::new(rect.max.x - 4.0, rect.center().y - 1.0),
        Align2::RIGHT_CENTER,
        &size_text,
        FontId::proportional(12.0),
        text_color,
    );
    let name_rect = Rect::from_min_max(Pos2::new(rect.min.x + 22.0, rect.min.y), Pos2::new(rect.max.x - size_w, rect.max.y));
    let name = if node.is_dir { format!("{}/", node.name) } else { node.name.to_string() };
    let galley = painter.layout_no_wrap(name, FontId::proportional(12.5), text_color);
    let name_w = galley.size().x;
    painter.with_clip_rect(name_rect).galley(Pos2::new(name_rect.min.x, rect.center().y - galley.size().y / 2.0 - 1.0), galley, text_color);
    if !detail.is_empty() && name_w + 12.0 < name_rect.width() {
        let detail_rect = Rect::from_min_max(Pos2::new(name_rect.min.x + name_w + 8.0, rect.min.y), name_rect.max);
        painter.with_clip_rect(detail_rect).text(
            Pos2::new(detail_rect.min.x, rect.center().y - 1.0),
            Align2::LEFT_CENTER,
            detail,
            FontId::proportional(11.0),
            visuals.weak_text_color(),
        );
    }

    if response.clicked() {
        actions.push(Action::Select(path.to_vec()));
    }
    if response.double_clicked() {
        actions.push(Action::ZoomTo(path.to_vec()));
    }
    let response = response.on_hover_text(format!("{} · {:.1}% of this folder", size_text, share * 100.0));
    response.context_menu(|ui| context_menu_items(ui, tree, path, scanning, actions));
}

fn context_menu_items(ui: &mut Ui, tree: &Node, path: &[usize], scanning: bool, actions: &mut Vec<Action>) {
    let Some(node) = tree.at(path) else { return };
    ui.label(RichText::new(&*node.name).strong());
    ui.label(RichText::new(format_size(node.size)).weak());
    ui.separator();
    if node.is_dir && ui.button("Zoom In").clicked() {
        actions.push(Action::ZoomTo(path.to_vec()));
        ui.close();
    }
    if ui.button("Reveal in Finder").clicked() {
        actions.push(Action::Reveal(path.to_vec()));
        ui.close();
    }
    if ui.button("Open").clicked() {
        actions.push(Action::Open(path.to_vec()));
        ui.close();
    }
    if ui.button("Copy Path").clicked() {
        actions.push(Action::CopyPath(path.to_vec()));
        ui.close();
    }
    ui.separator();
    let can_trash = !scanning && !path.is_empty();
    let trash = ui.add_enabled(can_trash, egui::Button::new("Move to Trash…"));
    let trash = if scanning { trash.on_disabled_hover_text("Wait for the scan to finish") } else { trash };
    if trash.clicked() {
        actions.push(Action::AskTrash(path.to_vec()));
        ui.close();
    }
}

/// Free and total bytes of the volume holding `path`.
fn volume_space(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: c_path is a valid NUL-terminated string and stat is a valid out-pointer.
    if unsafe { libc::statfs(c_path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let block = stat.f_bsize as u64;
    Some((stat.f_bavail * block, stat.f_blocks * block))
}
