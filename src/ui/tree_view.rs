use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::SystemTime;

use egui::{Color32, Id, Rect, Sense, Stroke, pos2, vec2};

use crate::model::tree::{FileNode, FileTree, NodeId};
use crate::settings::{AppPrefs, TableColumn};
use crate::ui::file_icons::{self, FileIconCache};
use crate::ui::{self, ActivePane, DeletionOverlay, NodeCommand, PaneState};
use crate::{format_count, format_modified, format_size};

const HEADER_H: f32 = 24.0;
const ROW_H: f32 = 22.0;
const RESIZE_W: f32 = 5.0;

pub struct TreeViewState<'a> {
    pub pane: &'a mut PaneState,
    pub expanded: &'a mut BTreeSet<NodeId>,
    pub deleted: &'a DeletionOverlay,
    pub shrunk_treemap_nodes: &'a BTreeSet<NodeId>,
    pub file_icons: &'a mut FileIconCache,
    pub table: &'a mut TableState,
}

#[derive(Default)]
pub struct TableState {
    sort: Option<(TableColumn, bool)>,
    child_orders: BTreeMap<NodeId, Rc<[usize]>>,
}

impl TableState {
    fn begin_frame(&mut self, prefs: &AppPrefs) {
        let sort = (prefs.sort_column, prefs.sort_descending);
        if self.sort != Some(sort) {
            self.child_orders.clear();
            self.sort = Some(sort);
        }
    }

    fn child_order(&mut self, node: &FileNode, prefs: &AppPrefs) -> Rc<[usize]> {
        if let Some(order) = self.child_orders.get(&node.id) {
            return Rc::clone(order);
        }

        let mut indices = (0..node.children.len()).collect::<Vec<_>>();
        indices.sort_by(|&a, &b| compare_nodes(&node.children[a], &node.children[b], prefs));
        let order = Rc::from(indices.into_boxed_slice());
        self.child_orders.insert(node.id, Rc::clone(&order));
        order
    }
}

#[derive(Clone, Copy)]
struct RowStyle {
    frame_fill: Color32,
    alt_row_color: Color32,
}

struct RowContext<'a> {
    pane: &'a mut PaneState,
    expanded: &'a mut BTreeSet<NodeId>,
    deleted: &'a DeletionOverlay,
    shrunk_treemap_nodes: &'a BTreeSet<NodeId>,
    prefs: &'a AppPrefs,
    file_icons: &'a mut FileIconCache,
    style: RowStyle,
}

struct TextStyle {
    font_id: egui::FontId,
    color: Color32,
}

pub fn show(
    ui: &mut egui::Ui,
    tree: &FileTree,
    state: TreeViewState<'_>,
    prefs: &mut AppPrefs,
    prefs_changed: &mut bool,
) -> Option<NodeCommand> {
    let TreeViewState {
        pane,
        expanded,
        deleted,
        shrunk_treemap_nodes,
        file_icons,
        table,
    } = state;

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

    let frame = egui::Frame::new()
        .fill(frame_fill)
        .corner_radius(8.0)
        .inner_margin(4.0);
    let frame_height = ui.available_height();
    let content_height = (frame_height - frame.total_margin().sum().y).max(0.0);
    frame.show(ui, |ui| {
        ui.set_min_height(content_height);
        ui.set_max_height(content_height);
        let table_width = prefs
            .columns
            .iter()
            .map(|pref| pref.width + RESIZE_W)
            .sum::<f32>();

        egui::ScrollArea::horizontal()
            .id_salt("file_table_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(table_width);
                show_header(ui, prefs, prefs_changed);
                table.begin_frame(prefs);

                let mut rows = Vec::new();
                collect_rows(&tree.root, None, 0, expanded, prefs, table, &mut rows);

                let arrow_command = handle_keyboard(ui.ctx(), tree, pane, expanded, &rows);
                if command.is_none() {
                    command = arrow_command;
                }

                let mut row_context = RowContext {
                    pane,
                    expanded,
                    deleted,
                    shrunk_treemap_nodes,
                    prefs,
                    file_icons,
                    style: RowStyle {
                        frame_fill,
                        alt_row_color,
                    },
                };
                egui::ScrollArea::vertical()
                    .id_salt("file_table_rows")
                    .auto_shrink([false, false])
                    .show_rows(ui, ROW_H, rows.len(), |ui, row_range| {
                        ui.set_min_width(table_width);
                        for row_index in row_range {
                            if let Some(cmd) =
                                show_row(ui, &rows[row_index], row_index, &mut row_context)
                            {
                                command = Some(cmd);
                            }
                        }
                    });
            });
    });

    command
}

pub fn show_empty(ui: &mut egui::Ui, prefs: &mut AppPrefs, prefs_changed: &mut bool) {
    let frame_fill = if ui.visuals().dark_mode {
        Color32::from_rgb(38, 38, 38)
    } else {
        Color32::from_rgb(236, 236, 236)
    };

    let frame = egui::Frame::new()
        .fill(frame_fill)
        .corner_radius(8.0)
        .inner_margin(4.0);
    let frame_height = ui.available_height();
    let content_height = (frame_height - frame.total_margin().sum().y).max(0.0);
    frame.show(ui, |ui| {
        ui.set_min_height(content_height);
        ui.set_max_height(content_height);
        let table_width = prefs
            .columns
            .iter()
            .map(|pref| pref.width + RESIZE_W)
            .sum::<f32>();

        egui::ScrollArea::horizontal()
            .id_salt("file_table_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(table_width);
                show_header(ui, prefs, prefs_changed);
            });
    });
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

