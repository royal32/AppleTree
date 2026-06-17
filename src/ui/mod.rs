pub mod file_icons;
pub mod tree_view;
pub mod treemap_view;

use std::ops::Range;

use crate::model::tree::{FileNode, NodeId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivePane {
    Table,
    Treemap,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeCommand {
    Open(NodeId),
    Reveal(NodeId),
    Delete { id: NodeId, confirm: bool },
    CopyPath(NodeId),
    ZoomIn(NodeId),
    ZoomOut,
    ToggleShrink(NodeId),
}

pub struct PaneState {
    pub selected: Option<NodeId>,
    pub hovered: Option<NodeId>,
    pub active_pane: ActivePane,
}

impl Default for PaneState {
    fn default() -> Self {
        Self {
            selected: None,
            hovered: None,
            active_pane: ActivePane::Table,
        }
    }
}

impl PaneState {
    pub fn select(&mut self, id: NodeId, active_pane: ActivePane) {
        self.selected = Some(id);
        self.active_pane = active_pane;
    }
}

#[derive(Default)]
pub struct DeletionOverlay {
    ranges: Vec<Range<NodeId>>,
}

impl DeletionOverlay {
    pub fn mark_deleted(&mut self, node: &FileNode) {
        self.ranges
            .push(node.id..node.id.saturating_add(node.subtree_node_count()));
    }

    pub fn outline_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.ranges.iter().map(|range| range.start)
    }

    pub fn is_node_deleted(&self, id: NodeId) -> bool {
        self.ranges.iter().any(|range| range.contains(&id))
    }

    pub fn is_deleted(&self, id: NodeId) -> bool {
        self.is_node_deleted(id)
    }
}

pub(crate) fn node_context_menu(
    ui: &mut egui::Ui,
    id: NodeId,
    can_zoom_in: bool,
    zoom_in_label: &str,
    is_shrunk: bool,
    command: &mut Option<NodeCommand>,
) {
    if can_zoom_in && ui.button(zoom_in_label).clicked() {
        *command = Some(NodeCommand::ZoomIn(id));
        ui.close_menu();
    }
    if ui.button("Zoom Out").clicked() {
        *command = Some(NodeCommand::ZoomOut);
        ui.close_menu();
    }
    let shrink_label = if is_shrunk {
        "Unshrink in Treemap"
    } else {
        "Shrink in Treemap"
    };
    if ui.button(shrink_label).clicked() {
        *command = Some(NodeCommand::ToggleShrink(id));
        ui.close_menu();
    }
    ui.separator();
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

    fn dir(id: NodeId, name: &str, children: Vec<FileNode>) -> FileNode {
        let size = children.iter().map(|child| child.size).sum();
        let file_count = children.iter().map(|child| child.file_count).sum();
        let dir_count = 1 + children.iter().map(|child| child.dir_count).sum::<u64>();
        FileNode {
            id,
            name: name.into(),
            source_path: None,
            size,
            is_dir: true,
            children: children.into(),
            modified: None,
            file_count,
            dir_count,
        }
    }

    #[test]
    fn deletion_overlay_marks_subtree_range() {
        let node = dir(2, "folder", vec![file(3, "nested.txt", 10)]);
        let mut deleted = DeletionOverlay::default();

        deleted.mark_deleted(&node);

        assert_eq!(deleted.outline_ids().collect::<Vec<_>>(), vec![2]);
        assert!(deleted.is_deleted(2));
        assert!(deleted.is_deleted(3));
        assert!(!deleted.is_deleted(1));
        assert!(!deleted.is_deleted(4));
    }
}
