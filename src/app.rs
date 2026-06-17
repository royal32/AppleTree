use std::collections::BTreeSet;
use std::ffi::{CStr, CString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::model::color::ColorMap;
use crate::model::tree::{FileTree, NodeId};
use crate::settings::{AppPrefs, SplitOrientation};
use crate::ui::file_icons::FileIconCache;
use crate::ui::{self, NodeCommand};
use crate::{format_compact_count, format_size};

pub struct App {
    state: AppState,
    prefs: AppPrefs,
    scope: ScanScopeState,
    prefs_changed: bool,
    #[cfg(target_os = "macos")]
    about_configured: bool,
}

enum AppState {
    WaitingForPicker {
        frames: u8,
    },
    Scanning {
        paths: Vec<PathBuf>,
        start_time: Instant,
        receiver: Receiver<ScanResult>,
        worker: Option<JoinHandle<()>>,
        previous: Option<Box<LoadedState>>,
    },
    ScanFailed {
        paths: Vec<PathBuf>,
        message: String,
    },
    Loaded(Box<LoadedState>),
}

type ScanResult = Result<ScanCompletion, String>;

struct ScanCompletion {
    tree: FileTree,
    color_map: ColorMap,
    scan_time_ms: f64,
}

struct LoadedState {
    tree: FileTree,
    color_map: ColorMap,
    pane: ui::PaneState,
    expanded: BTreeSet<NodeId>,
    table: ui::tree_view::TableState,
    deleted: ui::DeletionOverlay,
    treemap: ui::treemap_view::TreemapState,
    status_message: Option<String>,
    scan_time_ms: f64,
    pending_scan: Option<Vec<PathBuf>>,
    file_icons: FileIconCache,
    memory_relief: MemoryRelief,
}

struct ScanScopeState {
    items: Vec<ScopeItem>,
}

struct ScopeItem {
    label: String,
    path: PathBuf,
    checked: bool,
    custom: bool,
}

#[derive(Default)]
struct ScopeSpace {
    total: u64,
    used: u64,
    free: u64,
}

const SCOPE_PANEL_WIDTH: f32 = 280.0;
const SCOPE_PANEL_GAP: f32 = 8.0;

struct MemoryRelief {
    until: Instant,
    next: Instant,
}

impl MemoryRelief {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            until: now + Duration::from_secs(120),
            next: now,
        }
    }

    fn restart(&mut self) {
        *self = Self::new();
    }

    fn run(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        if now >= self.until {
            return;
        }
        if now >= self.next {
            crate::memory::pressure_relief();
            self.next = now + Duration::from_secs(1);
        }
        ctx.request_repaint_after(Duration::from_secs(1));
    }
}

impl ScanScopeState {
    fn new(initial_path: Option<&Path>) -> Self {
        let mut state = Self { items: Vec::new() };
        state.add_standard("Macintosh HD", PathBuf::from("/"));

        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            state.add_standard("Home", home.clone());
            state.add_standard("Downloads", home.join("Downloads"));
            state.add_standard("Documents", home.join("Documents"));
            state.add_standard("Desktop", home.join("Desktop"));
        }

        state.add_standard("Applications", PathBuf::from("/Applications"));
        state.add_standard("Users", PathBuf::from("/Users"));
        state.add_mounted_volumes();

        if let Some(path) = initial_path {
            state.add_custom_checked(path);
        }

        state
    }

    fn add_standard(&mut self, label: impl Into<String>, path: PathBuf) {
        if path.is_dir() {
            self.add_item(label.into(), path, false, false);
        }
    }

    fn add_mounted_volumes(&mut self) {
        let Ok(entries) = fs::read_dir("/Volumes") else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let label = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Volume")
                .to_owned();
            self.add_item(label, path, false, false);
        }
    }

    fn add_custom_checked(&mut self, path: &Path) {
        if let Some(item) = self.items.iter_mut().find(|item| item.path == path) {
            item.checked = true;
            return;
        }
        let label = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| path.display().to_string());
        self.add_item(label, path.to_path_buf(), true, true);
    }

    fn add_item(&mut self, label: String, path: PathBuf, checked: bool, custom: bool) {
        if self.items.iter().any(|item| item.path == path) {
            return;
        }
        self.items.push(ScopeItem {
            label,
            path,
            checked,
            custom,
        });
    }

    fn checked_paths(&self) -> Vec<PathBuf> {
        let mut seen = BTreeSet::new();
        let mut paths = Vec::new();
        for item in self.items.iter().filter(|item| item.checked) {
            if !item.path.is_dir() {
                continue;
            }
            let key = item.path.display().to_string();
            if seen.insert(key) {
                paths.push(item.path.clone());
            }
        }
        paths
    }

    fn remove_checked_custom(&mut self) {
        self.items.retain(|item| !(item.custom && item.checked));
    }

    fn has_checked_custom(&self) -> bool {
        self.items.iter().any(|item| item.custom && item.checked)
    }

    fn selected_space(&self) -> ScopeSpace {
        selected_scope_space(&self.checked_paths())
    }
}

