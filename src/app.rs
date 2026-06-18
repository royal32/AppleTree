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

use crate::model::color::{ColorMap, folder_frame_color, palette_colors};
use crate::model::tree::{FileTree, NodeId};
use crate::settings::{AppPrefs, FilenameTruncation, SplitOrientation, TreemapPalette};
use crate::ui::file_icons::FileIconCache;
use crate::ui::{self, NodeCommand};
use crate::{format_compact_count, format_size};

pub struct App {
    state: AppState,
    prefs: AppPrefs,
    scope: ScanScopeState,
    scope_logo: egui::TextureHandle,
    prefs_changed: bool,
    #[cfg(target_os = "macos")]
    about_configured: bool,
}

enum AppState {
    Empty,
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
    scan_paths: Vec<PathBuf>,
    scan_time_ms: f64,
}

struct LoadedState {
    tree: FileTree,
    color_map: ColorMap,
    scan_paths: Vec<PathBuf>,
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
    last_file_label_depth: usize,
    last_folder_label_depth: usize,
    palette_preview: Option<TreemapPalette>,
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
const LEFT_RIGHT_SCOPE_PANEL_HEIGHT: f32 = 330.0;
const LEFT_RIGHT_SCOPE_PANEL_MIN_HEIGHT: f32 = 190.0;
const FILE_TABLE_MIN_HEIGHT: f32 = 220.0;
const SCOPE_LOGO_MAX_WIDTH: f32 = 90.0;
const SCOPE_LOGO_BOTTOM_GAP: f32 = 8.0;
const SCOPE_LIST_MIN_HEIGHT: f32 = 96.0;
const SCOPE_CONTROLS_HEIGHT: f32 = 44.0;

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
        let mut paths = Vec::new();
        for item in self.items.iter().filter(|item| item.checked) {
            if !item.path.is_dir() {
                continue;
            }
            paths.push(item.path.clone());
        }
        normalize_scope_paths(paths)
    }

    fn remove_checked_custom(&mut self) {
        self.items.retain(|item| !(item.custom && item.checked));
    }

    fn has_checked_custom(&self) -> bool {
        self.items.iter().any(|item| item.custom && item.checked)
    }
}

fn selected_scope_space(paths: &[PathBuf], used: u64) -> ScopeSpace {
    let mut seen_mounts = BTreeSet::new();
    let mut space = ScopeSpace {
        used,
        ..ScopeSpace::default()
    };

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
    space
}

fn normalize_scope_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut paths = paths
        .into_iter()
        .filter(|path| path.is_dir())
        .map(|path| {
            let key = canonical_scope_path(&path);
            (key, path)
        })
        .collect::<Vec<_>>();
    paths.sort_by(|(a, _), (b, _)| {
        a.components()
            .count()
            .cmp(&b.components().count())
            .then_with(|| a.cmp(b))
    });

    let mut kept_keys: Vec<PathBuf> = Vec::new();
    let mut normalized = Vec::new();
    for (key, path) in paths {
        if kept_keys.iter().any(|parent| key.starts_with(parent)) {
            continue;
        }
        kept_keys.push(key);
        normalized.push(path);
    }
    normalized
}

