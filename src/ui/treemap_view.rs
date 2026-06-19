use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use egui::{Color32, ColorImage, Rect, Sense, TextureHandle, TextureOptions, pos2, vec2};
use treemap::{Mappable, TreemapLayout};

use crate::format_size;
use crate::model::color::{
    ColorMap, PALETTE_BRIGHTNESS, folder_content_color, folder_frame_color, folder_header_color,
    folder_shell_color,
};
use crate::model::tree::{FileNode, FileTree, NodeId};
use crate::settings::{AppPrefs, FilenameTruncation, TreemapPalette};
use crate::ui::{self, ActivePane, DeletionOverlay, NodeCommand, PaneState};

/// Cushion surface coefficients: [a_x, a_y, c_x, c_y]
/// z(x,y) = a_x*x^2 + a_y*y^2 + c_x*x + c_y*y
type Surface = [f64; 4];

/// Leaf data collected during layout: (treemap rect, surface coefficients, color)
struct CushionLeaf {
    rect: treemap::Rect,
    surface: Surface,
    color: Color32,
}

#[derive(Default)]
pub struct TreemapCache {
    rects: Vec<treemap::Rect>,
}

impl TreemapCache {
    pub fn clear(&mut self) {
        self.rects.clear();
    }

    fn rebuild_layout(
        &mut self,
        root: &FileNode,
        bounds: treemap::Rect,
        prefs: &AppPrefs,
        shrunk_nodes: &BTreeSet<NodeId>,
    ) {
        self.rects.clear();
        layout_node(root, bounds, 0, prefs, self, shrunk_nodes);
    }

    fn rect(&self, id: NodeId) -> Option<&treemap::Rect> {
        let rect = self.rects.get(id as usize)?;
        (rect.w > 0.0 && rect.h > 0.0).then_some(rect)
    }

    fn egui_rect(&self, id: NodeId) -> Option<Rect> {
        self.rect(id).map(to_egui_rect)
    }

    fn insert_rect(&mut self, id: NodeId, rect: treemap::Rect) {
        let index = id as usize;
        if self.rects.len() <= index {
            self.rects.resize(index + 1, treemap::Rect::new());
        }
        self.rects[index] = rect;
    }
}

pub struct TreemapState {
    pub root_id: NodeId,
    pub zoom_history: Vec<NodeId>,
    pub shrunk_nodes: BTreeSet<NodeId>,
    cached_layout_rect: Option<Rect>,
    cached_palette: Option<TreemapPalette>,
    cache: TreemapCache,
    texture: Option<TextureHandle>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TreemapRenderBench {
    pub(crate) layout: Duration,
    pub(crate) render: Duration,
    pub(crate) total: Duration,
    pub(crate) leaves: usize,
    pub(crate) pixels: usize,
}

impl TreemapState {
    pub fn new(root_id: NodeId) -> Self {
        Self {
            root_id,
            zoom_history: Vec::new(),
            shrunk_nodes: BTreeSet::new(),
            cached_layout_rect: None,
            cached_palette: None,
            cache: TreemapCache::default(),
            texture: None,
        }
    }

    pub fn clear_layout(&mut self) {
        self.cached_layout_rect = None;
        self.cached_palette = None;
        self.cache.clear();
        self.texture = None;
    }

    pub fn clear_texture(&mut self) {
        self.cached_palette = None;
        self.texture = None;
    }

    pub fn is_shrunk(&self, id: NodeId) -> bool {
        self.shrunk_nodes.contains(&id)
    }

    pub fn toggle_shrink(&mut self, id: NodeId) -> bool {
        let is_shrunk = if self.shrunk_nodes.remove(&id) {
            false
        } else {
            self.shrunk_nodes.insert(id);
            true
        };
        self.clear_layout();
        is_shrunk
    }
}

struct HitNode<'a> {
    node: &'a FileNode,
}

#[derive(Clone)]
struct LayoutItem {
    child_index: usize,
    size: f64,
    bounds: treemap::Rect,
}

const CUSHION_HEIGHT: f64 = 0.38;
const CUSHION_SCALE: f64 = 0.91;
const FOLDER_HEADER_H: f64 = 9.0;
const FOLDER_PAD: f64 = 1.0;
const MIN_HEADER_W: f64 = 42.0;
const MIN_HEADER_H: f64 = FOLDER_HEADER_H + FOLDER_PAD * 2.0 + 6.0;
const LABEL_FONT_SIZE: f32 = 8.0;
const FILE_LABEL_MIN_W: f32 = 72.0;
const FILE_LABEL_MIN_H: f32 = 18.0;
const FILE_LABEL_MIN_AREA: f32 = 4_200.0;
const SHRUNK_MAX_FRACTION: f64 = 0.10;