fn selected_scope_space(paths: &[PathBuf]) -> ScopeSpace {
    let mut seen_mounts = BTreeSet::new();
    let mut space = ScopeSpace::default();

    for path in paths {
        let Some((mount, total, free)) = path_space(path) else {
            continue;
        };
        if !seen_mounts.insert(mount) {
            continue;
        }
        space.total = space.total.saturating_add(total);
        space.free = space.free.saturating_add(free);
    }
    space.used = space.total.saturating_sub(space.free);
    space
}

fn path_space(path: &Path) -> Option<(String, u64, u64)> {
    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statfs(c_path.as_ptr(), &mut stats) };
    if rc != 0 {
        return None;
    }

    let block_size = stats.f_bsize.max(0) as u64;
    let total = (stats.f_blocks as u64).saturating_mul(block_size);
    let free = (stats.f_bavail as u64).saturating_mul(block_size);
    let mount = unsafe { CStr::from_ptr(stats.f_mntonname.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    Some((mount, total, free))
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial_path: Option<String>) -> Self {
        #[cfg(target_os = "macos")]
        use_macos_system_font(&cc.egui_ctx);

        let initial_path_buf = initial_path.as_ref().map(PathBuf::from);
        let mut app = Self {
            state: AppState::WaitingForPicker { frames: 2 },
            prefs: AppPrefs::load(),
            scope: ScanScopeState::new(initial_path_buf.as_deref()),
            prefs_changed: false,
            #[cfg(target_os = "macos")]
            about_configured: false,
        };
        if let Some(path) = initial_path_buf {
            app.start_scan(path);
        }
        app
    }

    fn start_scan(&mut self, path: PathBuf) {
        self.start_scan_paths(vec![path]);
    }

    fn start_scan_paths(&mut self, mut paths: Vec<PathBuf>) {
        paths.retain(|path| path.is_dir());
        paths.sort();
        paths.dedup();
        if paths.is_empty() {
            return;
        }

        let previous = match std::mem::replace(
            &mut self.state,
            AppState::WaitingForPicker { frames: u8::MAX },
        ) {
            AppState::Loaded(loaded) => Some(loaded),
            AppState::Scanning { previous, .. } => previous,
            _ => None,
        };

        let start_time = Instant::now();
        let (sender, receiver) = mpsc::channel();
        let worker_paths = paths.clone();
        let worker_sender = sender.clone();
        let worker = match std::thread::Builder::new()
            .name("macdirstat-scan".to_owned())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let tree = FileTree::scan_paths(&worker_paths);
                    let color_map = ColorMap::from_extensions(&tree.extensions);
                    ScanCompletion {
                        tree,
                        color_map,
                        scan_time_ms: start_time.elapsed().as_secs_f64() * 1000.0,
                    }
                }))
                .map_err(|_| {
                    format!(
                        "Scan failed unexpectedly for {}",
                        scan_scope_display(&worker_paths)
                    )
                });
                let _ = worker_sender.send(result);
            }) {
            Ok(worker) => Some(worker),
            Err(e) => {
                let _ = sender.send(Err(format!("Failed to start scan: {e}")));
                None
            }
        };

        self.state = AppState::Scanning {
            paths,
            start_time,
            receiver,
            worker,
            previous,
        };
    }

    fn poll_scan(&mut self) {
        let mut scan_result = None;
        if let AppState::Scanning {
            paths,
            receiver,
            worker,
            previous,
            ..
        } = &mut self.state
        {
            scan_result = match receiver.try_recv() {
                Ok(result) => {
                    if let Some(worker) = worker.take() {
                        let _ = worker.join();
                    }
                    Some((result, paths.clone(), previous.take()))
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    if let Some(worker) = worker.take() {
                        let _ = worker.join();
                    }
                    Some((
                        Err("Scan worker stopped before returning a result".to_owned()),
                        paths.clone(),
                        previous.take(),
                    ))
                }
            };
        }

        if let Some((result, paths, previous)) = scan_result {
            match result {
                Ok(completion) => {
                    self.state = AppState::Loaded(Box::new(LoadedState::from_scan(completion)));
                }
                Err(message) => {
                    if let Some(mut loaded) = previous {
                        loaded.status_message = Some(message);
                        self.state = AppState::Loaded(loaded);
                    } else {
                        self.state = AppState::ScanFailed { paths, message };
                    }
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn use_macos_system_font(ctx: &egui::Context) {
    let Ok(bytes) = std::fs::read("/System/Library/Fonts/SFNS.ttf") else {
        return;
    };

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "SFNS".to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );

    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "SFNS".to_owned());

    ctx.set_fonts(fonts);
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Global blue selection highlight
        ctx.style_mut(|style| {
            style.visuals.selection.bg_fill = egui::Color32::from_rgb(56, 132, 244);
        });

        // Configure the native About panel text on the first frame.
        #[cfg(target_os = "macos")]
        if !self.about_configured {
            self.about_configured = true;
            configure_about_panel_text();
        }

        self.poll_scan();

        let mut immediate_scan_paths: Option<Vec<PathBuf>> = None;
        match &mut self.state {
            AppState::WaitingForPicker { frames } => {
                let mut scan_paths = None;
                show_empty_panes(ctx, &mut self.scope, &mut scan_paths, true);
                if let Some(paths) = scan_paths {
                    *frames = u8::MAX;
                    immediate_scan_paths = Some(paths);
                } else if *frames > 0 {
                    *frames -= 1;
                    ctx.request_repaint();
                } else if *frames == 0 {
                    // Prevent re-entry after the blocking dialog returns
                    *frames = u8::MAX;
                    let result = pick_folder_at_home();
                    if let Some(path) = result {
                        self.scope.add_custom_checked(&path);
                        immediate_scan_paths = Some(vec![path]);
                    } else {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                // frames == u8::MAX: dialog was dismissed, waiting for close
            }
            AppState::Scanning {
                paths,
                start_time,
                previous,
                ..
            } => {
                if let Some(previous) = previous {
                    previous.pane.hovered = None;
                    let _ = previous.show_disabled_panels(
                        ctx,
                        &mut self.prefs,
                        &mut self.scope,
                        &mut self.prefs_changed,
                    );
                    previous.memory_relief.run(ctx);
                } else {
                    let mut scan_paths = None;
                    show_empty_panes(ctx, &mut self.scope, &mut scan_paths, false);
                }
                show_scanning_overlay(ctx, paths, start_time.elapsed());
            }
            AppState::ScanFailed { paths, message } => {
                let mut retry_paths = None;
                show_scan_failed(ctx, paths, message, &mut retry_paths);
                if let Some(paths) = retry_paths {
                    self.start_scan_paths(paths);
                }
            }
            AppState::Loaded(loaded) => {
                loaded.pane.hovered = None;
                let mut command = handle_delete(loaded, ctx);
                if let Some(ui_command) = loaded.as_mut().show_panels(
                    ctx,
                    &mut self.prefs,
                    &mut self.scope,
                    &mut self.prefs_changed,
                ) {
                    command = Some(ui_command);
                }
                if let Some(command) = command {
                    execute_node_command(loaded, ctx, command);
                }
                loaded.memory_relief.run(ctx);
            }
        }

        if let Some(paths) = immediate_scan_paths {
            self.start_scan_paths(paths);
        }

        // Handle ⌘O and pending scans from breadcrumb menu (outside the match
        // to avoid borrow conflicts with self.state).
        if let AppState::Loaded(loaded) = &mut self.state {
            let cmd_o = ctx.input(|i| i.key_pressed(egui::Key::O) && i.modifiers.command);
            let paths = if cmd_o {
                pick_folder().map(|path| {
                    self.scope.add_custom_checked(&path);
                    vec![path]
                })
            } else {
                loaded.pending_scan.take()
            };
            if let Some(paths) = paths {
                for path in &paths {
                    self.scope.add_custom_checked(path);
                }
                self.start_scan_paths(paths);
            }
        }

        if self.prefs_changed {
            self.prefs.save();
            self.prefs_changed = false;
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.prefs.save();
    }
}

impl LoadedState {
    fn from_scan(completion: ScanCompletion) -> Self {
        let mut expanded = BTreeSet::new();
        expanded.insert(completion.tree.root.id);
        let treemap = ui::treemap_view::TreemapState::new(completion.tree.root.id);

        Self {
            tree: completion.tree,
            color_map: completion.color_map,
            pane: ui::PaneState::default(),
            expanded,
            table: ui::tree_view::TableState::default(),
            deleted: ui::DeletionOverlay::default(),
            treemap,
            status_message: None,
            scan_time_ms: completion.scan_time_ms,
            pending_scan: None,
            file_icons: FileIconCache::default(),
            memory_relief: MemoryRelief::new(),
        }
    }

    fn show_panels(
        &mut self,
        ctx: &egui::Context,
        prefs: &mut AppPrefs,
        scope: &mut ScanScopeState,
        prefs_changed: &mut bool,
    ) -> Option<NodeCommand> {
        self.show_panels_enabled(ctx, prefs, scope, prefs_changed, true)
    }

    fn show_disabled_panels(
        &mut self,
        ctx: &egui::Context,
        prefs: &mut AppPrefs,
        scope: &mut ScanScopeState,
        prefs_changed: &mut bool,
    ) -> Option<NodeCommand> {
        self.show_panels_enabled(ctx, prefs, scope, prefs_changed, false)
    }

    fn show_panels_enabled(
        &mut self,
        ctx: &egui::Context,
        prefs: &mut AppPrefs,
        scope: &mut ScanScopeState,
        prefs_changed: &mut bool,
        enabled: bool,
    ) -> Option<NodeCommand> {
        let mut command = None;
        self.show_status_bar(ctx, prefs, prefs_changed, &mut command, enabled);
        self.show_main_layout(ctx, prefs, scope, prefs_changed, &mut command, enabled);
        command
    }

    fn show_status_bar(
        &mut self,
        ctx: &egui::Context,
        prefs: &mut AppPrefs,
        prefs_changed: &mut bool,
        command: &mut Option<NodeCommand>,
        enabled: bool,
    ) {
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            if !enabled {
                ui.disable();
            }
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{} Files",
                    format_compact_count(self.tree.root.file_count)
                ));
                ui.separator();
                ui.label(format!(
                    "{} Scanned in {:.0}ms",
                    format_size(self.tree.root.size),
                    self.scan_time_ms,
                ));
                if let Some(path) = self.status_path() {
                    ui.separator();
                    ui.label(path);
                }
                if let Some(message) = &self.status_message {
                    ui.separator();
                    ui.colored_label(egui::Color32::from_rgb(220, 80, 80), message);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let has_selection = self.pane.selected.is_some();

                    let trash_text = egui::RichText::new("\u{1F5D1}").color(if has_selection {
                        egui::Color32::from_rgb(220, 60, 60)
                    } else {
                        egui::Color32::from_rgb(160, 120, 120)
                    });
                    let trash_btn = ui.add_enabled(has_selection, egui::Button::new(trash_text));
                    if trash_btn.clicked()
                        && let Some(id) = self.pane.selected
                    {
                        *command = Some(NodeCommand::Delete { id, confirm: true });
                    }

                    let reveal_btn = ui.add_enabled(
                        has_selection,
                        egui::Button::new("\u{1F50D} Reveal in Finder"),
                    );
                    if reveal_btn.clicked()
                        && let Some(id) = self.pane.selected
                    {
                        *command = Some(NodeCommand::Reveal(id));
                    }

                    ui.separator();
                    let before_split = prefs.split_orientation;
                    ui.selectable_value(
                        &mut prefs.split_orientation,
                        SplitOrientation::TopBottom,
                        "Top/Bottom",
                    );
                    ui.selectable_value(
                        &mut prefs.split_orientation,
                        SplitOrientation::LeftRight,
                        "Left/Right",
                    );
                    if prefs.split_orientation != before_split {
                        *prefs_changed = true;
                    }

                    ui.separator();
                    let mut label_depth = prefs.treemap_label_depth as u32;
                    let mut folder_depth = prefs.treemap_folder_depth as u32;
                    if ui
                        .add(egui::Slider::new(&mut label_depth, 0..=5).text("Labels"))
                        .changed()
                    {
                        prefs.treemap_label_depth = label_depth as usize;
                        *prefs_changed = true;
                    }
                    if ui
                        .add(egui::Slider::new(&mut folder_depth, 0..=6).text("Boxes"))
                        .changed()
                    {
                        prefs.treemap_folder_depth = folder_depth as usize;
                        *prefs_changed = true;
                        self.treemap.clear_layout();
                        self.memory_relief.restart();
                    }
                });
            });
        });
    }

    fn show_main_layout(
        &mut self,
        ctx: &egui::Context,
        prefs: &mut AppPrefs,
        scope: &mut ScanScopeState,
        prefs_changed: &mut bool,
        command: &mut Option<NodeCommand>,
        enabled: bool,
    ) {
        match prefs.split_orientation {
            SplitOrientation::LeftRight => {
                egui::SidePanel::left("file_table")
                    .default_width(820.0)
                    .min_width(640.0)
                    .show_separator_line(false)
                    .frame(
                        egui::Frame::side_top_panel(ctx.style().as_ref())
                            .inner_margin(egui::Margin::from(8)),
                    )
                    .show(ctx, |ui| {
                        if !enabled {
                            ui.disable();
                        }
                        if let Some(cmd) =
                            self.show_file_table_and_scope(ui, prefs, scope, prefs_changed)
                        {
                            *command = Some(cmd);
                        }
                    });

                egui::CentralPanel::default().show(ctx, |ui| {
                    if !enabled {
                        ui.disable();
                    }
                    self.show_breadcrumb_area(ui);
                    if let Some(cmd) = self.show_treemap(ui, prefs) {
                        *command = Some(cmd);
                    }
                });
            }
            SplitOrientation::TopBottom => {
                let table_response = egui::TopBottomPanel::top("file_table_top")
                    .default_height(prefs.top_bottom_table_height)
                    .min_height(180.0)
                    .resizable(enabled)
                    .frame(
                        egui::Frame::side_top_panel(ctx.style().as_ref())
                            .inner_margin(egui::Margin::from(8)),
                    )
                    .show(ctx, |ui| {
                        if !enabled {
                            ui.disable();
                        }
                        if let Some(cmd) =
                            self.show_file_table_and_scope(ui, prefs, scope, prefs_changed)
                        {
                            *command = Some(cmd);
                        }
                    });
                let new_height = table_response.response.rect.height();
                if (new_height - prefs.top_bottom_table_height).abs() > 1.0 {
                    prefs.top_bottom_table_height = new_height;
                    *prefs_changed = true;
                }

                egui::CentralPanel::default().show(ctx, |ui| {
                    if !enabled {
                        ui.disable();
                    }
                    self.show_breadcrumb_area(ui);
                    if let Some(cmd) = self.show_treemap(ui, prefs) {
                        *command = Some(cmd);
                    }
                });
            }
        }
    }

    fn show_file_table(
        &mut self,
        ui: &mut egui::Ui,
        prefs: &mut AppPrefs,
        prefs_changed: &mut bool,
    ) -> Option<NodeCommand> {
        ui::tree_view::show(
            ui,
            &self.tree,
            ui::tree_view::TreeViewState {
                pane: &mut self.pane,
                expanded: &mut self.expanded,
                deleted: &self.deleted,
                shrunk_treemap_nodes: &self.treemap.shrunk_nodes,
                file_icons: &mut self.file_icons,
                table: &mut self.table,
            },
            prefs,
            prefs_changed,
        )
    }

    fn show_file_table_and_scope(
        &mut self,
        ui: &mut egui::Ui,
        prefs: &mut AppPrefs,
        scope: &mut ScanScopeState,
        prefs_changed: &mut bool,
    ) -> Option<NodeCommand> {
        let mut command = None;
        let mut pending_scan = None;

        show_table_scope_row(
            ui,
            |ui| {
                if let Some(cmd) = self.show_file_table(ui, prefs, prefs_changed) {
                    command = Some(cmd);
                }
            },
            |ui| {
                pending_scan = show_scope_panel(ui, scope, true);
            },
        );

        if let Some(paths) = pending_scan {
            self.pending_scan = Some(paths);
        }

        command
    }

    fn show_treemap(&mut self, ui: &mut egui::Ui, prefs: &AppPrefs) -> Option<NodeCommand> {
        ui::treemap_view::show(
            ui,
            &self.tree,
            &mut self.pane,
            &self.color_map,
            &self.deleted,
            prefs,
            &mut self.treemap,
        )
    }

    fn show_breadcrumb_area(&mut self, ui: &mut egui::Ui) {
        let mut new_scan_path: Option<PathBuf> = None;
        self.show_breadcrumb(ui, &mut new_scan_path);
        ui.add_space(2.0);
        if let Some(path) = new_scan_path {
            self.pending_scan = Some(vec![path]);
        }
    }

    fn status_path(&self) -> Option<String> {
        self.pane
            .hovered
            .or(self.pane.selected)
            .and_then(|id| self.tree.full_display_path_for_id(id))
    }

    fn show_breadcrumb(&self, ui: &mut egui::Ui, new_scan_path: &mut Option<PathBuf>) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if self.tree.root.source_path.is_none() && self.tree.root_path.is_empty() {
                ui.label(egui::RichText::new("Scan Scope").size(14.0).strong());
                return;
            }
            let segments: Vec<&str> = self
                .tree
                .root_path
                .split('/')
                .filter(|s| !s.is_empty())
                .collect();

            ui.label(egui::RichText::new("\u{1F4BB}").size(13.0));
            let last_idx = segments.len().saturating_sub(1);
            if !segments.is_empty() {
                ui.label(egui::RichText::new("Macintosh HD").size(13.0));
                ui.label(
                    egui::RichText::new(" \u{203A} ")
                        .size(13.0)
                        .color(egui::Color32::GRAY),
                );
            }
            for (i, seg) in segments.iter().enumerate() {
                if i == last_idx {
                    let blue = egui::Color32::from_rgb(56, 132, 244);
                    let text = egui::RichText::new(*seg).size(14.0).strong().color(blue);
                    let resp = ui.add(egui::Label::new(text).sense(egui::Sense::click()));

                    let chevron_center =
                        egui::pos2(resp.rect.right() + 6.0, resp.rect.center().y + 1.0);
                    let s = 3.0;
                    ui.painter().add(egui::Shape::convex_polygon(
                        vec![
                            egui::pos2(chevron_center.x - s, chevron_center.y - s),
                            egui::pos2(chevron_center.x + s, chevron_center.y - s),
                            egui::pos2(chevron_center.x, chevron_center.y + s),
                        ],
                        blue,
                        egui::Stroke::NONE,
                    ));
                    ui.add_space(14.0);

                    let menu_id = resp.id.with("breadcrumb_menu");
                    if resp.clicked() {
                        ui.memory_mut(|m| m.toggle_popup(menu_id));
                    }
                    egui::popup_below_widget(
                        ui,
                        menu_id,
                        &resp,
                        egui::PopupCloseBehavior::CloseOnClick,
                        |ui| {
                            ui.set_min_width(200.0);
                            if ui.button("\u{1F4C2}  Open Folder\u{2026}").clicked()
                                && let Some(path) = pick_folder()
                            {
                                *new_scan_path = Some(path);
                            }
                            if segments.len() > 1 {
                                ui.separator();
                                let mut path = PathBuf::from("/");
                                for (j, ancestor) in segments[..last_idx].iter().enumerate() {
                                    path.push(ancestor);
                                    let indent = "  ".repeat(j);
                                    let label = format!("{indent}\u{1F4C1}  {ancestor}");
                                    if ui.button(&label).clicked() {
                                        *new_scan_path = Some(path.clone());
                                    }
                                }
                            }
                        },
                    );
                } else {
                    ui.label(egui::RichText::new(*seg).size(13.0));
                    ui.label(
                        egui::RichText::new(" \u{203A} ")
                            .size(13.0)
                            .color(egui::Color32::GRAY),
                    );
                }
            }
        });
    }
}