fn canonical_scope_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn path_space(path: &Path) -> Option<(String, u64, u64)> {
    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statfs(c_path.as_ptr(), &mut stats) };
    if rc != 0 {
        return None;
    }

    let block_size = stats.f_bsize as u64;
    let total = (stats.f_blocks as u64).saturating_mul(block_size);
    let free = (stats.f_bavail as u64).saturating_mul(block_size);
    let mount = unsafe { CStr::from_ptr(stats.f_mntonname.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    Some((mount, total, free))
}

fn load_scope_logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let bytes = include_bytes!("ui/assets/black_text_with_white_backdrop.png");
    let image = image::load_from_memory(bytes)
        .expect("Failed to decode scope logo")
        .into_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    ctx.load_texture("scope_logo", color_image, egui::TextureOptions::LINEAR)
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial_path: Option<String>) -> Self {
        #[cfg(target_os = "macos")]
        use_macos_system_font(&cc.egui_ctx);

        let initial_path_buf = initial_path.as_ref().map(PathBuf::from);
        let mut app = Self {
            state: AppState::Empty,
            prefs: AppPrefs::load(),
            scope: ScanScopeState::new(initial_path_buf.as_deref()),
            scope_logo: load_scope_logo_texture(&cc.egui_ctx),
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

    fn start_scan_paths(&mut self, paths: Vec<PathBuf>) {
        let paths = normalize_scope_paths(paths);
        if paths.is_empty() {
            return;
        }

        let previous = match std::mem::replace(&mut self.state, AppState::Empty) {
            AppState::Loaded(loaded) => Some(loaded),
            AppState::Scanning { previous, .. } => previous,
            _ => None,
        };

        let start_time = Instant::now();
        let (sender, receiver) = mpsc::channel();
        let worker_paths = paths.clone();
        let worker_sender = sender.clone();
        let worker = match std::thread::Builder::new()
            .name("appletree-scan".to_owned())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let tree = FileTree::scan_paths(&worker_paths);
                    let color_map = ColorMap::from_extensions(&tree.extensions);
                    ScanCompletion {
                        tree,
                        color_map,
                        scan_paths: worker_paths.clone(),
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
        let scope_logo = self.scope_logo.clone();
        match &mut self.state {
            AppState::Empty => {
                let mut scan_paths = None;
                show_empty_panes(
                    ctx,
                    &mut self.prefs,
                    &mut self.prefs_changed,
                    &mut self.scope,
                    &scope_logo,
                    &mut scan_paths,
                    true,
                );
                if let Some(paths) = scan_paths {
                    immediate_scan_paths = Some(paths);
                }
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
                        &scope_logo,
                        &mut self.prefs_changed,
                    );
                    previous.memory_relief.run(ctx);
                } else {
                    let mut scan_paths = None;
                    show_empty_panes(
                        ctx,
                        &mut self.prefs,
                        &mut self.prefs_changed,
                        &mut self.scope,
                        &scope_logo,
                        &mut scan_paths,
                        false,
                    );
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
                    &scope_logo,
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

        // Handle ⌘O and pending scans requested by in-panel controls outside
        // the match to avoid borrow conflicts with self.state.
        let cmd_o = ctx.input(|i| i.key_pressed(egui::Key::O) && i.modifiers.command);
        match &mut self.state {
            AppState::Empty => {
                if cmd_o && let Some(path) = pick_folder() {
                    self.scope.add_custom_checked(&path);
                    self.start_scan_paths(vec![path]);
                }
            }
            AppState::Loaded(loaded) => {
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
            AppState::Scanning { .. } | AppState::ScanFailed { .. } => {}
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
        let default_prefs = AppPrefs::default();

        Self {
            tree: completion.tree,
            color_map: completion.color_map,
            scan_paths: completion.scan_paths,
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
            last_file_label_depth: default_prefs.treemap_label_depth,
            last_folder_label_depth: default_prefs.treemap_folder_depth,
            palette_preview: None,
        }
    }

    fn show_panels(
        &mut self,
        ctx: &egui::Context,
        prefs: &mut AppPrefs,
        scope: &mut ScanScopeState,
        scope_logo: &egui::TextureHandle,
        prefs_changed: &mut bool,
    ) -> Option<NodeCommand> {
        self.show_panels_enabled(ctx, prefs, scope, scope_logo, prefs_changed, true)
    }

    fn show_disabled_panels(
        &mut self,
        ctx: &egui::Context,
        prefs: &mut AppPrefs,
        scope: &mut ScanScopeState,
        scope_logo: &egui::TextureHandle,
        prefs_changed: &mut bool,
    ) -> Option<NodeCommand> {
        self.show_panels_enabled(ctx, prefs, scope, scope_logo, prefs_changed, false)
    }

    fn show_panels_enabled(
        &mut self,
        ctx: &egui::Context,
        prefs: &mut AppPrefs,
        scope: &mut ScanScopeState,
        scope_logo: &egui::TextureHandle,
        prefs_changed: &mut bool,
        enabled: bool,
    ) -> Option<NodeCommand> {
        self.show_status_bar(ctx, prefs, prefs_changed, enabled);
        self.show_main_layout(ctx, prefs, scope, scope_logo, prefs_changed, enabled)
    }

    fn show_status_bar(
        &mut self,
        ctx: &egui::Context,
        prefs: &mut AppPrefs,
        prefs_changed: &mut bool,
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
                    status_path_label(ui, &path);
                }
                if let Some(message) = &self.status_message {
                    ui.separator();
                    ui.colored_label(egui::Color32::from_rgb(220, 80, 80), message);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if split_layout_toggle(ui, &mut prefs.split_orientation) {
                        *prefs_changed = true;
                    }
                    if filename_truncation_toggle(ui, &mut prefs.filename_truncation) {
                        *prefs_changed = true;
                    }
                    let palette_response = color_palette_control(ui, prefs.treemap_palette);
                    if let Some(palette) = palette_response.selected {
                        prefs.treemap_palette = palette;
                        *prefs_changed = true;
                    }
                    if self.palette_preview != palette_response.hovered
                        || palette_response.selected.is_some()
                    {
                        self.palette_preview = palette_response.hovered;
                        self.treemap.clear_texture();
                        self.memory_relief.restart();
                    }

                    ui.separator();
                    if let Some(label_depth) = icon_depth_slider(
                        ui,
                        StatusIcon::FileLabels,
                        prefs.treemap_label_depth,
                        if prefs.treemap_label_depth > 0 {
                            prefs.treemap_label_depth
                        } else {
                            self.last_file_label_depth
                        },
                        0..=5,
                        "File labels",
                    ) {
                        if label_depth > 0 {
                            self.last_file_label_depth = label_depth;
                        } else if prefs.treemap_label_depth > 0 {
                            self.last_file_label_depth = prefs.treemap_label_depth;
                        }
                        prefs.treemap_label_depth = label_depth;
                        *prefs_changed = true;
                    }
                    if let Some(folder_depth) = icon_depth_slider(
                        ui,
                        StatusIcon::FolderLabels,
                        prefs.treemap_folder_depth,
                        if prefs.treemap_folder_depth > 0 {
                            prefs.treemap_folder_depth
                        } else {
                            self.last_folder_label_depth
                        },
                        0..=6,
                        "Folder boxes",
                    ) {
                        if folder_depth > 0 {
                            self.last_folder_label_depth = folder_depth;
                        } else if prefs.treemap_folder_depth > 0 {
                            self.last_folder_label_depth = prefs.treemap_folder_depth;
                        }
                        prefs.treemap_folder_depth = folder_depth;
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
        scope_logo: &egui::TextureHandle,
        prefs_changed: &mut bool,
        enabled: bool,
    ) -> Option<NodeCommand> {
        let mut command = None;
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
                        if let Some(cmd) = self.show_file_table_and_scope(
                            ui,
                            prefs,
                            scope,
                            scope_logo,
                            SplitOrientation::LeftRight,
                            prefs_changed,
                        ) {
                            command = Some(cmd);
                        }
                    });

                egui::CentralPanel::default().show(ctx, |ui| {
                    if !enabled {
                        ui.disable();
                    }
                    if let Some(cmd) = self.show_treemap(ui, prefs) {
                        command = Some(cmd);
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
                        if let Some(cmd) = self.show_file_table_and_scope(
                            ui,
                            prefs,
                            scope,
                            scope_logo,
                            SplitOrientation::TopBottom,
                            prefs_changed,
                        ) {
                            command = Some(cmd);
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
                    if let Some(cmd) = self.show_treemap(ui, prefs) {
                        command = Some(cmd);
                    }
                });
            }
        }
        command
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
        scope_logo: &egui::TextureHandle,
        orientation: SplitOrientation,
        prefs_changed: &mut bool,
    ) -> Option<NodeCommand> {
        let mut command = None;
        let mut pending_scan = None;
        let scope_space = selected_scope_space(&self.scan_paths, self.tree.root.size);

        let mut show_table = |ui: &mut egui::Ui| {
            if let Some(cmd) = self.show_file_table(ui, prefs, prefs_changed) {
                command = Some(cmd);
            }
        };
        let mut show_scope = |ui: &mut egui::Ui| {
            show_scope_logo(ui, scope_logo, Some(&scope_space));
            pending_scan = show_scope_panel(ui, scope, true);
        };
        match orientation {
            SplitOrientation::LeftRight => {
                show_table_scope_column(ui, &mut show_table, &mut show_scope);
            }
            SplitOrientation::TopBottom => {
                show_table_scope_row(ui, &mut show_table, &mut show_scope);
            }
        }

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
            self.palette_preview.unwrap_or(prefs.treemap_palette),
            &mut self.treemap,
        )
    }

    fn status_path(&self) -> Option<String> {
        self.pane
            .hovered
            .or(self.pane.selected)
            .and_then(|id| self.tree.full_display_path_for_id(id))
    }
}

#[derive(Clone, Copy)]
enum StatusIcon {
    ColorPalette,
    FileLabels,
    FolderLabels,
    FilenameTruncation,
    SplitLayout,
}

struct PaletteControlResponse {
    hovered: Option<TreemapPalette>,
    selected: Option<TreemapPalette>,
}

fn split_layout_toggle(ui: &mut egui::Ui, orientation: &mut SplitOrientation) -> bool {
    let rotation = match orientation {
        SplitOrientation::LeftRight => 0,
        SplitOrientation::TopBottom => 1,
    };
    let response = status_icon_button(ui, StatusIcon::SplitLayout, rotation, true);
    let tooltip = match orientation {
        SplitOrientation::LeftRight => "Switch to top/bottom layout",
        SplitOrientation::TopBottom => "Switch to left/right layout",
    };
    show_immediate_tooltip(&response, tooltip);
    let clicked = response.clicked();
    if clicked {
        *orientation = match orientation {
            SplitOrientation::LeftRight => SplitOrientation::TopBottom,
            SplitOrientation::TopBottom => SplitOrientation::LeftRight,
        };
    }
    clicked
}

fn color_palette_control(ui: &mut egui::Ui, selected: TreemapPalette) -> PaletteControlResponse {
    let response = status_icon_button(ui, StatusIcon::ColorPalette, 0, true);
    show_immediate_tooltip(&response, "Treemap colors");

    let popup_id = response.id.with("palette_popup");
    let pointer_pos = ui.ctx().pointer_hover_pos();
    let popup_hovered = ui
        .ctx()
        .data(|data| data.get_temp::<egui::Rect>(popup_id))
        .is_some_and(|rect| pointer_pos.is_some_and(|pos| rect.expand(6.0).contains(pos)));

    let mut result = PaletteControlResponse {
        hovered: None,
        selected: None,
    };

    if response.hovered() || popup_hovered {
        let popup_pos = response.rect.left_top() + egui::vec2(0.0, -6.0);
        let inner = egui::Area::new(popup_id)
            .order(egui::Order::Tooltip)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(popup_pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for palette in TreemapPalette::ALL {
                            let tile = palette_tile(ui, palette, palette == selected);
                            if tile.hovered() {
                                result.hovered = Some(palette);
                            }
                            if tile.clicked() {
                                result.selected = Some(palette);
                            }
                            show_immediate_tooltip(&tile, palette.label());
                        }
                    });
                });
            });
        ui.ctx()
            .data_mut(|data| data.insert_temp(popup_id, inner.response.rect));
    } else {
        ui.ctx()
            .data_mut(|data| data.remove::<egui::Rect>(popup_id));
    }

    result
}