// Lighting parameters (WinDirStat defaults)
const AMBIENT: f64 = 0.13;
const DIFFUSE: f64 = 1.0 - AMBIENT;
const BRIGHTNESS_FACTOR: f64 = 0.88 / PALETTE_BRIGHTNESS;

fn treemap_canvas_fill(visuals: &egui::Visuals) -> Color32 {
    visuals.extreme_bg_color
}

fn treemap_folder_label_color(dark_mode: bool, deleted: bool) -> Color32 {
    if deleted {
        if dark_mode {
            Color32::from_rgb(255, 82, 82)
        } else {
            Color32::from_rgb(190, 35, 35)
        }
    } else if dark_mode {
        Color32::from_rgb(235, 235, 235)
    } else {
        Color32::from_rgb(35, 35, 35)
    }
}

fn treemap_file_label_color(deleted: bool) -> Color32 {
    if deleted {
        Color32::from_rgb(255, 70, 70)
    } else {
        Color32::from_rgb(235, 235, 235)
    }
}

impl Mappable for LayoutItem {
    fn size(&self) -> f64 {
        self.size
    }

    fn bounds(&self) -> &treemap::Rect {
        &self.bounds
    }

    fn set_bounds(&mut self, bounds: treemap::Rect) {
        self.bounds = bounds;
    }
}