fn show_table_scope_row(
    ui: &mut egui::Ui,
    show_table: impl FnOnce(&mut egui::Ui),
    show_scope: impl FnOnce(&mut egui::Ui),
) {
    let row_size = ui.available_size_before_wrap();
    ui.allocate_ui_with_layout(
        row_size,
        egui::Layout::left_to_right(egui::Align::Min),
        |ui| {
            ui.set_min_size(row_size);
            ui.set_max_size(row_size);
            ui.spacing_mut().item_spacing.x = 0.0;

            let row_height = row_size.y.max(0.0);
            let table_width = (row_size.x - SCOPE_PANEL_WIDTH - SCOPE_PANEL_GAP).max(260.0);

            let table_size = egui::vec2(table_width, row_height);
            ui.allocate_ui_with_layout(
                table_size,
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_size(table_size);
                    ui.set_max_size(table_size);
                    show_table(ui);
                },
            );

            ui.add_space(SCOPE_PANEL_GAP);

            let scope_size = egui::vec2(SCOPE_PANEL_WIDTH, row_height);
            ui.allocate_ui_with_layout(
                scope_size,
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_size(scope_size);
                    ui.set_max_size(scope_size);
                    show_scope(ui);
                },
            );
        },
    );
}

fn show_scope_panel(
    ui: &mut egui::Ui,
    scope: &mut ScanScopeState,
    enabled: bool,
) -> Option<Vec<PathBuf>> {
    if !enabled {
        ui.disable();
    }

    let mut scan_request = None;
    ui.label(egui::RichText::new("Scope").strong());
    ui.add_space(3.0);

    let list_h = (ui.available_height() - 104.0).max(96.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_height(list_h);
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical()
            .id_salt("scan_scope_scroll")
            .auto_shrink([false, false])
            .max_height(list_h)
            .show(ui, |ui| {
                ui.set_min_height(list_h);
                for item in &mut scope.items {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut item.checked, "");
                        let label = if item.custom {
                            egui::RichText::new(&item.label).strong()
                        } else {
                            egui::RichText::new(&item.label)
                        };
                        ui.add(egui::Label::new(label).sense(egui::Sense::hover()))
                            .on_hover_text(item.path.display().to_string());
                    });
                }
            });
    });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Browse...").clicked()
            && let Some(path) = pick_folder()
        {
            scope.add_custom_checked(&path);
        }

        let remove = ui
            .add_enabled(scope.has_checked_custom(), egui::Button::new("-"))
            .on_hover_text("Remove checked custom directories");
        if remove.clicked() {
            scope.remove_checked_custom();
        }

        let checked_paths = scope.checked_paths();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(!checked_paths.is_empty(), egui::Button::new("Scan"))
                .clicked()
            {
                scan_request = Some(checked_paths);
            }
        });
    });

    let space = scope.selected_space();
    ui.add_space(4.0);
    ui.small(format!("Total: {}", format_size(space.total)));
    ui.small(format!("Used: {}", format_size(space.used)));
    ui.small(format!("Free: {}", format_size(space.free)));

    scan_request
}