fn show_row(
    ui: &mut egui::Ui,
    row: &RowInfo<'_>,
    row_index: usize,
    context: &mut RowContext<'_>,
) -> Option<NodeCommand> {
    let mut command = None;
    let is_selected = context.pane.selected == Some(row.id);
    let bg = if is_selected {
        ui.visuals().selection.bg_fill
    } else if row_index % 2 == 1 {
        context.style.alt_row_color
    } else {
        context.style.frame_fill
    };

    let total_w = context
        .prefs
        .columns
        .iter()
        .map(|pref| pref.width + RESIZE_W)
        .sum::<f32>();
    let row_start = ui.cursor().min;
    let row_rect = Rect::from_min_size(row_start, vec2(total_w, ROW_H));
    ui.painter().rect_filled(row_rect, 0.0, bg);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for pref in &context.prefs.columns {
            let (rect, resp) = ui.allocate_exact_size(vec2(pref.width, ROW_H), Sense::click());
            let cell_resp = resp;
            if cell_resp.hovered() {
                context.pane.hovered = Some(row.id);
            }
            if cell_resp.clicked() {
                context.pane.select(row.id, ActivePane::Table);
            }
            if cell_resp.double_clicked() {
                command = Some(NodeCommand::Open(row.id));
            }
            cell_resp.context_menu(|ui| {
                context.pane.select(row.id, ActivePane::Table);
                ui::node_context_menu(
                    ui,
                    row.id,
                    row.is_dir,
                    "Zoom In Treemap",
                    context.shrunk_treemap_nodes.contains(&row.id),
                    &mut command,
                );
            });
            let is_deleted = context.deleted.is_deleted(row.id);
            paint_cell(ui, rect, row, pref.column, context, is_selected, is_deleted);
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
    context: &mut RowContext<'_>,
    is_selected: bool,
    is_deleted: bool,
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
                    if context.expanded.contains(&row.id) {
                        context.expanded.remove(&row.id);
                    } else {
                        context.expanded.insert(row.id);
                    }
                }
                let glyph = if context.expanded.contains(&row.id) {
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
                    TextStyle {
                        font_id: egui::FontId::proportional(13.0),
                        color: text_color,
                    },
                    is_deleted,
                );
            }
            x += 16.0;
            let icon_rect =
                Rect::from_center_size(pos2(x + 8.0, rect.center().y), vec2(16.0, 16.0));
            if let Some(texture) =
                context
                    .file_icons
                    .texture_for(ui.ctx(), row.is_dir, row.display_name)
            {
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
                TextStyle {
                    font_id: egui::FontId::proportional(13.0),
                    color: text_color,
                },
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
                TextStyle {
                    font_id: egui::FontId::proportional(12.0),
                    color: text_color,
                },
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
        TextStyle {
            font_id: egui::FontId::proportional(12.0),
            color,
        },
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
    style: TextStyle,
    strike: bool,
) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), style.font_id, style.color);
    let text_rect = align.anchor_size(pos, galley.size());
    ui.painter().galley(text_rect.min, galley, style.color);

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

fn handle_keyboard(
    ctx: &egui::Context,
    tree: &FileTree,
    pane: &mut PaneState,
    expanded: &mut BTreeSet<NodeId>,
    rows: &[RowInfo<'_>],
) -> Option<NodeCommand> {
    if pane.active_pane != ActivePane::Table {
        return None;
    }
    if rows.is_empty() {
        return None;
    }

    let command = ctx.input(|i| {
        if i.key_pressed(egui::Key::Enter) {
            pane.selected.map(NodeCommand::Open)
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
    let current = pane
        .selected
        .and_then(|id| rows.iter().position(|row| row.id == id))
        .unwrap_or(0);
    match key {
        egui::Key::ArrowDown => {
            pane.selected = Some(rows[(current + 1).min(rows.len() - 1)].id);
        }
        egui::Key::ArrowUp => {
            pane.selected = Some(rows[current.saturating_sub(1)].id);
        }
        egui::Key::Home => {
            pane.selected = Some(rows[0].id);
        }
        egui::Key::End => {
            pane.selected = Some(rows[rows.len() - 1].id);
        }
        egui::Key::ArrowRight => {
            if let Some(id) = pane.selected
                && let Some(node) = tree.node(id)
                && node.is_dir
                && !node.children.is_empty()
            {
                expanded.insert(id);
            }
        }
        egui::Key::ArrowLeft => {
            if let Some(id) = pane.selected {
                if expanded.remove(&id) {
                    return None;
                }
                if let Some(parent_id) = tree.parent_id(id) {
                    pane.selected = Some(parent_id);
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
    table: &mut TableState,
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

    let child_indices = table.child_order(node, prefs);
    for &child_index in child_indices.iter() {
        collect_rows(
            &node.children[child_index],
            Some(node.size),
            depth + 1,
            expanded,
            prefs,
            table,
            rows,
        );
    }
}

pub(crate) fn compare_nodes(a: &FileNode, b: &FileNode, prefs: &AppPrefs) -> Ordering {
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
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
}