fn palette_tile(ui: &mut egui::Ui, palette: TreemapPalette, selected: bool) -> egui::Response {
    let size = egui::vec2(54.0, 34.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let visuals = ui.style().interact(&response);
    let painter = ui.painter();

    painter.rect_filled(rect, 4.0, visuals.bg_fill);

    let swatch_rect = rect.shrink2(egui::vec2(5.0, 6.0));
    let colors = palette_colors(palette);
    let stripe_w = swatch_rect.width() / 6.0;
    for i in 0..6 {
        let stripe = egui::Rect::from_min_max(
            egui::pos2(swatch_rect.left() + i as f32 * stripe_w, swatch_rect.top()),
            egui::pos2(
                if i == 5 {
                    swatch_rect.right()
                } else {
                    swatch_rect.left() + (i + 1) as f32 * stripe_w
                },
                swatch_rect.bottom(),
            ),
        );
        painter.rect_filled(stripe, 0.0, colors[i]);
    }
    if palette == TreemapPalette::DesaturatedBoldFrames {
        painter.rect_stroke(
            swatch_rect,
            0.0,
            egui::Stroke::new(1.5, folder_frame_color(palette)),
            egui::StrokeKind::Inside,
        );
    }

    let stroke = if selected {
        egui::Stroke::new(1.5, ui.visuals().selection.stroke.color)
    } else if response.hovered() || response.has_focus() {
        visuals.bg_stroke
    } else {
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
    };
    painter.rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Inside);

    response
}