/// Snapshot of a node's metadata needed for deletion.
struct DeleteTarget {
    id: NodeId,
    fs_path: std::path::PathBuf,
    is_dir: bool,
    size: u64,
}

impl DeleteTarget {
    /// Resolve the selected node into a DeleteTarget, or None if the path is invalid.
    fn from_id(tree: &FileTree, id: NodeId) -> Option<Self> {
        let sel_path = tree.root.path_to_id(id)?;
        let fs_path = tree.build_fs_path(&sel_path)?;
        let node = tree.root.resolve_path(&sel_path)?;
        Some(Self {
            id,
            fs_path,
            is_dir: node.is_dir,
            size: node.size,
        })
    }

    fn name(&self) -> &str {
        self.fs_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
    }
}

/// Handle Delete/Backspace when something is selected.
/// Shift+Delete bypasses confirmation; Delete alone confirms.
fn handle_delete(loaded: &mut LoadedState, ctx: &egui::Context) -> Option<NodeCommand> {
    let id = loaded.pane.selected?;
    let delete = ctx.input(|i| {
        let del = i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace);
        del.then_some(!i.modifiers.shift)
    });
    delete.map(|confirm| NodeCommand::Delete { id, confirm })
}

fn execute_node_command(loaded: &mut LoadedState, ctx: &egui::Context, command: NodeCommand) {
    loaded.status_message = None;
    match command {
        NodeCommand::Open(id) => {
            if let Some(path) = loaded.tree.build_fs_path_for_id(id) {
                open_path(&path, loaded);
            }
        }
        NodeCommand::Reveal(id) => {
            if let Some(path) = loaded.tree.build_fs_path_for_id(id) {
                reveal_in_finder(&path, loaded);
            }
        }
        NodeCommand::CopyPath(id) => {
            if let Some(path) = loaded.tree.full_display_path_for_id(id) {
                ctx.copy_text(path);
            }
        }
        NodeCommand::Delete { id, confirm } => {
            let Some(target) = DeleteTarget::from_id(&loaded.tree, id) else {
                loaded.status_message = Some("Selected item no longer exists".to_owned());
                return;
            };
            if confirm
                && !native_confirm_delete(
                    target.name(),
                    target.size,
                    &target.fs_path,
                    target.is_dir,
                )
            {
                return;
            }
            execute_delete(loaded, &target);
        }
        NodeCommand::ZoomIn(id) => {
            zoom_in_treemap(loaded, id);
        }
        NodeCommand::ZoomOut => {
            zoom_out_treemap(loaded);
        }
        NodeCommand::ToggleShrink(id) => {
            toggle_treemap_shrink(loaded, id);
        }
    }
}

