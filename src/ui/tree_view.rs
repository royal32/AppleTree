use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use egui::{Color32, Id, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::format_size;
use crate::model::tree::{FileNode, FileTree, NodeId};
use crate::settings::{AppPrefs, TableColumn};
use crate::ui::file_icons::{self, FileIconCache};
use crate::ui::{self, ActivePane, NodeCommand};

const HEADER_H: f32 = 24.0;
const ROW_H: f32 = 22.0;
const RESIZE_W: f32 = 5.0;

pub fn show_branding(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label(RichText::new("Mac").size(16.0).strong());
        ui.label(
            RichText::new("Dir")
                .size(16.0)
                .strong()
                .color(Color32::from_rgb(56, 132, 244)),
        );
        ui.label(RichText::new("Stat").size(16.0).strong());
    });
}

pub fn show(
    ui: &mut egui::Ui,
    tree: &FileTree,
    selected: &mut Option<NodeId>,
    hovered: &mut Option<NodeId>,
    expanded: &mut BTreeSet<NodeId>,
    deleted_nodes: &BTreeSet<NodeId>,
    deleted_outlines: &BTreeSet<NodeId>,
    active_pane: &mut ActivePane,
    prefs: &mut AppPrefs,
    prefs_changed: &mut bool,
    file_icons: &mut FileIconCache,
) -> Option<NodeCommand> {
    show_branding(ui);
    ui.add_space(4.0);

    expanded.insert(tree.root.id);

    let frame_fill = if ui.visuals().dark_mode {
        Color32::from_rgb(38, 38, 38)
    } else {
        Color32::from_rgb(236, 236, 236)
    };
    let alt_row_color = if ui.visuals().dark_mode {
        Color32::from_rgb(46, 46, 46)
    } else {
        Color32::from_rgb(226, 226, 226)
    };

    let mut command = None;
    let mut rows = Vec::new();
    collect_rows(&tree.root, None, 0, expanded, prefs, &mut rows);

    let arrow_command = handle_keyboard(ui.ctx(), tree, selected, expanded, active_pane, &rows);
    if command.is_none() {
        command = arrow_command;
    }

    let frame = egui::Frame::new()
        .fill(frame_fill)
        .corner_radius(8.0)
        .inner_margin(4.0);
    frame.show(ui, |ui| {
        egui::ScrollArea::both()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                show_header(ui, prefs, prefs_changed);
                for (row_index, row) in rows.iter().enumerate() {
                    if let Some(cmd) = show_row(
                        ui,
                        row,
                        row_index,
                        selected,
                        hovered,
                        expanded,
                        deleted_nodes,
                        deleted_outlines,
                        active_pane,
                        prefs,
                        file_icons,
                        frame_fill,
                        alt_row_color,
                    ) {
                        command = Some(cmd);
                    }
                }
            });
    });

    command
}