fn filename_truncation_toggle(ui: &mut egui::Ui, truncation: &mut FilenameTruncation) -> bool {
    let flip_y = match truncation {
        FilenameTruncation::Middle => false,
        FilenameTruncation::End => true,
    };
    let response =
        status_icon_button_transformed(ui, StatusIcon::FilenameTruncation, 0, flip_y, true);
    let tooltip = match truncation {
        FilenameTruncation::Middle => "Switch to end truncation",
        FilenameTruncation::End => "Switch to middle truncation",
    };
    show_immediate_tooltip(&response, tooltip);
    let clicked = response.clicked();
    if clicked {
        *truncation = match truncation {
            FilenameTruncation::Middle => FilenameTruncation::End,
            FilenameTruncation::End => FilenameTruncation::Middle,
        };
    }
    clicked
}

fn icon_depth_slider(
    ui: &mut egui::Ui,
    icon: StatusIcon,
    current: usize,
    restore_value: usize,
    range: std::ops::RangeInclusive<u32>,
    label: &'static str,
) -> Option<usize> {
    let range_start = *range.start();
    let range_end = *range.end();
    let mut value = (current as u32).clamp(range_start, range_end);
    let mut next_value = None;
    let response = status_icon_button(ui, icon, 0, current > 0);
    if response.clicked() {
        let restored = (restore_value as u32).clamp(range_start.max(1), range_end) as usize;
        next_value = Some(if current == 0 { restored } else { 0 });
    }
    let popup_id = response.id.with("depth_slider_popup");
    let pointer_pos = ui.ctx().pointer_hover_pos();
    let popup_hovered = ui
        .ctx()
        .data(|data| data.get_temp::<egui::Rect>(popup_id))
        .is_some_and(|rect| pointer_pos.is_some_and(|pos| rect.expand(6.0).contains(pos)));

    if response.hovered() || popup_hovered {
        let popup_pos = response.rect.left_top() + egui::vec2(0.0, -6.0);
        let inner = egui::Area::new(popup_id)
            .order(egui::Order::Tooltip)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(popup_pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_size(egui::vec2(46.0, 120.0));
                    ui.vertical_centered(|ui| {
                        ui.label(value.to_string());
                        let slider_response = ui.add(
                            egui::Slider::new(&mut value, range_start..=range_end)
                                .vertical()
                                .show_value(false),
                        );
                        paint_slider_notches(ui, slider_response.rect, range_start..=range_end);
                        if slider_response.changed() {
                            next_value = Some(value as usize);
                        }
                    });
                });
            });
        ui.ctx()
            .data_mut(|data| data.insert_temp(popup_id, inner.response.rect));
        if response.hovered() || popup_hovered {
            show_immediate_tooltip_above(
                ui.ctx(),
                response.id.with("tooltip"),
                inner.response.rect,
                label,
            );
        }
    } else {
        ui.ctx()
            .data_mut(|data| data.remove::<egui::Rect>(popup_id));
    }

    next_value
}

fn status_path_label(ui: &mut egui::Ui, path: &str) {
    let max_width = (ui.available_width() - status_controls_reserved_width(ui)).max(0.0);
    if max_width <= 0.0 {
        return;
    }

    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let color = ui.visuals().text_color();
    let text = middle_truncate_status_text(ui, path, &font_id, color, max_width);
    let response = ui.add_sized(
        egui::vec2(max_width, ui.spacing().interact_size.y),
        egui::Label::new(text)
            .truncate()
            .sense(egui::Sense::click()),
    );
    show_immediate_tooltip(&response, "Copy full path");
    if response.clicked() {
        ui.ctx().copy_text(path.to_owned());
    }
}

fn status_controls_reserved_width(ui: &egui::Ui) -> f32 {
    let icon_width = 28.0;
    let control_count = 5.0;
    let separator_width = 12.0;
    let spacing = ui.spacing().item_spacing.x;
    control_count * icon_width + separator_width + 7.0 * spacing
}

fn middle_truncate_status_text(
    ui: &egui::Ui,
    text: &str,
    font_id: &egui::FontId,
    color: egui::Color32,
    max_width: f32,
) -> String {
    if text.is_empty() || max_width <= 0.0 {
        return String::new();
    }
    if status_text_width(ui, text, font_id, color) <= max_width {
        return text.to_owned();
    }

    const MARKER: &str = "…";
    if status_text_width(ui, MARKER, font_id, color) > max_width {
        return String::new();
    }

    let chars = text.chars().collect::<Vec<_>>();
    let mut best = MARKER.to_owned();
    let mut low = 0usize;
    let mut high = chars.len().saturating_sub(1);
    while low <= high {
        let visible = (low + high) / 2;
        let candidate = middle_truncate_status_chars(&chars, visible);
        if status_text_width(ui, &candidate, font_id, color) <= max_width {
            best = candidate;
            low = visible + 1;
        } else if visible == 0 {
            break;
        } else {
            high = visible - 1;
        }
    }
    best
}

fn middle_truncate_status_chars(chars: &[char], visible: usize) -> String {
    if visible == 0 {
        return "…".to_owned();
    }

    let suffix_len = ((visible * 2) / 3).max(1).min(chars.len());
    let prefix_len = visible
        .saturating_sub(suffix_len)
        .min(chars.len() - suffix_len);

    let mut truncated = String::with_capacity(visible + 3);
    truncated.extend(chars.iter().take(prefix_len));
    truncated.push_str("…");
    truncated.extend(chars.iter().skip(chars.len() - suffix_len));
    truncated
}

fn status_text_width(
    ui: &egui::Ui,
    text: &str,
    font_id: &egui::FontId,
    color: egui::Color32,
) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), font_id.clone(), color)
        .size()
        .x
}

fn show_immediate_tooltip(response: &egui::Response, text: &'static str) {
    if response.hovered() {
        response.show_tooltip_text(text);
    }
}