pub fn show(
    ui: &mut egui::Ui,
    tree: &FileTree,
    pane: &mut PaneState,
    color_map: &ColorMap,
    deleted: &DeletionOverlay,
    prefs: &AppPrefs,
    palette: TreemapPalette,
    state: &mut TreemapState,
) -> Option<NodeCommand> {
    let mut command = None;
    let available = ui.available_size();
    let (response, painter) = ui.allocate_painter(available, Sense::click_and_drag());
    let canvas = response.rect;

    if canvas.width() < 2.0 || canvas.height() < 2.0 {
        return None;
    }

    let w = canvas.width();
    let h = canvas.height();
    let display_root_id = if tree.node(state.root_id).is_some() {
        state.root_id
    } else {
        tree.root.id
    };

    // Check if we need to re-layout and re-render the cushion texture.
    // Compare the full canvas rect (position + size) so that side panel
    // resizing or window moves trigger a re-layout.
    let needs_update = if let Some(cached) = state.cached_layout_rect {
        (canvas.left() - cached.left()).abs() > 1.0
            || (canvas.top() - cached.top()).abs() > 1.0
            || (w - cached.width()).abs() > 1.0
            || (h - cached.height()).abs() > 1.0
            || state.cached_palette != Some(palette)
    } else {
        true
    };

    if needs_update {
        let bounds = treemap::Rect::from_points(
            canvas.left() as f64,
            canvas.top() as f64,
            w as f64,
            h as f64,
        );

        let mut leaves = Vec::new();
        let surface = [0.0f64; 4];
        if let Some(root) = tree.node(display_root_id) {
            state
                .cache
                .rebuild_layout(root, bounds, prefs, &state.shrunk_nodes);
            collect_cushion_leaves(
                root,
                &state.cache,
                surface,
                CUSHION_HEIGHT,
                true,
                color_map,
                palette,
                &mut leaves,
            );
        }

        // Render cushion texture
        let pw = w as usize;
        let ph = h as usize;
        if pw > 0 && ph > 0 {
            let image = render_cushion_image(pw, ph, canvas, &leaves);
            let texture = ui
                .ctx()
                .load_texture("treemap_cushion", image, TextureOptions::NEAREST);
            state.texture = Some(texture);
        }

        state.cached_layout_rect = Some(canvas);
        state.cached_palette = Some(palette);
    }

    let Some(display_root) = tree.node(display_root_id) else {
        return command;
    };

    let dark_mode = ui.visuals().dark_mode;
    painter.rect_filled(canvas, 0.0, treemap_canvas_fill(ui.visuals()));
    paint_folder_backgrounds(
        &painter,
        display_root,
        &state.cache,
        0,
        prefs,
        palette,
        dark_mode,
    );

    // Paint the cached file-cushion texture over transparent folder content.
    if let Some(tex) = &state.texture {
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        painter.image(tex.id(), canvas, uv, Color32::WHITE);
    }

    // Handle clicks
    if response.clicked()
        && let Some(pos) = response.interact_pointer_pos()
    {
        if let Some(hit) = find_node_at(display_root, &state.cache, pos) {
            pane.select(hit.node.id, ActivePane::Treemap);
        } else {
            pane.selected = None;
        }
    }

    let menu_target_key = response.id.with("treemap_context_target");
    if response.secondary_clicked() {
        let target = ui
            .ctx()
            .pointer_latest_pos()
            .filter(|pos| canvas.contains(*pos))
            .and_then(|pos| find_node_at(display_root, &state.cache, pos).map(|hit| hit.node.id));

        ui.ctx().data_mut(|data| {
            if let Some(id) = target {
                data.insert_temp(menu_target_key, id);
            } else {
                data.remove::<NodeId>(menu_target_key);
            }
        });

        if let Some(id) = target {
            pane.select(id, ActivePane::Treemap);
        }
    }

    let menu_target = ui
        .ctx()
        .data_mut(|data| data.get_temp::<NodeId>(menu_target_key));
    if let Some(id) = menu_target {
        response.context_menu(|ui| {
            let can_zoom_in = tree.node(id).is_some_and(|node| node.is_dir);
            ui::node_context_menu(
                ui,
                id,
                can_zoom_in,
                "Zoom In",
                state.is_shrunk(id),
                &mut command,
            );
        });
    }

    // Hover tooltip
    if !ui.ctx().any_popup_open()
        && !response.secondary_clicked()
        && let Some(pos) = response.hover_pos()
        && let Some(hit) = find_node_at(display_root, &state.cache, pos)
    {
        pane.hovered = Some(hit.node.id);
        let full_path = tree
            .full_display_path_for_id(hit.node.id)
            .unwrap_or_else(|| hit.node.name.to_string());
        let tip = format!("{}\n{}", full_path, format_size(hit.node.size));
        egui::Tooltip::always_open(
            ui.ctx().clone(),
            ui.layer_id(),
            response.id.with("tip"),
            egui::PopupAnchor::Pointer,
        )
        .show(|ui| {
            ui.set_max_width(560.0);
            ui.label(tip);
        });
    }

    paint_folder_labels_and_borders(
        &painter,
        display_root,
        &state.cache,
        0,
        prefs,
        palette,
        deleted,
        dark_mode,
    );
    paint_file_labels(&painter, display_root, &state.cache, prefs, deleted);
    paint_shrink_markers(
        &painter,
        display_root,
        &state.cache,
        0,
        prefs,
        &state.shrunk_nodes,
    );

    if let Some(hover_id) = pane.hovered
        && let Some(r) = state.cache.egui_rect(hover_id)
    {
        painter.rect_stroke(
            r,
            0.0,
            egui::Stroke::new(1.0, Color32::WHITE),
            egui::StrokeKind::Outside,
        );
    }

    // Draw selection highlight
    if let Some(sel_id) = pane.selected
        && let Some(r) = state.cache.egui_rect(sel_id)
    {
        painter.rect_stroke(
            r,
            0.0,
            egui::Stroke::new(2.0, Color32::WHITE),
            egui::StrokeKind::Outside,
        );
    }

    paint_deleted_outlines(&painter, &state.cache, deleted);

    command
}

pub fn show_empty(ui: &mut egui::Ui) {
    let available = ui.available_size();
    let (rect, _) = ui.allocate_exact_size(available, Sense::hover());
    if rect.width() < 2.0 || rect.height() < 2.0 {
        return;
    }

    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, treemap_canvas_fill(ui.visuals()));
    painter.rect_stroke(
        rect.shrink(0.5),
        0.0,
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
}

pub(crate) fn benchmark_render(
    tree: &FileTree,
    color_map: &ColorMap,
    prefs: &AppPrefs,
    width: usize,
    height: usize,
) -> TreemapRenderBench {
    let width = width.max(1);
    let height = height.max(1);
    let bounds = treemap::Rect::from_points(0.0, 0.0, width as f64, height as f64);
    let canvas = Rect::from_min_size(pos2(0.0, 0.0), vec2(width as f32, height as f32));
    let mut cache = TreemapCache::default();
    let mut leaves = Vec::new();

    let total_start = Instant::now();
    let layout_start = Instant::now();
    cache.rebuild_layout(&tree.root, bounds, prefs, &BTreeSet::new());
    collect_cushion_leaves(
        &tree.root,
        &cache,
        [0.0; 4],
        CUSHION_HEIGHT,
        true,
        color_map,
        prefs.treemap_palette,
        &mut leaves,
    );
    let layout = layout_start.elapsed();

    let render_start = Instant::now();
    let image = render_cushion_image(width, height, canvas, &leaves);
    let render = render_start.elapsed();
    std::hint::black_box(image);

    TreemapRenderBench {
        layout,
        render,
        total: total_start.elapsed(),
        leaves: leaves.len(),
        pixels: width * height,
    }
}