fn show_header(ui: &mut egui::Ui, prefs: &mut AppPrefs, prefs_changed: &mut bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for index in 0..prefs.columns.len() {
            let column = prefs.columns[index].column;
            let width = prefs.columns[index].width;
            let (rect, response) = ui.allocate_exact_size(vec2(width, HEADER_H), Sense::click());
            let fill = ui.visuals().widgets.inactive.bg_fill;
            ui.painter().rect_filled(rect, 0.0, fill);
            ui.painter().rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0, ui.visuals().widgets.inactive.bg_stroke.color),
                egui::StrokeKind::Inside,
            );

            let sort = if prefs.sort_column == column {
                if prefs.sort_descending {
                    " ▼"
                } else {
                    " ▲"
                }
            } else {
                ""
            };
            ui.painter().text(
                pos2(rect.left() + 6.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                format!("{}{}", column.title(), sort),
                egui::FontId::proportional(12.0),
                Color32::from_rgb(220, 220, 220),
            );

            if response.clicked() {
                if prefs.sort_column == column {
                    prefs.sort_descending = !prefs.sort_descending;
                } else {
                    prefs.sort_column = column;
                    prefs.sort_descending = column != TableColumn::Name;
                }
                *prefs_changed = true;
            }

            response.context_menu(|ui| {
                if ui.button("Move Column Left").clicked() {
                    prefs.move_column_left(column);
                    *prefs_changed = true;
                    ui.close_menu();
                }
                if ui.button("Move Column Right").clicked() {
                    prefs.move_column_right(column);
                    *prefs_changed = true;
                    ui.close_menu();
                }
            });

            let (resize_rect, resize_resp) =
                ui.allocate_exact_size(vec2(RESIZE_W, HEADER_H), Sense::drag());
            ui.painter().line_segment(
                [
                    pos2(resize_rect.center().x, resize_rect.top() + 3.0),
                    pos2(resize_rect.center().x, resize_rect.bottom() - 3.0),
                ],
                Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
            );
            if resize_resp.dragged() {
                let delta_x = ui.input(|i| i.pointer.delta().x);
                prefs.columns[index].width =
                    (prefs.columns[index].width + delta_x).clamp(48.0, 480.0);
                *prefs_changed = true;
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn show_row(
    ui: &mut egui::Ui,
    row: &RowInfo<'_>,
    row_index: usize,
    selected: &mut Option<NodeId>,
    hovered: &mut Option<NodeId>,
    expanded: &mut BTreeSet<NodeId>,
    deleted_nodes: &BTreeSet<NodeId>,
    deleted_outlines: &BTreeSet<NodeId>,
    active_pane: &mut ActivePane,
    prefs: &AppPrefs,
    file_icons: &mut FileIconCache,
    frame_fill: Color32,
    alt_row_color: Color32,
) -> Option<NodeCommand> {
    let mut command = None;
    let is_selected = *selected == Some(row.id);
    let bg = if is_selected {
        ui.visuals().selection.bg_fill
    } else if row_index % 2 == 1 {
        alt_row_color
    } else {
        frame_fill
    };

    let total_w = prefs
        .columns
        .iter()
        .map(|pref| pref.width + RESIZE_W)
        .sum::<f32>();
    let row_start = ui.cursor().min;
    let row_rect = Rect::from_min_size(row_start, vec2(total_w, ROW_H));
    ui.painter().rect_filled(row_rect, 0.0, bg);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for pref in &prefs.columns {
            let (rect, resp) = ui.allocate_exact_size(vec2(pref.width, ROW_H), Sense::click());
            let cell_resp = resp;
            if cell_resp.hovered() {
                *hovered = Some(row.id);
            }
            if cell_resp.clicked() {
                *selected = Some(row.id);
                *active_pane = ActivePane::Table;
            }
            if cell_resp.double_clicked() {
                command = Some(NodeCommand::Open(row.id));
            }
            cell_resp.context_menu(|ui| {
                *selected = Some(row.id);
                *active_pane = ActivePane::Table;
                ui::node_context_menu(ui, row.id, row.is_dir, "Zoom In Treemap", &mut command);
            });
            let is_deleted = row_is_deleted(row.id, deleted_nodes, deleted_outlines);
            paint_cell(
                ui,
                rect,
                row,
                pref.column,
                expanded,
                is_selected,
                is_deleted,
                file_icons,
            );
            ui.allocate_exact_size(vec2(RESIZE_W, ROW_H), Sense::hover());
        }
    });
    command
}