fn show_immediate_tooltip_above(
    ctx: &egui::Context,
    id: egui::Id,
    anchor_rect: egui::Rect,
    text: &'static str,
) {
    let pos = anchor_rect.left_top() + egui::vec2(0.0, -6.0);
    egui::Area::new(id)
        .order(egui::Order::Tooltip)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(text);
            });
        });
}

fn paint_slider_notches(
    ui: &egui::Ui,
    slider_rect: egui::Rect,
    range: std::ops::RangeInclusive<u32>,
) {
    let start = *range.start();
    let end = *range.end();
    if end <= start {
        return;
    }

    let line_start_x = slider_rect.right() + 6.0;
    let line_end_x = ui.max_rect().right() - 6.0;
    if line_end_x <= line_start_x {
        return;
    }

    let stroke = egui::Stroke::new(0.75, ui.visuals().widgets.noninteractive.bg_stroke.color);
    let painter = ui.painter();
    let span = (end - start) as f32;
    let handle_radius = slider_rect.width() / 2.5;
    let notch_bottom = slider_rect.bottom() - handle_radius;
    let notch_top = slider_rect.top() + handle_radius;
    for notch in start..=end {
        let t = (notch - start) as f32 / span;
        let y = egui::lerp(notch_bottom..=notch_top, t);
        painter.line_segment(
            [egui::pos2(line_start_x, y), egui::pos2(line_end_x, y)],
            stroke,
        );
    }
}

fn status_icon_button(
    ui: &mut egui::Ui,
    icon: StatusIcon,
    quarter_turns: u8,
    active: bool,
) -> egui::Response {
    status_icon_button_transformed(ui, icon, quarter_turns, false, active)
}

fn status_icon_button_transformed(
    ui: &mut egui::Ui,
    icon: StatusIcon,
    quarter_turns: u8,
    flip_y: bool,
    active: bool,
) -> egui::Response {
    let size = egui::vec2(28.0, 24.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let visuals = ui.style().interact(&response);
    let painter = ui.painter();

    painter.rect_filled(rect, 4.0, visuals.bg_fill);
    if response.hovered() || response.has_focus() {
        painter.rect_stroke(rect, 4.0, visuals.bg_stroke, egui::StrokeKind::Inside);
    }

    let icon_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(20.0, 20.0));
    let stroke_color = if active {
        visuals.fg_stroke.color
    } else {
        ui.visuals().weak_text_color()
    };
    let stroke = egui::Stroke::new(0.75, stroke_color);
    match icon {
        StatusIcon::ColorPalette => paint_color_palette_icon(painter, icon_rect, stroke),
        StatusIcon::FileLabels => paint_file_label_icon(painter, icon_rect, stroke),
        StatusIcon::FolderLabels => paint_folder_label_icon(painter, icon_rect, stroke),
        StatusIcon::FilenameTruncation => {
            paint_filename_truncation_icon(painter, icon_rect, stroke, flip_y)
        }
        StatusIcon::SplitLayout => {
            paint_split_layout_icon(painter, icon_rect, stroke, quarter_turns)
        }
    }
    if !active {
        painter.line_segment(
            [icon_rect.left_bottom(), icon_rect.right_top()],
            egui::Stroke::new(1.1, stroke_color),
        );
    }

    response
}

fn icon_pos(rect: egui::Rect, x: f32, y: f32, quarter_turns: u8) -> egui::Pos2 {
    icon_pos_transformed(rect, x, y, quarter_turns, false)
}

fn icon_pos_transformed(
    rect: egui::Rect,
    x: f32,
    y: f32,
    quarter_turns: u8,
    flip_y: bool,
) -> egui::Pos2 {
    let y = if flip_y { 24.0 - y } else { y };
    let (x, y) = match quarter_turns % 4 {
        1 => (24.0 - y, x),
        2 => (24.0 - x, 24.0 - y),
        3 => (y, 24.0 - x),
        _ => (x, y),
    };
    egui::pos2(
        rect.left() + rect.width() * x / 24.0,
        rect.top() + rect.height() * y / 24.0,
    )
}

fn paint_polyline(
    painter: &egui::Painter,
    rect: egui::Rect,
    stroke: egui::Stroke,
    points: &[(f32, f32)],
) {
    painter.line(
        points
            .iter()
            .map(|&(x, y)| icon_pos(rect, x, y, 0))
            .collect::<Vec<_>>(),
        stroke,
    );
}

fn paint_svg_polyline(
    painter: &egui::Painter,
    rect: egui::Rect,
    stroke: egui::Stroke,
    flip_y: bool,
    points: &[(f32, f32)],
) {
    painter.line(
        points
            .iter()
            .map(|&(x, y)| icon_pos_transformed(rect, x, y, 0, flip_y))
            .collect::<Vec<_>>(),
        stroke,
    );
}

fn paint_eye_icon(painter: &egui::Painter, rect: egui::Rect, stroke: egui::Stroke) {
    paint_polyline(
        painter,
        rect,
        stroke,
        &[
            (10.6, 17.15),
            (12.0, 15.25),
            (14.0, 14.0),
            (16.0, 13.65),
            (18.0, 14.0),
            (20.0, 15.25),
            (21.4, 17.15),
            (20.0, 19.05),
            (18.0, 20.3),
            (16.0, 20.65),
            (14.0, 20.3),
            (12.0, 19.05),
            (10.6, 17.15),
        ],
    );
    painter.circle_stroke(
        icon_pos(rect, 16.0, 17.15, 0),
        rect.width() * 1.35 / 24.0,
        stroke,
    );
}

fn paint_file_label_icon(painter: &egui::Painter, rect: egui::Rect, stroke: egui::Stroke) {
    paint_polyline(
        painter,
        rect,
        stroke,
        &[(4.5, 3.75), (12.5, 3.75), (16.5, 7.75), (16.5, 11.8)],
    );
    paint_polyline(
        painter,
        rect,
        stroke,
        &[(12.5, 3.75), (12.5, 7.75), (16.5, 7.75)],
    );
    paint_polyline(
        painter,
        rect,
        stroke,
        &[(4.5, 3.75), (4.5, 20.25), (11.5, 20.25)],
    );
    paint_eye_icon(painter, rect, stroke);
}

