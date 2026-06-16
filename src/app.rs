use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Instant;

use eframe::egui;

use crate::format_size;
use crate::model::color::ColorMap;
use crate::model::tree::{FileTree, NodeId};
use crate::settings::{AppPrefs, SplitOrientation};
use crate::ui::file_icons::FileIconCache;
use crate::ui::{self, ActivePane, NodeCommand};

pub struct App {
    state: AppState,
    prefs: AppPrefs,
    prefs_changed: bool,
    #[cfg(target_os = "macos")]
    about_configured: bool,
}

enum AppState {
    WaitingForPicker { frames: u8 },
    Scanning { path: PathBuf, start_time: Instant },
    Loaded(Box<LoadedState>),
}

struct LoadedState {
    tree: FileTree,
    color_map: ColorMap,
    selected: Option<NodeId>,
    hovered: Option<NodeId>,
    expanded: BTreeSet<NodeId>,
    deleted_nodes: BTreeSet<NodeId>,
    deleted_outlines: BTreeSet<NodeId>,
    treemap_root: NodeId,
    zoom_history: Vec<NodeId>,
    active_pane: ActivePane,
    status_message: Option<String>,
    scan_time_ms: f64,
    cached_layout_rect: Option<egui::Rect>,
    treemap_cache: ui::treemap_view::TreemapCache,
    treemap_texture: Option<egui::TextureHandle>,
    pending_scan: Option<PathBuf>,
    file_icons: FileIconCache,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial_path: Option<String>) -> Self {
        #[cfg(target_os = "macos")]
        use_macos_system_font(&cc.egui_ctx);

        let mut app = Self {
            state: AppState::WaitingForPicker { frames: 2 },
            prefs: AppPrefs::load(),
            prefs_changed: false,
            #[cfg(target_os = "macos")]
            about_configured: false,
        };
        if let Some(path) = initial_path {
            app.start_scan(PathBuf::from(path));
        }
        app
    }

    fn start_scan(&mut self, path: PathBuf) {
        self.state = AppState::Scanning {
            path,
            start_time: Instant::now(),
        };
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

        // Scanning is synchronous — blocks the UI thread
        if let AppState::Scanning {
            ref path,
            start_time,
        } = self.state
        {
            let tree = FileTree::scan(path);
            let scan_time_ms = start_time.elapsed().as_secs_f64() * 1000.0;
            let color_map = ColorMap::from_extensions(&tree.extensions);
            let mut expanded = BTreeSet::new();
            expanded.insert(tree.root.id);
            let treemap_root = tree.root.id;
            self.state = AppState::Loaded(Box::new(LoadedState {
                tree,
                color_map,
                selected: None,
                hovered: None,
                expanded,
                deleted_nodes: BTreeSet::new(),
                deleted_outlines: BTreeSet::new(),
                treemap_root,
                zoom_history: Vec::new(),
                active_pane: ActivePane::Table,
                status_message: None,
                scan_time_ms,
                cached_layout_rect: None,
                treemap_cache: ui::treemap_view::TreemapCache::default(),
                treemap_texture: None,
                pending_scan: None,
                file_icons: FileIconCache::default(),
            }));
        }

        match &mut self.state {
            AppState::WaitingForPicker { frames } => {
                show_empty_panes(ctx);

                if *frames > 0 {
                    *frames -= 1;
                    ctx.request_repaint();
                } else if *frames == 0 {
                    // Prevent re-entry after the blocking dialog returns
                    *frames = u8::MAX;
                    let result = pick_folder_at_home();
                    if let Some(path) = result {
                        self.start_scan(path);
                    } else {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                // frames == u8::MAX: dialog was dismissed, waiting for close
            }
            AppState::Scanning { .. } => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.heading("Scanning...");
                    });
                });
            }
            AppState::Loaded(loaded) => {
                loaded.hovered = None;
                let mut command = handle_delete(loaded, ctx);
                if let Some(ui_command) =
                    loaded
                        .as_mut()
                        .show_panels(ctx, &mut self.prefs, &mut self.prefs_changed)
                {
                    command = Some(ui_command);
                }
                if let Some(command) = command {
                    execute_node_command(loaded, ctx, command);
                }
            }
        }

        // Handle ⌘O and pending scans from breadcrumb menu (outside the match
        // to avoid borrow conflicts with self.state).
        if let AppState::Loaded(loaded) = &mut self.state {
            let cmd_o = ctx.input(|i| i.key_pressed(egui::Key::O) && i.modifiers.command);
            let path = if cmd_o {
                pick_folder()
            } else {
                loaded.pending_scan.take()
            };
            if let Some(path) = path {
                self.start_scan(path);
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
    fn show_panels(
        &mut self,
        ctx: &egui::Context,
        prefs: &mut AppPrefs,
        prefs_changed: &mut bool,
    ) -> Option<NodeCommand> {
        let mut command = None;
        self.show_status_bar(ctx, prefs, prefs_changed, &mut command);
        self.show_main_layout(ctx, prefs, prefs_changed, &mut command);
        command
    }

    fn show_status_bar(
        &mut self,
        ctx: &egui::Context,
        prefs: &mut AppPrefs,
        prefs_changed: &mut bool,
        command: &mut Option<NodeCommand>,
    ) {
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{} Files",
                    format_file_count(self.tree.root.file_count)
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
                    let has_selection = self.selected.is_some();

                    let trash_text = egui::RichText::new("\u{1F5D1}").color(if has_selection {
                        egui::Color32::from_rgb(220, 60, 60)
                    } else {
                        egui::Color32::from_rgb(160, 120, 120)
                    });
                    let trash_btn = ui.add_enabled(has_selection, egui::Button::new(trash_text));
                    if trash_btn.clicked()
                        && let Some(id) = self.selected
                    {
                        *command = Some(NodeCommand::Delete { id, confirm: true });
                    }

                    let reveal_btn = ui.add_enabled(
                        has_selection,
                        egui::Button::new("\u{1F50D} Reveal in Finder"),
                    );
                    if reveal_btn.clicked()
                        && let Some(id) = self.selected
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
                        self.cached_layout_rect = None;
                        self.treemap_cache.clear();
                        self.treemap_texture = None;
                    }
                });
            });
        });
    }

    fn show_main_layout(
        &mut self,
        ctx: &egui::Context,
        prefs: &mut AppPrefs,
        prefs_changed: &mut bool,
        command: &mut Option<NodeCommand>,
    ) {
        match prefs.split_orientation {
            SplitOrientation::LeftRight => {
                egui::SidePanel::left("file_table")
                    .default_width(520.0)
                    .min_width(360.0)
                    .show_separator_line(false)
                    .frame(
                        egui::Frame::side_top_panel(ctx.style().as_ref())
                            .inner_margin(egui::Margin::from(8)),
                    )
                    .show(ctx, |ui| {
                        if let Some(cmd) = self.show_file_table(ui, prefs, prefs_changed) {
                            *command = Some(cmd);
                        }
                    });

                egui::CentralPanel::default().show(ctx, |ui| {
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
                    .resizable(true)
                    .frame(
                        egui::Frame::side_top_panel(ctx.style().as_ref())
                            .inner_margin(egui::Margin::from(8)),
                    )
                    .show(ctx, |ui| {
                        if let Some(cmd) = self.show_file_table(ui, prefs, prefs_changed) {
                            *command = Some(cmd);
                        }
                    });
                let new_height = table_response.response.rect.height();
                if (new_height - prefs.top_bottom_table_height).abs() > 1.0 {
                    prefs.top_bottom_table_height = new_height;
                    *prefs_changed = true;
                }

                egui::CentralPanel::default().show(ctx, |ui| {
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
            &mut self.selected,
            &mut self.hovered,
            &mut self.expanded,
            &self.deleted_nodes,
            &self.deleted_outlines,
            &mut self.active_pane,
            prefs,
            prefs_changed,
            &mut self.file_icons,
        )
    }

    fn show_treemap(&mut self, ui: &mut egui::Ui, prefs: &AppPrefs) -> Option<NodeCommand> {
        ui::treemap_view::show(
            ui,
            &mut self.tree,
            self.treemap_root,
            &mut self.selected,
            &mut self.hovered,
            &mut self.active_pane,
            &self.color_map,
            &self.deleted_nodes,
            &self.deleted_outlines,
            prefs,
            &mut self.cached_layout_rect,
            &mut self.treemap_cache,
            &mut self.treemap_texture,
        )
    }

    fn show_breadcrumb_area(&mut self, ui: &mut egui::Ui) {
        let mut new_scan_path: Option<PathBuf> = None;
        self.show_breadcrumb(ui, &mut new_scan_path);
        ui.add_space(2.0);
        if let Some(path) = new_scan_path {
            self.pending_scan = Some(path);
        }
    }

    fn status_path(&self) -> Option<String> {
        self.hovered
            .or(self.selected)
            .and_then(|id| self.tree.full_display_path_for_id(id))
    }

    fn show_breadcrumb(&self, ui: &mut egui::Ui, new_scan_path: &mut Option<PathBuf>) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
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
    let id = loaded.selected?;
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
    }
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
    if loaded.treemap_root == id {
        return;
    }
    if node_contains_id(&loaded.tree, loaded.treemap_root, id) {
        loaded.zoom_history.push(loaded.treemap_root);
    } else {
        loaded.zoom_history.clear();
        if loaded.tree.root.id != id {
            loaded.zoom_history.push(loaded.tree.root.id);
        }
    }
    loaded.treemap_root = id;
    loaded.selected = Some(id);
    loaded.cached_layout_rect = None;
    loaded.treemap_cache.clear();
    loaded.treemap_texture = None;
}

fn zoom_out_treemap(loaded: &mut LoadedState) {
    if let Some(previous) = loaded.zoom_history.pop()
        && loaded.tree.root.resolve_id(previous).is_some()
    {
        loaded.treemap_root = previous;
        loaded.selected = Some(previous);
        loaded.cached_layout_rect = None;
        loaded.treemap_cache.clear();
        loaded.treemap_texture = None;
        return;
    }

    if let Some(parent_id) = parent_id_for_node(&loaded.tree, loaded.treemap_root) {
        loaded.treemap_root = parent_id;
        loaded.selected = Some(parent_id);
        loaded.cached_layout_rect = None;
        loaded.treemap_cache.clear();
        loaded.treemap_texture = None;
        return;
    }

    if let Some(path) = loaded.tree.build_fs_path_for_id(loaded.treemap_root)
        && let Some(parent) = path.parent()
        && parent != path
    {
        loaded.pending_scan = Some(parent.to_path_buf());
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
            loaded.deleted_nodes.insert(target.id);
            if let Some(node) = loaded.tree.root.resolve_id(target.id) {
                let before = loaded.deleted_outlines.len();
                collect_deleted_outline_ids(node, &mut loaded.deleted_outlines);
                if loaded.deleted_outlines.len() == before {
                    loaded.deleted_outlines.insert(target.id);
                }
            }
            loaded.hovered = None;
            loaded.status_message = Some(format!("Deleted {}", target.name()));
        }
        Err(e) => {
            loaded.status_message = Some(format!("Failed to delete {}: {}", target.name(), e));
        }
    }
}

fn collect_deleted_outline_ids(
    node: &crate::model::tree::FileNode,
    outlines: &mut BTreeSet<NodeId>,
) {
    if node.is_dir {
        for child in node.children.iter() {
            collect_deleted_outline_ids(child, outlines);
        }
    } else {
        outlines.insert(node.id);
    }
}

/// Render the three-pane layout with empty panels (same IDs as Loaded state).
fn show_empty_panes(ctx: &egui::Context) {
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |_ui| {});

    egui::SidePanel::left("file_table")
        .default_width(520.0)
        .min_width(360.0)
        .show_separator_line(false)
        .frame(
            egui::Frame::side_top_panel(ctx.style().as_ref()).inner_margin(egui::Margin::from(8)),
        )
        .show(ctx, |ui| {
            ui::tree_view::show_branding(ui);
        });

    egui::CentralPanel::default().show(ctx, |_ui| {});
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

fn format_file_count(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        // Format with comma separators
        let s = count.to_string();
        let mut result = String::new();
        for (i, c) in s.chars().rev().enumerate() {
            if i > 0 && i % 3 == 0 {
                result.push(',');
            }
            result.push(c);
        }
        result.chars().rev().collect()
    } else {
        count.to_string()
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
