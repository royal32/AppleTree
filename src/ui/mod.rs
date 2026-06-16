pub mod file_icons;
pub mod tree_view;
pub mod treemap_view;

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