fn toggle_treemap_shrink(loaded: &mut LoadedState, id: NodeId) {
    let Some(node) = loaded.tree.root.resolve_id(id) else {
        loaded.status_message = Some("Cannot shrink: item no longer exists".to_owned());
        return;
    };
    let name = node.name.to_string();
    let is_shrunk = loaded.treemap.toggle_shrink(id);
    loaded.memory_relief.restart();
    loaded.status_message = Some(if is_shrunk {
        format!("Shrunk {name} in treemap")
    } else {
        format!("Restored {name} in treemap")
    });
}

fn zoom_in_treemap(loaded: &mut LoadedState, id: NodeId) {
    let Some(node) = loaded.tree.root.resolve_id(id) else {
        loaded.status_message = Some("Cannot zoom: item no longer exists".to_owned());
        return;
    };
    if !node.is_dir {
        loaded.status_message = Some("Cannot zoom into a file".to_owned());
        return;
    }
    if loaded.treemap.root_id == id {
        return;
    }
    if node_contains_id(&loaded.tree, loaded.treemap.root_id, id) {
        loaded.treemap.zoom_history.push(loaded.treemap.root_id);
    } else {
        loaded.treemap.zoom_history.clear();
        if loaded.tree.root.id != id {
            loaded.treemap.zoom_history.push(loaded.tree.root.id);
        }
    }
    loaded.treemap.root_id = id;
    loaded.pane.selected = Some(id);
    loaded.treemap.clear_layout();
    loaded.memory_relief.restart();
}

