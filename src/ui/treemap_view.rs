use std::collections::BTreeSet;

use egui::{Color32, ColorImage, Rect, Sense, TextureHandle, TextureOptions, pos2};
use treemap::{Mappable, TreemapLayout};

use crate::format_size;
use crate::model::color::{ColorMap, PALETTE_BRIGHTNESS};
use crate::model::tree::{FileNode, FileTree, NodeId};
use crate::settings::AppPrefs;
use crate::ui::{ActivePane, NodeCommand};

/// Cushion surface coefficients: [a_x, a_y, c_x, c_y]
/// z(x,y) = a_x*x^2 + a_y*y^2 + c_x*x + c_y*y
type Surface = [f64; 4];

/// Leaf data collected during layout: (treemap rect, surface coefficients, color)
struct CushionLeaf {
    rect: treemap::Rect,
    surface: Surface,
    color: Color32,
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

// Lighting parameters (WinDirStat defaults)
const AMBIENT: f64 = 0.13;
const DIFFUSE: f64 = 1.0 - AMBIENT;
const BRIGHTNESS_FACTOR: f64 = 0.88 / PALETTE_BRIGHTNESS;

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
    tree: &mut FileTree,
    selected: &mut Option<NodeId>,
    hovered: &mut Option<NodeId>,
    active_pane: &mut ActivePane,
    color_map: &ColorMap,
    deleted_nodes: &BTreeSet<NodeId>,
    deleted_outlines: &BTreeSet<NodeId>,
    prefs: &AppPrefs,
    cached_layout_rect: &mut Option<Rect>,
    treemap_texture: &mut Option<TextureHandle>,
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

    // Check if we need to re-layout and re-render the cushion texture.
    // Compare the full canvas rect (position + size) so that side panel
    // resizing or window moves trigger a re-layout.
    let needs_update = if let Some(cached) = *cached_layout_rect {
        (canvas.left() - cached.left()).abs() > 1.0
            || (canvas.top() - cached.top()).abs() > 1.0
            || (w - cached.width()).abs() > 1.0
            || (h - cached.height()).abs() > 1.0
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

        layout_node(&mut tree.root, bounds, 0, prefs);

        // Collect cushion leaves
        let mut leaves = Vec::new();
        let surface = [0.0f64; 4];
        collect_cushion_leaves(
            &tree.root,
            surface,
            CUSHION_HEIGHT,
            true,
            color_map,
            &mut leaves,
        );

        // Render cushion texture
        let pw = w as usize;
        let ph = h as usize;
        if pw > 0 && ph > 0 {
            let image = render_cushion_image(pw, ph, canvas, &leaves);
            let texture = ui
                .ctx()
                .load_texture("treemap_cushion", image, TextureOptions::NEAREST);
            *treemap_texture = Some(texture);
        }

        *cached_layout_rect = Some(canvas);
    }

    paint_folder_backgrounds(&painter, &tree.root, 0, prefs);

    // Paint the cached file-cushion texture over transparent folder content.
    if let Some(tex) = treemap_texture {
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        painter.image(tex.id(), canvas, uv, Color32::WHITE);
    }

    // Handle clicks
    if response.clicked()
        && let Some(pos) = response.interact_pointer_pos()
    {
        if let Some(id) = find_node_at(&tree.root, pos) {
            *selected = Some(id);
            *active_pane = ActivePane::Treemap;
        } else {
            *selected = None;
        }
    }

    let menu_target_key = response.id.with("treemap_context_target");
    if response.secondary_clicked() {
        let target = ui
            .ctx()
            .pointer_latest_pos()
            .filter(|pos| canvas.contains(*pos))
            .and_then(|pos| find_node_at(&tree.root, pos));

        ui.ctx().data_mut(|data| {
            if let Some(id) = target {
                data.insert_temp(menu_target_key, id);
            } else {
                data.remove::<NodeId>(menu_target_key);
            }
        });

        if let Some(id) = target {
            *selected = Some(id);
            *active_pane = ActivePane::Treemap;
        }
    }

    let menu_target = ui
        .ctx()
        .data_mut(|data| data.get_temp::<NodeId>(menu_target_key));
    if let Some(id) = menu_target {
        response.context_menu(|ui| {
            node_context_menu(ui, id, &mut command);
        });
    }

    // Hover tooltip
    if let Some(pos) = response.hover_pos() {
        if let Some(id) = find_node_at(&tree.root, pos)
            && let Some(node) = tree.root.resolve_id(id)
        {
            *hovered = Some(id);
            let full_path = tree
                .full_display_path_for_id(id)
                .unwrap_or_else(|| node.name.to_string());
            let tip = format!("{}\n{}", full_path, format_size(node.size));
            egui::show_tooltip_at_pointer(ui.ctx(), ui.layer_id(), response.id.with("tip"), |ui| {
                ui.set_max_width(560.0);
                ui.label(tip);
            });
        }
    }