fn paint_cell(
    ui: &mut egui::Ui,
    rect: Rect,
    row: &RowInfo<'_>,
    column: TableColumn,
    expanded: &mut BTreeSet<NodeId>,
    is_selected: bool,
    is_deleted: bool,
    file_icons: &mut FileIconCache,
) {
    let text_color = if is_deleted {
        Color32::from_rgb(230, 45, 45)
    } else if is_selected {
        Color32::WHITE
    } else {
        Color32::from_rgb(220, 220, 220)
    };
    match column {
        TableColumn::Name => {
            let mut x = rect.left() + 4.0 + row.depth as f32 * 14.0;
            if row.is_dir && row.has_children {
                let arrow_rect = Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(14.0, 16.0));
                let arrow_resp =
                    ui.interact(arrow_rect, Id::new(("expand", row.id)), Sense::click());
                if arrow_resp.clicked() {
                    if expanded.contains(&row.id) {
                        expanded.remove(&row.id);
                    } else {
                        expanded.insert(row.id);
                    }
                }
                let glyph = if expanded.contains(&row.id) {
                    "▾"
                } else {
                    "▸"
                };
                paint_text(
                    ui,
                    arrow_rect,
                    arrow_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    glyph,
                    egui::FontId::proportional(13.0),
                    text_color,
                    is_deleted,
                );
            }
            x += 16.0;
            let icon_rect =
                Rect::from_center_size(pos2(x + 8.0, rect.center().y), vec2(16.0, 16.0));
            if let Some(texture) = file_icons.texture_for(ui.ctx(), row.is_dir, row.display_name) {
                ui.painter().image(
                    texture.id(),
                    icon_rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else if row.is_dir {
                file_icons::paint_fallback_folder_icon(ui.painter(), icon_rect);
            } else {
                file_icons::paint_fallback_file_icon(ui.painter(), icon_rect);
            }
            x = icon_rect.right() + 5.0;
            paint_text(
                ui,
                rect,
                pos2(x, rect.center().y),
                egui::Align2::LEFT_CENTER,
                row.display_name,
                egui::FontId::proportional(13.0),
                text_color,
                is_deleted,
            );
        }
        TableColumn::Size => paint_right(ui, rect, &format_size(row.size), text_color, is_deleted),
        TableColumn::PercentOfParent => {
            let pct_value = if let Some(parent_size) = row.parent_size {
                if parent_size > 0 {
                    row.size as f32 / parent_size as f32
                } else {
                    0.0
                }
            } else {
                1.0
            };
            paint_percent_bar(ui, rect, pct_value, is_deleted);
            let pct = if row.parent_size == Some(0) {
                String::new()
            } else {
                format!("{:.1}%", pct_value * 100.0)
            };
            paint_right(ui, rect, &pct, text_color, is_deleted);
        }
        TableColumn::Items => paint_right(
            ui,
            rect,
            &format_count(row.file_count + row.dir_count),
            text_color,
            is_deleted,
        ),
        TableColumn::Files => paint_right(
            ui,
            rect,
            &format_count(row.file_count),
            text_color,
            is_deleted,
        ),
        TableColumn::Folders => paint_right(
            ui,
            rect,
            &format_count(row.dir_count),
            text_color,
            is_deleted,
        ),
        TableColumn::Modified => {
            let text = row.modified.map(format_modified).unwrap_or_default();
            paint_text(
                ui,
                rect,
                pos2(rect.left() + 6.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                &text,
                egui::FontId::proportional(12.0),
                text_color,
                is_deleted,
            );
        }
    }
}

fn paint_right(ui: &egui::Ui, rect: Rect, text: &str, color: Color32, strike: bool) {
    paint_text(
        ui,
        rect,
        pos2(rect.right() - 6.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        text,
        egui::FontId::proportional(12.0),
        color,
        strike,
    );
}

fn paint_percent_bar(ui: &egui::Ui, rect: Rect, fraction: f32, is_deleted: bool) {
    let fraction = fraction.clamp(0.0, 1.0);
    if fraction <= 0.0 {
        return;
    }

    let inset = rect.shrink2(vec2(3.0, 4.0));
    let fill = Rect::from_min_size(inset.min, vec2(inset.width() * fraction, inset.height()));
    let color = if is_deleted {
        Color32::from_rgba_unmultiplied(230, 45, 45, 45)
    } else if ui.visuals().dark_mode {
        Color32::from_rgba_unmultiplied(120, 150, 190, 48)
    } else {
        Color32::from_rgba_unmultiplied(56, 132, 244, 32)
    };
    ui.painter().rect_filled(fill, 2.0, color);
}

fn paint_text(
    ui: &egui::Ui,
    cell_rect: Rect,
    pos: egui::Pos2,
    align: egui::Align2,
    text: &str,
    font_id: egui::FontId,
    color: Color32,
    strike: bool,
) {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font_id, color);
    let text_rect = align.anchor_size(pos, galley.size());
    ui.painter().galley(text_rect.min, galley, color);

    if strike && !text.is_empty() {
        let strike_rect = text_rect.intersect(cell_rect.shrink2(vec2(4.0, 0.0)));
        if strike_rect.width() > 1.0 {
            let y = strike_rect.center().y;
            ui.painter().line_segment(
                [pos2(strike_rect.left(), y), pos2(strike_rect.right(), y)],
                Stroke::new(1.0, Color32::from_rgb(230, 45, 45)),
            );
        }
    }
}

fn row_is_deleted(
    id: NodeId,
    deleted_nodes: &BTreeSet<NodeId>,
    deleted_outlines: &BTreeSet<NodeId>,
) -> bool {
    deleted_nodes.contains(&id) || deleted_outlines.contains(&id)
}

fn handle_keyboard(
    ctx: &egui::Context,
    tree: &FileTree,
    selected: &mut Option<NodeId>,
    expanded: &mut BTreeSet<NodeId>,
    active_pane: &mut ActivePane,
    rows: &[RowInfo<'_>],
) -> Option<NodeCommand> {
    if *active_pane != ActivePane::Table {
        return None;
    }
    if rows.is_empty() {
        return None;
    }

    let command = ctx.input(|i| {
        if i.key_pressed(egui::Key::Enter) {
            selected.map(NodeCommand::Open)
        } else {
            None
        }
    });
    if command.is_some() {
        return command;
    }

    let key = ctx.input(|i| {
        if i.key_pressed(egui::Key::ArrowDown) {
            Some(egui::Key::ArrowDown)
        } else if i.key_pressed(egui::Key::ArrowUp) {
            Some(egui::Key::ArrowUp)
        } else if i.key_pressed(egui::Key::Home) {
            Some(egui::Key::Home)
        } else if i.key_pressed(egui::Key::End) {
            Some(egui::Key::End)
        } else if i.key_pressed(egui::Key::ArrowLeft) {
            Some(egui::Key::ArrowLeft)
        } else if i.key_pressed(egui::Key::ArrowRight) {
            Some(egui::Key::ArrowRight)
        } else {
            None
        }
    });

    let key = key?;
    let current = selected
        .and_then(|id| rows.iter().position(|row| row.id == id))
        .unwrap_or(0);
    match key {
        egui::Key::ArrowDown => {
            *selected = Some(rows[(current + 1).min(rows.len() - 1)].id);
        }
        egui::Key::ArrowUp => {
            *selected = Some(rows[current.saturating_sub(1)].id);
        }
        egui::Key::Home => {
            *selected = Some(rows[0].id);
        }
        egui::Key::End => {
            *selected = Some(rows[rows.len() - 1].id);
        }
        egui::Key::ArrowRight => {
            if let Some(id) = *selected
                && let Some(node) = tree.root.resolve_id(id)
                && node.is_dir
                && !node.children.is_empty()
            {
                expanded.insert(id);
            }
        }
        egui::Key::ArrowLeft => {
            if let Some(id) = *selected {
                if expanded.remove(&id) {
                    return None;
                }
                if let Some(path) = tree.root.path_to_id(id)
                    && let Some(parent_path) = path.split_last().map(|(_, parent)| parent)
                    && let Some(parent) = tree.root.resolve_path(parent_path)
                {
                    *selected = Some(parent.id);
                }
            }
        }
        _ => {}
    }
    None
}

struct RowInfo<'a> {
    id: NodeId,
    display_name: &'a str,
    size: u64,
    file_count: u64,
    dir_count: u64,
    modified: Option<SystemTime>,
    is_dir: bool,
    has_children: bool,
    depth: usize,
    parent_size: Option<u64>,
}

fn collect_rows<'a>(
    node: &'a FileNode,
    parent_size: Option<u64>,
    depth: usize,
    expanded: &BTreeSet<NodeId>,
    prefs: &AppPrefs,
    rows: &mut Vec<RowInfo<'a>>,
) {
    rows.push(RowInfo {
        id: node.id,
        display_name: &node.name,
        size: node.size,
        file_count: node.file_count,
        dir_count: node.dir_count,
        modified: node.modified,
        is_dir: node.is_dir,
        has_children: !node.children.is_empty(),
        depth,
        parent_size,
    });

    if !expanded.contains(&node.id) {
        return;
    }

    let mut child_indices = (0..node.children.len()).collect::<Vec<_>>();
    child_indices.sort_by(|&a, &b| compare_nodes(&node.children[a], &node.children[b], prefs));
    for child_index in child_indices {
        collect_rows(
            &node.children[child_index],
            Some(node.size),
            depth + 1,
            expanded,
            prefs,
            rows,
        );
    }
}

fn compare_nodes(a: &FileNode, b: &FileNode, prefs: &AppPrefs) -> Ordering {
    let ord = match prefs.sort_column {
        TableColumn::Name => natural_name_cmp(&a.name, &b.name),
        TableColumn::Size | TableColumn::PercentOfParent => a.size.cmp(&b.size),
        TableColumn::Items => (a.file_count + a.dir_count).cmp(&(b.file_count + b.dir_count)),
        TableColumn::Files => a.file_count.cmp(&b.file_count),
        TableColumn::Folders => a.dir_count.cmp(&b.dir_count),
        TableColumn::Modified => a.modified.cmp(&b.modified),
    };
    let ord = if prefs.sort_descending {
        ord.reverse()
    } else {
        ord
    };
    ord.then_with(|| natural_name_cmp(&a.name, &b.name))
}

fn natural_name_cmp(a: &str, b: &str) -> Ordering {
    a.to_lowercase().cmp(&b.to_lowercase())
}

fn format_count(count: u64) -> String {
    let s = count.to_string();
    let mut result = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

fn format_modified(time: SystemTime) -> String {
    let Ok(duration) = time.duration_since(UNIX_EPOCH) else {
        return String::new();
    };
    let secs = duration.as_secs() as libc::time_t;
    let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
    let ptr = unsafe { libc::localtime_r(&secs, tm.as_mut_ptr()) };
    if ptr.is_null() {
        return String::new();
    }
    let tm = unsafe { tm.assume_init() };
    let mut buf = [0i8; 32];
    let fmt = c"%Y-%m-%d %H:%M";
    let written = unsafe { libc::strftime(buf.as_mut_ptr(), buf.len(), fmt.as_ptr(), &tm) };
    if written == 0 {
        return String::new();
    }
    let bytes = &buf[..written];
    String::from_utf8_lossy(&bytes.iter().map(|&b| b as u8).collect::<Vec<_>>()).into_owned()
}