fn paint_color_palette_icon(painter: &egui::Painter, rect: egui::Rect, stroke: egui::Stroke) {
    // Geometry mirrors src/ui/assets/color-palette.svg closely enough for the 20px status icon.
    paint_svg_polyline(
        painter,
        rect,
        stroke,
        false,
        &[
            (12.0, 3.25),
            (7.17, 3.25),
            (3.25, 7.17),
            (3.25, 12.0),
            (3.25, 16.83),
            (7.17, 20.75),
            (12.0, 20.75),
            (13.15, 20.75),
        ],
    );
    painter.circle_stroke(
        icon_pos(rect, 13.15, 18.9, 0),
        rect.width() * 1.85 / 24.0,
        stroke,
    );
    paint_svg_polyline(
        painter,
        rect,
        stroke,
        false,
        &[(14.7, 17.83), (14.22, 17.35), (14.14, 16.88)],
    );
    painter.circle_stroke(
        icon_pos(rect, 16.0, 13.13, 0),
        rect.width() * 3.45 / 24.0,
        stroke,
    );
    paint_svg_polyline(
        painter,
        rect,
        stroke,
        false,
        &[(16.0, 13.13), (16.0, 15.13), (20.75, 13.13)],
    );

    for (x, y) in [(7.75, 10.0), (10.1, 6.85), (14.15, 6.85), (16.6, 10.0)] {
        painter.circle_filled(
            icon_pos(rect, x, y, 0),
            rect.width() * 1.0 / 24.0,
            stroke.color,
        );
    }
}

fn paint_filename_truncation_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    stroke: egui::Stroke,
    flip_y: bool,
) {
    // Geometry mirrors src/ui/assets/filename-truncation.svg.
    paint_svg_polyline(painter, rect, stroke, flip_y, &[(4.25, 7.25), (7.5, 7.25)]);
    for x in [10.15, 12.0, 13.85] {
        painter.circle_filled(
            icon_pos_transformed(rect, x, 7.25, 0, flip_y),
            rect.width() * 0.45 / 24.0,
            stroke.color,
        );
    }
    paint_svg_polyline(
        painter,
        rect,
        stroke,
        flip_y,
        &[(16.5, 7.25), (19.75, 7.25)],
    );

    paint_svg_polyline(
        painter,
        rect,
        stroke,
        flip_y,
        &[(4.25, 16.75), (13.25, 16.75)],
    );
    for x in [16.15, 18.0, 19.85] {
        painter.circle_filled(
            icon_pos_transformed(rect, x, 16.75, 0, flip_y),
            rect.width() * 0.45 / 24.0,
            stroke.color,
        );
    }

    paint_svg_polyline(
        painter,
        rect,
        stroke,
        flip_y,
        &[(8.25, 11.95), (15.75, 11.95)],
    );
    paint_svg_polyline(
        painter,
        rect,
        stroke,
        flip_y,
        &[(13.25, 9.7), (15.75, 11.95), (13.25, 14.2)],
    );
}

fn paint_folder_label_icon(painter: &egui::Painter, rect: egui::Rect, stroke: egui::Stroke) {
    paint_polyline(
        painter,
        rect,
        stroke,
        &[
            (3.25, 6.25),
            (9.25, 6.25),
            (11.25, 8.25),
            (20.75, 8.25),
            (20.75, 15.4),
        ],
    );
    paint_polyline(
        painter,
        rect,
        stroke,
        &[(3.25, 6.25), (3.25, 18.75), (10.6, 18.75)],
    );
    paint_eye_icon(painter, rect, stroke);
}

fn paint_split_layout_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    stroke: egui::Stroke,
    quarter_turns: u8,
) {
    let p = |x, y| icon_pos(rect, x, y, quarter_turns);
    let rect_min = p(3.25, 4.25);
    let rect_max = p(20.75, 19.75);
    painter.rect_stroke(
        egui::Rect::from_min_max(
            egui::pos2(rect_min.x.min(rect_max.x), rect_min.y.min(rect_max.y)),
            egui::pos2(rect_min.x.max(rect_max.x), rect_min.y.max(rect_max.y)),
        ),
        2.0,
        stroke,
        egui::StrokeKind::Inside,
    );
    painter.line_segment([p(12.0, 4.25), p(12.0, 19.75)], stroke);
    painter.line_segment([p(14.75, 12.0), p(20.75, 12.0)], stroke);
    painter.line(vec![p(17.75, 9.0), p(20.75, 12.0), p(17.75, 15.0)], stroke);
}