    paint_folder_labels_and_borders(&painter, &tree.root, 0, prefs, deleted_nodes);
    paint_file_labels(&painter, &tree.root, 0, prefs);

    if let Some(hover_id) = *hovered
        && let Some(node) = tree.root.resolve_id(hover_id)
    {
        let r = to_egui_rect(&node.rect);
        if r.width() > 0.0 && r.height() > 0.0 {
            painter.rect_stroke(
                r,
                0.0,
                egui::Stroke::new(1.0, Color32::WHITE),
                egui::StrokeKind::Outside,
            );
        }
    }

    // Draw selection highlight
    if let Some(sel_id) = selected
        && let Some(node) = tree.root.resolve_id(*sel_id)
    {
        let r = to_egui_rect(&node.rect);
        if r.width() > 0.0 && r.height() > 0.0 {
            painter.rect_stroke(
                r,
                0.0,
                egui::Stroke::new(2.0, Color32::WHITE),
                egui::StrokeKind::Outside,
            );
        }
    }

    paint_deleted_outlines(&painter, &tree.root, deleted_outlines);

    command
}

fn layout_node(node: &mut FileNode, bounds: treemap::Rect, depth: usize, prefs: &AppPrefs) {
    node.rect = bounds;

    if node.children.is_empty() || node.size == 0 {
        return;
    }

    let content = folder_content_rect(&bounds, depth, prefs);
    if content.w < 1.0 || content.h < 1.0 {
        return;
    }

    let mut items: Vec<LayoutItem> = node
        .children
        .iter()
        .enumerate()
        .map(|(child_index, c)| LayoutItem {
            child_index,
            size: c.size as f64,
            bounds: treemap::Rect::new(),
        })
        .collect();

    let layout = TreemapLayout::new();
    layout.layout_items(&mut items, content);

    for item in items.iter() {
        let b: treemap::Rect = *item.bounds();
        if let Some(child) = node.children.get_mut(item.child_index) {
            layout_node(child, b, depth + 1, prefs);
        }
    }
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
    mut surface: Surface,
    h: f64,
    is_root: bool,
    color_map: &ColorMap,
    leaves: &mut Vec<CushionLeaf>,
) {
    // Add ridge for this node (skip root per WinDirStat)
    if !is_root {
        add_ridge(&mut surface, &node.rect, h);
    }

    if node.children.is_empty() {
        if node.is_dir {
            return;
        }
        let color = color_map.get_treemap(node.extension());
        leaves.push(CushionLeaf {
            rect: node.rect,
            surface,
            color,
        });
    } else {
        let child_h = h * CUSHION_SCALE;
        for child in node.children.iter() {
            collect_cushion_leaves(child, surface, child_h, false, color_map, leaves);
        }
    }
}

fn render_cushion_image(pw: usize, ph: usize, canvas: Rect, leaves: &[CushionLeaf]) -> ColorImage {
    let mut image = ColorImage::new([pw, ph], Color32::TRANSPARENT);

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
            let row_offset = iy * pw;

            for ix in left..right {
                let sx = canvas_left + ix as f64 + 0.5;
                let nx = -(2.0 * s[0] * sx + s[2]);

                let cosa = (nx * lx + ny * ly + lz) / (nx * nx + ny * ny + 1.0).sqrt();
                let cosa = cosa.clamp(0.0, 1.0);

                let pixel = (DIFFUSE * cosa + AMBIENT) * BRIGHTNESS_FACTOR;

                let pr = (col_r * pixel).min(255.0) as u8;
                let pg = (col_g * pixel).min(255.0) as u8;
                let pb = (col_b * pixel).min(255.0) as u8;

                if let Some(dest) = image.pixels.get_mut(row_offset + ix) {
                    *dest = Color32::from_rgb(pr, pg, pb);
                }
            }
        }
    }

    image
}

fn find_node_at(node: &FileNode, pos: egui::Pos2) -> Option<NodeId> {
    let r = to_egui_rect(&node.rect);
    if !r.contains(pos) {
        return None;
    }

    for child in node.children.iter() {
        if let Some(id) = find_node_at(child, pos) {
            return Some(id);
        }
    }

    Some(node.id)
}