fn zoom_out_treemap(loaded: &mut LoadedState) {
    if let Some(previous) = loaded.treemap.zoom_history.pop()
        && loaded.tree.root.resolve_id(previous).is_some()
    {
        loaded.treemap.root_id = previous;
        loaded.pane.selected = Some(previous);
        loaded.treemap.clear_layout();
        loaded.memory_relief.restart();
        return;
    }

    if let Some(parent_id) = parent_id_for_node(&loaded.tree, loaded.treemap.root_id) {
        loaded.treemap.root_id = parent_id;
        loaded.pane.selected = Some(parent_id);
        loaded.treemap.clear_layout();
        loaded.memory_relief.restart();
        return;
    }

    if let Some(path) = loaded.tree.build_fs_path_for_id(loaded.treemap.root_id)
        && let Some(parent) = path.parent()
        && parent != path
    {
        loaded.pending_scan = Some(vec![parent.to_path_buf()]);
    }
}

fn parent_id_for_node(tree: &FileTree, id: NodeId) -> Option<NodeId> {
    let path = tree.root.path_to_id(id)?;
    let (_, parent_path) = path.split_last()?;
    tree.root.resolve_path(parent_path).map(|node| node.id)
}

fn node_contains_id(tree: &FileTree, root_id: NodeId, descendant_id: NodeId) -> bool {
    tree.root
        .resolve_id(root_id)
        .is_some_and(|root| root.resolve_id(descendant_id).is_some())
}

