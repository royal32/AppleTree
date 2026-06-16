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
}
