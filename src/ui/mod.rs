pub mod file_icons;
pub mod tree_view;
pub mod treemap_view;

use std::collections::BTreeSet;

use crate::model::tree::NodeId;

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
    nodes: BTreeSet<NodeId>,
    outlines: BTreeSet<NodeId>,
}

impl DeletionOverlay {
    pub fn insert_node(&mut self, id: NodeId) {
        self.nodes.insert(id);
    }

    pub fn insert_outline(&mut self, id: NodeId) {
        self.outlines.insert(id);
    }

    pub fn outline_count(&self) -> usize {
        self.outlines.len()
    }

    pub fn outline_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.outlines.iter().copied()
    }

    pub fn is_node_deleted(&self, id: NodeId) -> bool {
        self.nodes.contains(&id)
    }

    pub fn is_deleted(&self, id: NodeId) -> bool {
        self.is_node_deleted(id) || self.outlines.contains(&id)
    }
}

pub(crate) fn node_context_menu(
    ui: &mut egui::Ui,
    id: NodeId,
    can_zoom_in: bool,
    zoom_in_label: &str,
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