fn execute_delete(loaded: &mut LoadedState, target: &DeleteTarget) {
    let result = if target.is_dir {
        std::fs::remove_dir_all(&target.fs_path)
    } else {
        std::fs::remove_file(&target.fs_path)
    };
    match result {
        Ok(()) => {
            if let Some(node) = loaded.tree.root.resolve_id(target.id) {
                loaded.deleted.mark_deleted(node);
            }
            loaded.pane.hovered = None;
            loaded.status_message = Some(format!("Deleted {}", target.name()));
        }
        Err(e) => {
            loaded.status_message = Some(format!("Failed to delete {}: {}", target.name(), e));
        }
    }
}

/// Render the three-pane layout with empty panels (same IDs as Loaded state).
fn show_empty_panes(
    ctx: &egui::Context,
    scope: &mut ScanScopeState,
    scan_request: &mut Option<Vec<PathBuf>>,
    enabled: bool,
) {
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |_ui| {});

    egui::SidePanel::left("file_table")
        .default_width(820.0)
        .min_width(640.0)
        .show_separator_line(false)
        .frame(
            egui::Frame::side_top_panel(ctx.style().as_ref()).inner_margin(egui::Margin::from(8)),
        )
        .show(ctx, |ui| {
            if !enabled {
                ui.disable();
            }
            show_table_scope_row(
                ui,
                |ui| {
                    ui::tree_view::show_branding(ui);
                },
                |ui| {
                    if let Some(paths) = show_scope_panel(ui, scope, enabled) {
                        *scan_request = Some(paths);
                    }
                },
            );
        });

    egui::CentralPanel::default().show(ctx, |_ui| {});
}