fn paint_folder_backgrounds(
    painter: &egui::Painter,
    node: &FileNode,
    depth: usize,
    prefs: &AppPrefs,
) {
    if !node.is_dir {
        return;
    }

    let rect = to_egui_rect(&node.rect);
    if depth <= prefs.treemap_folder_depth {
        painter.rect_filled(rect, 0.0, Color32::from_rgb(40, 40, 40));

        if let Some(header) = folder_header_rect(&node.rect, depth, prefs) {
            painter.rect_filled(to_egui_rect(&header), 0.0, Color32::from_rgb(50, 50, 50));
        }

        let content = folder_content_rect(&node.rect, depth, prefs);
        painter.rect_filled(to_egui_rect(&content), 0.0, Color32::from_rgb(28, 28, 28));
    }

    for child in node.children.iter() {
        paint_folder_backgrounds(painter, child, depth + 1, prefs);
    }
}

fn paint_folder_labels_and_borders(
    painter: &egui::Painter,
    node: &FileNode,
    depth: usize,
    prefs: &AppPrefs,
    deleted_nodes: &BTreeSet<NodeId>,
) {
    if !node.is_dir {
        return;
    }

    let rect = to_egui_rect(&node.rect);
    if depth <= prefs.treemap_folder_depth {
        if let Some(header) = folder_header_rect(&node.rect, depth, prefs) {
            let header_rect = to_egui_rect(&header);
            let label = format!("{} ({})", node.name, format_size(node.size));
            let label_pos = pos2(header_rect.left() + 4.0, header_rect.center().y);
            painter.with_clip_rect(header_rect).text(
                label_pos,
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::monospace(8.0),
                if deleted_nodes.contains(&node.id) {
                    Color32::from_rgb(255, 70, 70)
                } else {
                    Color32::from_rgb(235, 235, 235)
                },
            );
        }
        painter.rect_stroke(
            rect,
            0.0,
            egui::Stroke::new(1.0, Color32::from_rgb(76, 76, 76)),
            egui::StrokeKind::Inside,
        );
    }

    for child in node.children.iter() {
        paint_folder_labels_and_borders(painter, child, depth + 1, prefs, deleted_nodes);
    }
}

fn paint_file_labels(painter: &egui::Painter, node: &FileNode, depth: usize, prefs: &AppPrefs) {
    if node.is_dir {
        for child in node.children.iter() {
            paint_file_labels(painter, child, depth + 1, prefs);
        }
        return;
    }

    if prefs.treemap_label_depth == 0 || depth > prefs.treemap_label_depth {
        return;
    }

    let rect = to_egui_rect(&node.rect);
    if rect.width() < 92.0 || rect.height() < 34.0 {
        return;
    }

    let label = format!("{} ({})", node.name, format_size(node.size));
    let font_id = egui::FontId::proportional(10.5);
    let galley = painter.layout_no_wrap(label, font_id, Color32::WHITE);
    let badge_w = (galley.size().x + 8.0).min(rect.width() - 4.0);
    let badge_h = (galley.size().y + 5.0).min(rect.height() - 4.0);
    if badge_w <= 12.0 || badge_h <= 10.0 {
        return;
    }

    let badge = Rect::from_min_size(
        rect.left_top() + egui::vec2(2.0, 2.0),
        egui::vec2(badge_w, badge_h),
    );
    painter.rect_filled(badge, 2.0, Color32::from_black_alpha(175));
    painter.with_clip_rect(badge.shrink(3.0)).galley(
        badge.left_top() + egui::vec2(4.0, 2.0),
        galley,
        Color32::WHITE,
    );
}

fn paint_deleted_outlines(
    painter: &egui::Painter,
    node: &FileNode,
    deleted_outlines: &BTreeSet<NodeId>,
) {
    if deleted_outlines.contains(&node.id) {
        let rect = to_egui_rect(&node.rect);
        if rect.width() > 0.0 && rect.height() > 0.0 {
            painter.rect_stroke(
                rect,
                0.0,
                egui::Stroke::new(2.0, Color32::from_rgb(255, 40, 40)),
                egui::StrokeKind::Outside,
            );
        }
    }

    for child in node.children.iter() {
        paint_deleted_outlines(painter, child, deleted_outlines);
    }
}

fn node_context_menu(ui: &mut egui::Ui, id: NodeId, command: &mut Option<NodeCommand>) {
    if ui.button("Open").clicked() {
        *command = Some(NodeCommand::Open(id));
        ui.close_menu();
    }
    if ui.button("Reveal in Finder").clicked() {
        *command = Some(NodeCommand::Reveal(id));
        ui.close_menu();
    }
    if ui.button("Copy Path").clicked() {
        *command = Some(NodeCommand::CopyPath(id));
        ui.close_menu();
    }
    ui.separator();
    if ui.button("Delete").clicked() {
        *command = Some(NodeCommand::Delete { id, confirm: true });
        ui.close_menu();
    }
}

fn to_egui_rect(r: &treemap::Rect) -> Rect {
    Rect::from_min_max(
        pos2(r.x as f32, r.y as f32),
        pos2((r.x + r.w) as f32, (r.y + r.h) as f32),
    )
}