fn layout_node(
    node: &FileNode,
    bounds: treemap::Rect,
    depth: usize,
    prefs: &AppPrefs,
    cache: &mut TreemapCache,
    shrunk_nodes: &BTreeSet<NodeId>,
) {
    cache.insert_rect(node.id, bounds);

    if node.children.is_empty() || node.size == 0 {
        return;
    }

    let content = folder_content_rect(&bounds, depth, prefs);
    if content.w < 1.0 || content.h < 1.0 {
        return;
    }

    let visual_sizes = visual_child_sizes(&node.children, shrunk_nodes);
    let mut items: Vec<LayoutItem> = visual_sizes
        .into_iter()
        .enumerate()
        .map(|(child_index, size)| LayoutItem {
            child_index,
            size,
            bounds: treemap::Rect::new(),
        })
        .collect();

    let layout = TreemapLayout::new();
    layout.layout_items(&mut items, content);

    for item in items.iter() {
        let b: treemap::Rect = *item.bounds();
        if let Some(child) = node.children.get(item.child_index) {
            layout_node(child, b, depth + 1, prefs, cache, shrunk_nodes);
        }
    }
}

fn visual_child_sizes(children: &[FileNode], shrunk_nodes: &BTreeSet<NodeId>) -> Vec<f64> {
    let mut weights = children
        .iter()
        .map(|child| child.size as f64)
        .collect::<Vec<_>>();

    let has_unshrunk_weight = children
        .iter()
        .any(|child| !shrunk_nodes.contains(&child.id) && child.size > 0);
    if !has_unshrunk_weight {
        return weights;
    }

    for _ in 0..48 {
        let total = weights.iter().sum::<f64>();
        if total <= f64::EPSILON {
            break;
        }

        let cap = total * SHRUNK_MAX_FRACTION;
        let mut changed = false;
        for (index, child) in children.iter().enumerate() {
            if shrunk_nodes.contains(&child.id) && weights[index] > cap {
                weights[index] = cap;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    weights
}

fn folder_has_chrome(rect: &treemap::Rect, depth: usize, prefs: &AppPrefs) -> bool {
    depth <= prefs.treemap_folder_depth && rect.w >= MIN_HEADER_W && rect.h >= MIN_HEADER_H
}

fn folder_header_rect(
    rect: &treemap::Rect,
    depth: usize,
    prefs: &AppPrefs,
) -> Option<treemap::Rect> {
    folder_has_chrome(rect, depth, prefs).then_some(treemap::Rect {
        x: rect.x + FOLDER_PAD,
        y: rect.y + FOLDER_PAD,
        w: (rect.w - FOLDER_PAD * 2.0).max(0.0),
        h: FOLDER_HEADER_H,
    })
}

fn folder_content_rect(rect: &treemap::Rect, depth: usize, prefs: &AppPrefs) -> treemap::Rect {
    if folder_has_chrome(rect, depth, prefs) {
        let y = rect.y + FOLDER_PAD + FOLDER_HEADER_H + FOLDER_PAD;
        treemap::Rect {
            x: rect.x + FOLDER_PAD,
            y,
            w: (rect.w - FOLDER_PAD * 2.0).max(0.0),
            h: (rect.y + rect.h - y - FOLDER_PAD).max(0.0),
        }
    } else if depth <= prefs.treemap_folder_depth {
        treemap::Rect {
            x: rect.x + 1.0,
            y: rect.y + 1.0,
            w: (rect.w - 2.0).max(0.0),
            h: (rect.h - 2.0).max(0.0),
        }
    } else {
        *rect
    }
}

fn add_ridge(surface: &mut Surface, rect: &treemap::Rect, h: f64) {
    if rect.w < 0.001 || rect.h < 0.001 {
        return;
    }
    let h4 = 4.0 * h;
    let wf = h4 / rect.w;
    surface[0] -= wf; // a_x
    surface[2] += wf * (2.0 * rect.x + rect.w); // c_x
    let hf = h4 / rect.h;
    surface[1] -= hf; // a_y
    surface[3] += hf * (2.0 * rect.y + rect.h); // c_y
}

fn collect_cushion_leaves(
    node: &FileNode,
    cache: &TreemapCache,
    mut surface: Surface,
    h: f64,
    is_root: bool,
    color_map: &ColorMap,
    palette: TreemapPalette,
    leaves: &mut Vec<CushionLeaf>,
) {
    let Some(rect) = cache.rect(node.id) else {
        return;
    };

    // Add ridge for this node (skip root per WinDirStat)
    if !is_root {
        add_ridge(&mut surface, rect, h);
    }

    if node.children.is_empty() {
        if node.is_dir {
            return;
        }
        let color = color_map.get_treemap(node.extension(), palette);
        leaves.push(CushionLeaf {
            rect: *rect,
            surface,
            color,
        });
    } else {
        let child_h = h * CUSHION_SCALE;
        for child in node.children.iter() {
            collect_cushion_leaves(
                child, cache, surface, child_h, false, color_map, palette, leaves,
            );
        }
    }
}

fn render_cushion_image(pw: usize, ph: usize, canvas: Rect, leaves: &[CushionLeaf]) -> ColorImage {
    let mut image = ColorImage::new([pw, ph], vec![Color32::TRANSPARENT; pw * ph]);

    // Precompute normalized light direction
    let (lx, ly, lz) = {
        let len = (1.0f64 + 1.0 + 100.0).sqrt();
        (-1.0 / len, -1.0 / len, 10.0 / len)
    };

    let canvas_left = canvas.left() as f64;
    let canvas_top = canvas.top() as f64;

    for leaf in leaves {
        let r = &leaf.rect;
        if r.w < 0.5 || r.h < 0.5 {
            continue;
        }

        // Convert treemap coords to pixel coords (they're in canvas space).
        // Clamp to 0 before casting to usize to prevent negative-to-unsigned wrapping.
        let left = (r.x - canvas_left).max(0.0) as usize;
        let top = (r.y - canvas_top).max(0.0) as usize;
        let right = ((r.x + r.w - canvas_left).max(0.0) as usize + 1).min(pw);
        let bottom = ((r.y + r.h - canvas_top).max(0.0) as usize + 1).min(ph);

        if left >= right || top >= bottom {
            continue;
        }

        let s = &leaf.surface;
        let col_r = leaf.color.r() as f64;
        let col_g = leaf.color.g() as f64;
        let col_b = leaf.color.b() as f64;

        for iy in top..bottom {
            // The surface coords are in canvas pixel space
            let sy = canvas_top + iy as f64 + 0.5;
            let ny = -(2.0 * s[1] * sy + s[3]);
            let ny_term = ny * ny + 1.0;
            let row_offset = iy * pw;
            let row = &mut image.pixels[row_offset + left..row_offset + right];
            let sx = canvas_left + left as f64 + 0.5;
            let mut nx = -(2.0 * s[0] * sx + s[2]);
            let nx_step = -2.0 * s[0];

            for dest in row {
                let cosa = (nx * lx + ny * ly + lz) / (nx * nx + ny_term).sqrt();
                let cosa = cosa.clamp(0.0, 1.0);

                let pixel = (DIFFUSE * cosa + AMBIENT) * BRIGHTNESS_FACTOR;

                let pr = (col_r * pixel).min(255.0) as u8;
                let pg = (col_g * pixel).min(255.0) as u8;
                let pb = (col_b * pixel).min(255.0) as u8;

                *dest = Color32::from_rgb(pr, pg, pb);
                nx += nx_step;
            }
        }
    }

    image
}

fn find_node_at<'a>(
    node: &'a FileNode,
    cache: &TreemapCache,
    pos: egui::Pos2,
) -> Option<HitNode<'a>> {
    let r = cache.egui_rect(node.id)?;
    if !r.contains(pos) {
        return None;
    }

    for child in node.children.iter() {
        if let Some(hit) = find_node_at(child, cache, pos) {
            return Some(hit);
        }
    }

    Some(HitNode { node })
}

fn paint_folder_backgrounds(
    painter: &egui::Painter,
    node: &FileNode,
    cache: &TreemapCache,
    depth: usize,
    prefs: &AppPrefs,
    palette: TreemapPalette,
    dark_mode: bool,
) {
    if !node.is_dir {
        return;
    }

    let Some(layout_rect) = cache.rect(node.id) else {
        return;
    };
    let rect = to_egui_rect(layout_rect);
    if depth <= prefs.treemap_folder_depth {
        painter.rect_filled(rect, 0.0, folder_shell_color(palette, dark_mode));

        if let Some(header) = folder_header_rect(layout_rect, depth, prefs) {
            painter.rect_filled(
                to_egui_rect(&header),
                0.0,
                folder_header_color(palette, dark_mode),
            );
        }

        let content = folder_content_rect(layout_rect, depth, prefs);
        painter.rect_filled(
            to_egui_rect(&content),
            0.0,
            folder_content_color(palette, dark_mode),
        );
    }

    for child in node.children.iter() {
        paint_folder_backgrounds(painter, child, cache, depth + 1, prefs, palette, dark_mode);
    }
}

fn paint_folder_labels_and_borders(
    painter: &egui::Painter,
    node: &FileNode,
    cache: &TreemapCache,
    depth: usize,
    prefs: &AppPrefs,
    palette: TreemapPalette,
    deleted: &DeletionOverlay,
    dark_mode: bool,
) {
    if !node.is_dir {
        return;
    }

    let Some(layout_rect) = cache.rect(node.id) else {
        return;
    };
    let rect = to_egui_rect(layout_rect);
    if depth <= prefs.treemap_folder_depth {
        if let Some(header) = folder_header_rect(layout_rect, depth, prefs) {
            let header_rect = to_egui_rect(&header);
            let label_pos = pos2(header_rect.left() + 2.0, header_rect.center().y);
            let font_id = egui::FontId::monospace(LABEL_FONT_SIZE);
            let color = treemap_folder_label_color(dark_mode, deleted.is_node_deleted(node.id));
            let label = node_size_label(
                painter,
                node,
                prefs.filename_truncation,
                font_id.clone(),
                color,
                (header_rect.right() - label_pos.x - 2.0).max(0.0),
            );
            painter.with_clip_rect(header_rect).text(
                label_pos,
                egui::Align2::LEFT_CENTER,
                label,
                font_id,
                color,
            );
        }
        painter.rect_stroke(
            rect,
            0.0,
            egui::Stroke::new(1.0, folder_frame_color(palette, dark_mode)),
            egui::StrokeKind::Inside,
        );
    }

    for child in node.children.iter() {
        paint_folder_labels_and_borders(
            painter,
            child,
            cache,
            depth + 1,
            prefs,
            palette,
            deleted,
            dark_mode,
        );
    }
}

fn paint_file_labels(
    painter: &egui::Painter,
    node: &FileNode,
    cache: &TreemapCache,
    prefs: &AppPrefs,
    deleted: &DeletionOverlay,
) {
    if node.is_dir {
        for child in node.children.iter() {
            paint_file_labels(painter, child, cache, prefs, deleted);
        }
        return;
    }

    if prefs.treemap_label_depth == 0 {
        return;
    }

    let Some(rect) = cache.egui_rect(node.id) else {
        return;
    };
    let Some(header) = file_label_header_rect(rect, prefs.treemap_label_depth) else {
        return;
    };
    // Draw a semi-transparent dark background behind file labels for better readability.
    // painter.rect_filled(
    //     header,
    //     0.0,
    //     Color32::from_rgba_unmultiplied(50, 50, 50, 230),
    // );
    let label_pos = pos2(header.left() + 2.0, header.center().y);
    let font_id = egui::FontId::monospace(LABEL_FONT_SIZE);
    let color = treemap_file_label_color(deleted.is_node_deleted(node.id));
    let label = node_size_label(
        painter,
        node,
        prefs.filename_truncation,
        font_id.clone(),
        color,
        (header.right() - label_pos.x - 2.0).max(0.0),
    );
    painter.with_clip_rect(header).text(
        label_pos,
        egui::Align2::LEFT_CENTER,
        label,
        font_id,
        color,
    );
}

fn node_size_label(
    painter: &egui::Painter,
    node: &FileNode,
    truncation: FilenameTruncation,
    font_id: egui::FontId,
    color: Color32,
    max_width: f32,
) -> String {
    let size_label = format!(" ({})", format_size(node.size));
    let full_label = format!("{}{}", node.name, size_label);
    if text_width(painter, &full_label, &font_id, color) <= max_width {
        return full_label;
    }

    let size_width = text_width(painter, &size_label, &font_id, color);
    let name_max_width = max_width - size_width;
    if name_max_width <= 0.0 {
        return truncate_text_to_width(
            painter,
            &full_label,
            truncation,
            &font_id,
            color,
            max_width,
        );
    }

    let name = truncate_text_to_width(
        painter,
        &node.name,
        truncation,
        &font_id,
        color,
        name_max_width,
    );
    if name.is_empty() {
        truncate_text_to_width(painter, &full_label, truncation, &font_id, color, max_width)
    } else {
        format!("{name}{size_label}")
    }
}

fn truncate_text_to_width(
    painter: &egui::Painter,
    text: &str,
    truncation: FilenameTruncation,
    font_id: &egui::FontId,
    color: Color32,
    max_width: f32,
) -> String {
    if text.is_empty() || max_width <= 0.0 {
        return String::new();
    }
    if text_width(painter, text, font_id, color) <= max_width {
        return text.to_owned();
    }

    const MARKER: &str = "…";
    if text_width(painter, MARKER, font_id, color) > max_width {
        return String::new();
    }

    let chars = text.chars().collect::<Vec<_>>();
    let mut best = MARKER.to_owned();
    let mut low = 0usize;
    let mut high = chars.len().saturating_sub(1);
    while low <= high {
        let visible = (low + high) / 2;
        let candidate = match truncation {
            FilenameTruncation::Middle => middle_truncate_chars(&chars, visible),
            FilenameTruncation::End => end_truncate_chars(&chars, visible),
        };
        if text_width(painter, &candidate, font_id, color) <= max_width {
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

fn middle_truncate_chars(chars: &[char], visible: usize) -> String {
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

fn end_truncate_chars(chars: &[char], visible: usize) -> String {
    if visible == 0 {
        return "…".to_owned();
    }

    let mut truncated = String::with_capacity(visible + 3);
    truncated.extend(chars.iter().take(visible.min(chars.len())));
    truncated.push_str("…");
    truncated
}

fn text_width(painter: &egui::Painter, text: &str, font_id: &egui::FontId, color: Color32) -> f32 {
    painter
        .layout_no_wrap(text.to_owned(), font_id.clone(), color)
        .size()
        .x
}

fn file_label_header_rect(rect: Rect, label_depth: usize) -> Option<Rect> {
    let scale = match label_depth {
        0 => return None,
        1 => 1.0,
        2 => 0.72,
        3 => 0.52,
        4 => 0.38,
        _ => 0.28,
    };
    let min_w = FILE_LABEL_MIN_W * scale;
    let min_h = (FILE_LABEL_MIN_H * scale).max(10.0);
    let min_area = (FILE_LABEL_MIN_AREA * scale * scale).max(650.0);

    if rect.width() < min_w || rect.height() < min_h || rect.width() * rect.height() < min_area {
        return None;
    }

    Some(Rect::from_min_max(
        pos2(rect.left() + 1.0, rect.top() + 1.0),
        pos2(
            rect.right() - 1.0,
            rect.top() + 1.0 + FOLDER_HEADER_H as f32,
        ),
    ))
}

fn paint_shrink_markers(
    painter: &egui::Painter,
    node: &FileNode,
    cache: &TreemapCache,
    depth: usize,
    prefs: &AppPrefs,
    shrunk_nodes: &BTreeSet<NodeId>,
) {
    if shrunk_nodes.contains(&node.id)
        && let Some(marker) = shrink_marker(node, cache, depth, prefs)
    {
        paint_shrink_marker(painter, marker);
    }

    for child in node.children.iter() {
        paint_shrink_markers(painter, child, cache, depth + 1, prefs, shrunk_nodes);
    }
}

enum ShrinkMarker {
    File(Rect),
    Folder { tab: Rect, icon: Rect },
}

fn shrink_marker(
    node: &FileNode,
    cache: &TreemapCache,
    depth: usize,
    prefs: &AppPrefs,
) -> Option<ShrinkMarker> {
    let rect = cache.egui_rect(node.id)?;
    if rect.width() < 14.0 || rect.height() < 14.0 {
        return None;
    }

    if node.is_dir {
        if let Some(layout_rect) = cache.rect(node.id)
            && let Some(header) = folder_header_rect(layout_rect, depth, prefs)
        {
            let header_rect = to_egui_rect(&header);
            if header_rect.width() < 16.0 || header_rect.height() < 7.0 {
                return None;
            }
            let spill = 3.0;
            let tab_h = (header_rect.height() + spill).min(rect.bottom() - header_rect.top());
            let tab_w = (tab_h + 8.0).min(header_rect.width() * 0.38);
            let tab = Rect::from_min_max(
                pos2(header_rect.right() - tab_w, header_rect.top()),
                pos2(header_rect.right(), header_rect.top() + tab_h),
            );
            let icon = tab.shrink2(vec2(3.0, 1.0));
            return Some(ShrinkMarker::Folder { tab, icon });
        }

        None
    } else {
        let size = rect.width().min(rect.height()).clamp(13.0, 22.0);
        Some(ShrinkMarker::File(Rect::from_min_size(
            pos2(rect.right() - size - 3.0, rect.top() + 3.0),
            vec2(size, size),
        )))
    }
}

fn paint_shrink_marker(painter: &egui::Painter, marker: ShrinkMarker) {
    match marker {
        ShrinkMarker::File(rect) => paint_file_shrink_marker(painter, rect),
        ShrinkMarker::Folder { tab, icon } => paint_folder_shrink_marker(painter, tab, icon),
    }
}

fn paint_file_shrink_marker(painter: &egui::Painter, rect: Rect) {
    painter.rect_filled(
        rect,
        3.0,
        Color32::from_rgba_unmultiplied(245, 245, 245, 215),
    );
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(20, 20, 20, 190)),
        egui::StrokeKind::Inside,
    );

    paint_shrink_glyph(painter, rect.shrink(3.0), Color32::from_rgb(28, 28, 26));
}

fn paint_folder_shrink_marker(painter: &egui::Painter, tab: Rect, icon: Rect) {
    painter.rect_filled(tab, 0.0, Color32::from_rgb(76, 76, 76));
    painter.line_segment(
        [tab.left_top(), tab.left_bottom()],
        egui::Stroke::new(1.0, Color32::from_rgb(40, 40, 40)),
    );
    paint_shrink_glyph(painter, icon, Color32::WHITE);
}

fn paint_shrink_glyph(painter: &egui::Painter, icon: Rect, color: Color32) {
    let scale = (icon.width() / 382.0).min(icon.height() / 404.0);
    let offset = icon.center() - vec2(382.0 * scale * 0.5, 404.0 * scale * 0.5);
    let p = |x: f32, y: f32| pos2(offset.x + x * scale, offset.y + y * scale);
    let stroke = egui::Stroke::new((9.0 * scale).clamp(1.0, 2.4), color);

    painter.line(
        vec![
            p(96.0, 207.0),
            p(96.0, 68.0),
            p(329.0, 68.0),
            p(329.0, 300.0),
            p(181.0, 300.0),
        ],
        stroke,
    );

    let small = Rect::from_min_size(p(38.0, 247.0), vec2(116.0 * scale, 116.0 * scale));
    painter.rect_stroke(small, 0.0, stroke, egui::StrokeKind::Inside);
    painter.line_segment([p(288.0, 113.0), p(176.0, 226.0)], stroke);
    painter.line(
        vec![p(177.0, 184.0), p(176.0, 226.0), p(218.0, 225.0)],
        stroke,
    );
}

fn paint_deleted_outlines(
    painter: &egui::Painter,
    cache: &TreemapCache,
    deleted: &DeletionOverlay,
) {
    for id in deleted.outline_ids() {
        let Some(rect) = cache.egui_rect(id) else {
            continue;
        };
        painter.rect_stroke(
            rect,
            0.0,
            egui::Stroke::new(2.0, Color32::from_rgb(255, 40, 40)),
            egui::StrokeKind::Outside,
        );
    }
}

fn to_egui_rect(r: &treemap::Rect) -> Rect {
    Rect::from_min_max(
        pos2(r.x as f32, r.y as f32),
        pos2((r.x + r.w) as f32, (r.y + r.h) as f32),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: NodeId, name: &str, size: u64) -> FileNode {
        FileNode {
            id,
            name: name.into(),
            source_path: None,
            size,
            is_dir: false,
            children: Box::new([]),
            modified: None,
            file_count: 1,
            dir_count: 0,
        }
    }

    #[test]
    fn shrunk_child_is_capped_against_final_visual_total() {
        let children = vec![file(1, "huge.bin", 90), file(2, "rest.bin", 10)];
        let shrunk = BTreeSet::from([1]);

        let weights = visual_child_sizes(&children, &shrunk);
        let total = weights.iter().sum::<f64>();

        assert!(weights[0] / total <= SHRUNK_MAX_FRACTION + 0.0001);
        assert!((weights[0] - 1.1111).abs() < 0.001);
        assert_eq!(weights[1], 10.0);
    }
}