fn show_scanning_overlay(ctx: &egui::Context, paths: &[PathBuf], elapsed: Duration) {
    let screen_rect = ctx.screen_rect();

    egui::Area::new(egui::Id::new("scan_overlay_blocker"))
        .order(egui::Order::Middle)
        .fixed_pos(screen_rect.min)
        .show(ctx, |ui| {
            let (rect, _response) =
                ui.allocate_exact_size(screen_rect.size(), egui::Sense::click_and_drag());
            ui.painter().rect_filled(
                rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 110),
            );
        });

    egui::Area::new(egui::Id::new("scan_overlay"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(egui::Color32::from_rgba_unmultiplied(32, 32, 32, 230))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40),
                ))
                .inner_margin(egui::Margin::symmetric(20, 16))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.heading("Scanning...");
                    });
                    ui.add_space(6.0);
                    ui.label(scan_scope_display(paths));
                    ui.label(format!("{:.1}s elapsed", elapsed.as_secs_f64()));
                });
        });
    ctx.request_repaint_after(Duration::from_millis(100));
}

fn show_scan_failed(
    ctx: &egui::Context,
    paths: &[PathBuf],
    message: &str,
    retry_paths: &mut Option<Vec<PathBuf>>,
) {
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        ui.colored_label(
            egui::Color32::from_rgb(220, 80, 80),
            format!("Failed to scan {}", scan_scope_display(paths)),
        );
    });

    egui::SidePanel::left("file_table")
        .default_width(820.0)
        .min_width(640.0)
        .show_separator_line(false)
        .frame(
            egui::Frame::side_top_panel(ctx.style().as_ref()).inner_margin(egui::Margin::from(8)),
        )
        .show(ctx, |ui| {
            ui::tree_view::show_branding(ui);
        });

    egui::CentralPanel::default().show(ctx, |ui| {
        ui.centered_and_justified(|ui| {
            ui.vertical_centered(|ui| {
                ui.heading("Scan Failed");
                ui.add_space(8.0);
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), message);
                ui.add_space(12.0);
                if ui.button("Open Folder...").clicked() {
                    *retry_paths = pick_folder().map(|path| vec![path]);
                }
                if ui.button("Retry").clicked() {
                    *retry_paths = Some(paths.to_vec());
                }
            });
        });
    });
}

fn scan_scope_display(paths: &[PathBuf]) -> String {
    match paths {
        [] => "nothing".to_owned(),
        [path] => path.display().to_string(),
        _ => format!("{} locations", paths.len()),
    }
}

fn reveal_in_finder(path: &std::path::Path, loaded: &mut LoadedState) {
    if let Err(e) = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn()
    {
        loaded.status_message = Some(format!("Failed to reveal {:?}: {}", path, e));
    }
}

fn open_path(path: &std::path::Path, loaded: &mut LoadedState) {
    if let Err(e) = std::process::Command::new("open").arg(path).spawn() {
        loaded.status_message = Some(format!("Failed to open {:?}: {}", path, e));
    }
}

/// Show a native macOS alert for delete confirmation. Returns true if user clicked "Delete".
fn native_confirm_delete(name: &str, size: u64, fs_path: &std::path::Path, is_dir: bool) -> bool {
    let kind = if is_dir { "directory" } else { "file" };
    let escaped_name = applescript_escape(name);
    let escaped_path = applescript_escape(&fs_path.display().to_string());
    let size_str = format_size(size);

    let mut message = format!("{} ({})\n{}", escaped_name, size_str, escaped_path);
    if is_dir {
        message.push_str("\n\nThis will permanently delete the directory and all its contents.");
    }

    let script = format!(
        r#"display alert "Delete this {}?" message "{}" as critical buttons {{"Cancel", "Delete"}} default button "Cancel""#,
        kind, message,
    );

    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout.contains("button returned:Delete")
        }
        _ => false,
    }
}

/// Escape a string for use inside AppleScript double-quoted strings.
fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Set text fields in the native About panel via the app's info dictionary.
#[cfg(target_os = "macos")]
fn configure_about_panel_text() {
    use crate::objc_ffi::*;

    unsafe {
        let bundle_cls = objc_getClass(c"NSBundle".as_ptr());
        let main_bundle = send0(bundle_cls, sel_registerName(c"mainBundle".as_ptr()));
        let info = send0(main_bundle, sel_registerName(c"infoDictionary".as_ptr()));
        let set_sel = sel_registerName(c"setObject:forKey:".as_ptr());

        send2_void(
            info,
            set_sel,
            nsstring("MacDirStat"),
            nsstring("CFBundleName"),
        );

        let version = env!("CARGO_PKG_VERSION");
        send2_void(
            info,
            set_sel,
            nsstring(version),
            nsstring("CFBundleShortVersionString"),
        );

        send2_void(
            info,
            set_sel,
            nsstring(
                "Author: Michael Strömberg\n\
                 \u{00A9} 2026 \u{2014} Licensed under GPL-3.0\n\n\
                 github.com/MichaelStromberg/macdirstat\n\
                 crates.io/crates/macdirstat",
            ),
            nsstring("NSHumanReadableCopyright"),
        );
    }
}

/// Folder picker starting at $HOME — used on startup.
fn pick_folder_at_home() -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users".to_string());
    rfd::FileDialog::new()
        .set_title("Select folder to scan")
        .set_directory(&home)
        .pick_folder()
}

/// Folder picker — used from the breadcrumb menu.
fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Select folder to scan")
        .pick_folder()
}