fn show_table_scope_row(
    ui: &mut egui::Ui,
    mut show_table: impl FnMut(&mut egui::Ui),
    mut show_scope: impl FnMut(&mut egui::Ui),
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

fn show_table_scope_column(
    ui: &mut egui::Ui,
    mut show_table: impl FnMut(&mut egui::Ui),
    mut show_scope: impl FnMut(&mut egui::Ui),
) {
    let column_size = ui.available_size_before_wrap();
    ui.allocate_ui_with_layout(
        column_size,
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_min_size(column_size);
            ui.set_max_size(column_size);

            let available_h = column_size.y.max(0.0);
            let scope_height = if available_h
                >= FILE_TABLE_MIN_HEIGHT + SCOPE_PANEL_GAP + LEFT_RIGHT_SCOPE_PANEL_MIN_HEIGHT
            {
                (available_h - FILE_TABLE_MIN_HEIGHT - SCOPE_PANEL_GAP).clamp(
                    LEFT_RIGHT_SCOPE_PANEL_MIN_HEIGHT,
                    LEFT_RIGHT_SCOPE_PANEL_HEIGHT,
                )
            } else {
                (available_h * 0.42).max(0.0)
            };
            let gap = if scope_height > 0.0 {
                SCOPE_PANEL_GAP
            } else {
                0.0
            };
            let table_height = (available_h - scope_height - gap).max(0.0);

            let table_size = egui::vec2(column_size.x, table_height);
            ui.allocate_ui_with_layout(
                table_size,
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_size(table_size);
                    ui.set_max_size(table_size);
                    show_table(ui);
                },
            );

            ui.add_space(gap);

            let scope_size = egui::vec2(column_size.x, scope_height);
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

fn show_scope_logo(ui: &mut egui::Ui, logo: &egui::TextureHandle, space: Option<&ScopeSpace>) {
    let width = ui.available_width().min(SCOPE_LOGO_MAX_WIDTH);
    let aspect = logo.size_vec2().y / logo.size_vec2().x;
    let logo_width = if space.is_some() {
        width.min((ui.available_width() - 112.0).max(80.0))
    } else {
        width
    };
    let size = egui::vec2(logo_width, logo_width * aspect);
    let required_height =
        size.y + SCOPE_LOGO_BOTTOM_GAP + SCOPE_CONTROLS_HEIGHT + SCOPE_LIST_MIN_HEIGHT;
    if ui.available_height() < required_height {
        return;
    }

    if let Some(space) = space {
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), size.y),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                paint_scope_logo(ui, logo, size);
                ui.add_space(8.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), size.y),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_height(size.y);
                        ui.set_max_height(size.y);
                        ui.vertical(|ui| {
                            scope_stat_label(ui, format!("Total: {}", format_size(space.total)));
                            scope_stat_label(ui, format!("Used: {}", format_size(space.used)));
                            scope_stat_label(ui, format!("Free: {}", format_size(space.free)));
                        });
                    },
                );
            },
        );
    } else {
        let (slot, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), size.y),
            egui::Sense::hover(),
        );
        let image_rect = egui::Rect::from_center_size(slot.center(), size);
        paint_scope_logo_at(ui, logo, image_rect);
    }
    ui.add_space(SCOPE_LOGO_BOTTOM_GAP);
}

fn scope_stat_label(ui: &mut egui::Ui, text: String) {
    ui.label(egui::RichText::new(text).size(13.0));
}

fn paint_scope_logo(ui: &mut egui::Ui, logo: &egui::TextureHandle, size: egui::Vec2) {
    let (slot, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let image_rect = egui::Rect::from_center_size(slot.center(), size);
    paint_scope_logo_at(ui, logo, image_rect);
}

fn paint_scope_logo_at(ui: &egui::Ui, logo: &egui::TextureHandle, image_rect: egui::Rect) {
    ui.painter().image(
        logo.id(),
        image_rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
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

    let list_h = (ui.available_height() - SCOPE_CONTROLS_HEIGHT).max(SCOPE_LIST_MIN_HEIGHT);
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
                    show_scope_item_row(ui, item);
                }
            });
    });

    ui.add_space(6.0);
    let mut checked_paths = Vec::new();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if ui
            .add_sized(
                scope_button_size(ui, "Browse…"),
                egui::Button::new("Browse…"),
            )
            .clicked()
            && let Some(path) = pick_folder()
        {
            scope.add_custom_checked(&path);
        }

        let remove_size = egui::vec2(
            ui.spacing().interact_size.x * 1.1,
            scope_button_size(ui, "Scan").y,
        );
        let remove = ui
            .add_enabled(
                scope.has_checked_custom(),
                egui::Button::new("-").min_size(remove_size),
            )
            .on_hover_text("Remove checked custom directories");
        if remove.clicked() {
            scope.remove_checked_custom();
        }

        checked_paths = scope.checked_paths();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(
                    !checked_paths.is_empty(),
                    egui::Button::new("Scan").min_size(scope_button_size(ui, "Scan")),
                )
                .clicked()
            {
                scan_request = Some(checked_paths.clone());
            }
        });
    });

    scan_request
}

fn show_scope_item_row(ui: &mut egui::Ui, item: &mut ScopeItem) {
    let row_size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
    let (rect, response) = ui.allocate_exact_size(row_size, egui::Sense::click());
    let response = response.on_hover_text(item.path.display().to_string());

    if response.clicked() {
        item.checked = !item.checked;
    }

    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }

    let checkbox_size = 14.0;
    let checkbox_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 7.0, rect.center().y),
        egui::vec2(checkbox_size, checkbox_size),
    );
    let checkbox_visuals = if response.hovered() {
        ui.visuals().widgets.hovered
    } else {
        ui.visuals().widgets.inactive
    };
    ui.painter().rect(
        checkbox_rect,
        2.0,
        checkbox_visuals.bg_fill,
        checkbox_visuals.bg_stroke,
        egui::StrokeKind::Inside,
    );
    if item.checked {
        let stroke = egui::Stroke::new(1.75, ui.visuals().selection.stroke.color);
        ui.painter().line_segment(
            [
                egui::pos2(checkbox_rect.left() + 3.0, checkbox_rect.center().y),
                egui::pos2(checkbox_rect.center().x - 1.0, checkbox_rect.bottom() - 3.0),
            ],
            stroke,
        );
        ui.painter().line_segment(
            [
                egui::pos2(checkbox_rect.center().x - 1.0, checkbox_rect.bottom() - 3.0),
                egui::pos2(checkbox_rect.right() - 3.0, checkbox_rect.top() + 3.0),
            ],
            stroke,
        );
    }

    let text = if item.custom {
        egui::RichText::new(&item.label).strong()
    } else {
        egui::RichText::new(&item.label)
    };
    let text_pos = egui::pos2(checkbox_rect.right() + 6.0, rect.center().y);
    ui.painter().text(
        text_pos,
        egui::Align2::LEFT_CENTER,
        text.text(),
        egui::FontId::proportional(13.0),
        ui.visuals().text_color(),
    );
}

fn scope_button_size(ui: &egui::Ui, text: &str) -> egui::Vec2 {
    let font_id = egui::TextStyle::Button.resolve(ui.style());
    let color = ui.visuals().text_color();
    let text_width = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font_id, color)
        .size()
        .x;
    let padding = ui.spacing().button_padding;
    let base = egui::vec2(
        (text_width + padding.x * 2.0).max(ui.spacing().interact_size.x),
        ui.spacing().interact_size.y,
    );
    base * 1.1
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
        let fs_path = tree.build_fs_path_for_id(id)?;
        let node = tree.node(id)?;
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
    let Some(node) = loaded.tree.node(id) else {
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
    let Some(node) = loaded.tree.node(id) else {
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
    if loaded.tree.contains(loaded.treemap.root_id, id) {
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
        && loaded.tree.node(previous).is_some()
    {
        loaded.treemap.root_id = previous;
        loaded.pane.selected = Some(previous);
        loaded.treemap.clear_layout();
        loaded.memory_relief.restart();
        return;
    }

    if let Some(parent_id) = loaded.tree.parent_id(loaded.treemap.root_id) {
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

fn execute_delete(loaded: &mut LoadedState, target: &DeleteTarget) {
    let result = if target.is_dir {
        std::fs::remove_dir_all(&target.fs_path)
    } else {
        std::fs::remove_file(&target.fs_path)
    };
    match result {
        Ok(()) => {
            if let Some(node) = loaded.tree.node(target.id) {
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
    prefs: &mut AppPrefs,
    prefs_changed: &mut bool,
    scope: &mut ScanScopeState,
    scope_logo: &egui::TextureHandle,
    scan_request: &mut Option<Vec<PathBuf>>,
    enabled: bool,
) {
    show_empty_status_bar(ctx, prefs, prefs_changed, enabled);

    let orientation = prefs.split_orientation;
    let mut show_table = |ui: &mut egui::Ui| {
        ui::tree_view::show_empty(ui, prefs, prefs_changed);
    };
    let mut show_scope = |ui: &mut egui::Ui| {
        show_scope_logo(ui, scope_logo, None);
        if let Some(paths) = show_scope_panel(ui, scope, enabled) {
            *scan_request = Some(paths);
        }
    };

    match orientation {
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
                    show_table_scope_column(ui, &mut show_table, &mut show_scope);
                });

            egui::CentralPanel::default().show(ctx, |ui| {
                ui::treemap_view::show_empty(ui);
            });
        }
        SplitOrientation::TopBottom => {
            egui::TopBottomPanel::top("file_table_top")
                .default_height(360.0)
                .min_height(180.0)
                .resizable(false)
                .frame(
                    egui::Frame::side_top_panel(ctx.style().as_ref())
                        .inner_margin(egui::Margin::from(8)),
                )
                .show(ctx, |ui| {
                    if !enabled {
                        ui.disable();
                    }
                    show_table_scope_row(ui, &mut show_table, &mut show_scope);
                });

            egui::CentralPanel::default().show(ctx, |ui| {
                ui::treemap_view::show_empty(ui);
            });
        }
    }
}

fn show_empty_status_bar(
    ctx: &egui::Context,
    prefs: &mut AppPrefs,
    prefs_changed: &mut bool,
    enabled: bool,
) {
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        if !enabled {
            ui.disable();
        }
        ui.horizontal(|ui| {
            ui.label("No scan");

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if split_layout_toggle(ui, &mut prefs.split_orientation) {
                    *prefs_changed = true;
                }
                if filename_truncation_toggle(ui, &mut prefs.filename_truncation) {
                    *prefs_changed = true;
                }
                if let Some(palette) = color_palette_control(ui, prefs.treemap_palette).selected {
                    prefs.treemap_palette = palette;
                    *prefs_changed = true;
                }

                ui.separator();
                if let Some(label_depth) = icon_depth_slider(
                    ui,
                    StatusIcon::FileLabels,
                    prefs.treemap_label_depth,
                    prefs.treemap_label_depth.max(1),
                    0..=5,
                    "File labels",
                ) {
                    prefs.treemap_label_depth = label_depth;
                    *prefs_changed = true;
                }
                if let Some(folder_depth) = icon_depth_slider(
                    ui,
                    StatusIcon::FolderLabels,
                    prefs.treemap_folder_depth,
                    prefs.treemap_folder_depth.max(1),
                    0..=6,
                    "Folder boxes",
                ) {
                    prefs.treemap_folder_depth = folder_depth;
                    *prefs_changed = true;
                }
            });
        });
    });
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
                        ui.heading("Scanning…");
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
        .show(ctx, |_ui| {});

    egui::CentralPanel::default().show(ctx, |ui| {
        ui.centered_and_justified(|ui| {
            ui.vertical_centered(|ui| {
                ui.heading("Scan Failed");
                ui.add_space(8.0);
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), message);
                ui.add_space(12.0);
                if ui.button("Open Folder…").clicked() {
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
            nsstring("AppleTree"),
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
                "Author: Seth Phillips\n\
                 \u{00A9} 2026 \u{2014} Licensed under GPL-3.0\n\n\
                 Forked from MacDirStat:
github.com/MichaelStromberg/macdirstat
                 ",
            ),
            nsstring("NSHumanReadableCopyright"),
        );
    }
}

/// Folder picker used by explicit rescan controls.
fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Select folder to scan")
        .pick_folder()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_paths_drop_nested_selections() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let nested = root.join("src");

        let paths = normalize_scope_paths(vec![nested, root.clone(), root.clone()]);

        assert_eq!(paths, vec![root]);
    }
}
