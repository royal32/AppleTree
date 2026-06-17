use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use crate::scan::getattrlistbulk::{self, DirEntry};

/// Stable scan-local identity for a node.
pub type NodeId = u64;

/// Index path from root to a node in the tree (e.g. [2, 0, 1] = root's 3rd child -> 1st child -> 2nd child).
pub type TreePath = Vec<usize>;

fn raw_extension(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() => ext,
        _ => "",
    }
}

fn extension_key(name: &str) -> &str {
    let ext = raw_extension(name);
    if ext.is_empty() { "(no ext)" } else { ext }
}

/// A node in the file tree. Uses compact representation (Box<str> + Box<[T]>).
pub struct FileNode {
    pub id: NodeId,
    pub name: Box<str>,
    /// Absolute filesystem path for scan-root nodes.
    pub source_path: Option<Box<str>>,
    /// Allocated bytes on disk. Directory values are aggregated from descendants.
    pub size: u64,
    pub is_dir: bool,
    pub children: Box<[FileNode]>,
    pub modified: Option<SystemTime>,
    /// Cached file count (1 for files, sum of children for dirs).
    pub file_count: u64,
    /// Cached directory count (0 for files, 1 + sum of children for dirs).
    pub dir_count: u64,
}

impl FileNode {
    pub fn subtree_node_count(&self) -> u64 {
        self.file_count + self.dir_count
    }

    pub fn contains_id(&self, id: NodeId) -> bool {
        id >= self.id && id < self.subtree_end()
    }

    /// Get the file extension, or empty string for dirs/extensionless files.
    pub fn extension(&self) -> &str {
        if self.is_dir {
            ""
        } else {
            raw_extension(&self.name)
        }
    }

    /// Walk a path of child indices to reach a descendant node.
    pub fn resolve_path(&self, path: &[usize]) -> Option<&FileNode> {
        let mut node = self;
        for &idx in path {
            node = node.children.get(idx)?;
        }
        Some(node)
    }

    pub fn resolve_id(&self, id: NodeId) -> Option<&FileNode> {
        if self.id == id {
            return Some(self);
        }
        let child = self.child_containing_id(id)?;
        child.resolve_id(id)
    }

    pub fn path_to_id(&self, id: NodeId) -> Option<TreePath> {
        if !self.contains_id(id) {
            return None;
        }
        let mut path = Vec::new();
        let mut node = self;
        while node.id != id {
            let idx = node.child_index_containing_id(id)?;
            path.push(idx);
            node = &node.children[idx];
        }
        Some(path)
    }

    fn subtree_end(&self) -> NodeId {
        self.id.saturating_add(self.subtree_node_count())
    }

    fn child_containing_id(&self, id: NodeId) -> Option<&FileNode> {
        self.child_index_containing_id(id)
            .and_then(|idx| self.children.get(idx))
    }

    fn child_index_containing_id(&self, id: NodeId) -> Option<usize> {
        if !self.contains_id(id) || id == self.id {
            return None;
        }
        let idx = self.children.partition_point(|child| child.id <= id);
        let idx = idx.checked_sub(1)?;
        self.children[idx].contains_id(id).then_some(idx)
    }
}

/// The complete scanned file tree with precomputed extension statistics.
pub struct FileTree {
    pub root: FileNode,
    pub root_path: String,
    /// Extension -> total allocated bytes mapping, sorted by size descending.
    pub extensions: Vec<(Box<str>, u64)>,
}

impl FileTree {
    /// Build a file tree by scanning the given path using getattrlistbulk.
    pub fn scan(root: &Path) -> Self {
        let roots = [root.to_path_buf()];
        Self::scan_paths(&roots)
    }

    /// Build a file tree from one or more scan roots.
    pub fn scan_paths(roots: &[std::path::PathBuf]) -> Self {
        let mut root_node = if roots.len() == 1 {
            build_root_node(&roots[0])
        } else {
            build_virtual_root(roots)
        };
        let mut next_id = 1;
        assign_node_ids(&mut root_node, &mut next_id);

        let mut extensions: Vec<(Box<str>, u64)> =
            extension_totals(&root_node).into_iter().collect();
        extensions.sort_unstable_by_key(|(_, size)| Reverse(*size));

        FileTree {
            root: root_node,
            root_path: if roots.len() == 1 {
                roots
                    .first()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            },
            extensions,
        }
    }

    /// Build the full filesystem path for a node identified by index path.
    pub fn build_fs_path(&self, path: &[usize]) -> Option<std::path::PathBuf> {
        if path.is_empty() && self.root.source_path.is_none() && self.root_path.is_empty() {
            return None;
        }
        let mut fs_path = self
            .root
            .source_path
            .as_deref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(&self.root_path));
        let mut node = &self.root;
        for &idx in path {
            let child = node.children.get(idx)?;
            if let Some(source_path) = child.source_path.as_deref() {
                fs_path = std::path::PathBuf::from(source_path);
            } else {
                fs_path.push(&*child.name);
            }
            node = child;
        }
        Some(fs_path)
    }

    pub fn node(&self, id: NodeId) -> Option<&FileNode> {
        self.root.resolve_id(id)
    }

    pub fn path_for_id(&self, id: NodeId) -> Option<TreePath> {
        self.root.path_to_id(id)
    }

    pub fn parent_id(&self, id: NodeId) -> Option<NodeId> {
        let path = self.path_for_id(id)?;
        let (_, parent_path) = path.split_last()?;
        self.root.resolve_path(parent_path).map(|node| node.id)
    }

    pub fn contains(&self, root_id: NodeId, descendant_id: NodeId) -> bool {
        self.node(root_id)
            .is_some_and(|root| root.contains_id(descendant_id))
    }

    pub fn build_fs_path_for_id(&self, id: NodeId) -> Option<std::path::PathBuf> {
        self.path_for_id(id)
            .and_then(|path| self.build_fs_path(&path))
    }

    pub fn full_display_path_for_id(&self, id: NodeId) -> Option<String> {
        self.build_fs_path_for_id(id)
            .map(|path| path.display().to_string())
    }
}

fn assign_node_ids(node: &mut FileNode, next_id: &mut NodeId) {
    node.id = *next_id;
    *next_id += 1;
    for child in node.children.iter_mut() {
        assign_node_ids(child, next_id);
    }
}

fn build_root_node(path: &Path) -> FileNode {
    let fd = getattrlistbulk::open_dir(path);
    if fd < 0 {
        eprintln!(
            "Warning: could not open directory {:?} (permission denied or not found)",
            path
        );
    }
    let name: Box<str> = root_display_name(path).into();
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let mut node = build_node_fd(fd, name, modified);
    node.source_path = Some(path.display().to_string().into());
    getattrlistbulk::close_dir(fd);
    node
}

fn build_virtual_root(roots: &[std::path::PathBuf]) -> FileNode {
    let mut children = roots
        .iter()
        .filter(|path| path.is_dir())
        .map(|path| build_root_node(path))
        .collect::<Vec<_>>();
    children.sort_unstable_by_key(|child| Reverse(child.size));

    let size = children.iter().map(|child| child.size).sum();
    let file_count = children.iter().map(|child| child.file_count).sum();
    let dir_count = 1 + children.iter().map(|child| child.dir_count).sum::<u64>();

    FileNode {
        id: 0,
        name: "Scan Scope".into(),
        source_path: None,
        size,
        is_dir: true,
        children: children.into(),
        modified: None,
        file_count,
        dir_count,
    }
}

fn root_display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

fn extension_totals(root: &FileNode) -> HashMap<Box<str>, u64> {
    let mut totals = HashMap::new();
    collect_extension_totals(root, &mut totals);
    totals
}

fn collect_extension_totals(node: &FileNode, totals: &mut HashMap<Box<str>, u64>) {
    if node.is_dir {
        for child in node.children.iter() {
            collect_extension_totals(child, totals);
        }
    } else {
        let key = extension_key(&node.name);
        if let Some(total) = totals.get_mut(key) {
            *total += node.size;
        } else {
            totals.insert(key.into(), node.size);
        }
    }
}

/// Build a FileNode from an already-opened directory fd.
/// `node_name` is the display name for this node.
fn build_node_fd(
    parent_fd: libc::c_int,
    node_name: Box<str>,
    modified: Option<SystemTime>,
) -> FileNode {
    use rayon::prelude::*;

    let mut file_nodes: Vec<FileNode> = Vec::new();
    let mut dir_entries: Vec<DirEntry> = Vec::new();
    let mut total_size: u64 = 0;
    let mut total_file_count: u64 = 0;

    getattrlistbulk::scan_dir_entries_fd(parent_fd, |entry| {
        if entry.is_dir {
            dir_entries.push(entry);
        } else {
            total_size += entry.disk_size;
            total_file_count += 1;
            file_nodes.push(FileNode {
                id: 0,
                name: entry.name,
                source_path: None,
                size: entry.disk_size,
                is_dir: false,
                children: Box::new([]),
                modified: entry.modified,
                file_count: 1,
                dir_count: 0,
            });
        }
    });

    // Recurse into subdirectories — use openat() relative to parent fd
    let build_child = |entry: &DirEntry| -> FileNode {
        let child_fd = getattrlistbulk::openat_dir(parent_fd, &entry.name);
        let node = build_node_fd(child_fd, entry.name.clone(), entry.modified);
        getattrlistbulk::close_dir(child_fd);
        node
    };

    let dir_nodes: Vec<FileNode> = if dir_entries.len() >= 2 {
        dir_entries.par_iter().map(build_child).collect()
    } else {
        dir_entries.iter().map(build_child).collect()
    };

    let mut total_dir_count: u64 = 0;
    for child in &dir_nodes {
        total_size += child.size;
        total_file_count += child.file_count;
        total_dir_count += child.dir_count;
    }

    let mut children: Vec<FileNode> = Vec::with_capacity(file_nodes.len() + dir_nodes.len());
    children.extend(file_nodes);
    children.extend(dir_nodes);

    children.sort_unstable_by_key(|child| Reverse(child.size));

    FileNode {
        id: 0,
        name: node_name,
        source_path: None,
        size: total_size,
        is_dir: true,
        children: children.into(),
        modified,
        file_count: total_file_count,
        dir_count: total_dir_count + 1,
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

    fn source_dir(id: NodeId, name: &str, source_path: &str, children: Vec<FileNode>) -> FileNode {
        let mut node = dir(id, name, children);
        node.source_path = Some(source_path.into());
        node
    }

    #[test]
    fn resolves_node_ids_and_paths() {
        let root = dir(
            1,
            "/tmp/root",
            vec![
                dir(2, "a", vec![file(3, "nested.txt", 10)]),
                file(4, "b.bin", 20),
            ],
        );

        assert_eq!(
            root.resolve_id(3).map(|node| &*node.name),
            Some("nested.txt")
        );
        assert_eq!(root.path_to_id(3), Some(vec![0, 0]));
        assert_eq!(root.path_to_id(4), Some(vec![1]));
        assert_eq!(root.path_to_id(99), None);
    }

    #[test]
    fn file_tree_builds_paths_from_node_ids() {
        let tree = FileTree {
            root: dir(
                1,
                "/tmp/root",
                vec![dir(2, "a", vec![file(3, "nested.txt", 10)])],
            ),
            root_path: "/tmp/root".to_owned(),
            extensions: Vec::new(),
        };

        assert_eq!(
            tree.build_fs_path_for_id(3)
                .map(|path| path.display().to_string()),
            Some("/tmp/root/a/nested.txt".to_owned())
        );
        assert_eq!(
            tree.full_display_path_for_id(2),
            Some("/tmp/root/a".to_owned())
        );
    }

    #[test]
    fn file_tree_uses_preorder_ranges_for_parent_and_containment() {
        let tree = FileTree {
            root: dir(
                1,
                "/tmp/root",
                vec![
                    dir(2, "a", vec![file(3, "nested.txt", 10)]),
                    file(4, "b.bin", 20),
                ],
            ),
            root_path: "/tmp/root".to_owned(),
            extensions: Vec::new(),
        };

        assert_eq!(tree.node(2).map(|node| &*node.name), Some("a"));
        assert_eq!(tree.parent_id(3), Some(2));
        assert_eq!(tree.parent_id(1), None);
        assert!(tree.contains(1, 3));
        assert!(tree.contains(2, 3));
        assert!(!tree.contains(2, 4));
        assert!(!tree.contains(99, 3));
    }

    #[test]
    fn virtual_scan_root_builds_paths_from_source_roots() {
        let tree = FileTree {
            root: dir(
                1,
                "Scan Scope",
                vec![source_dir(
                    2,
                    "Applications",
                    "/Applications",
                    vec![file(3, "Example.app", 7)],
                )],
            ),
            root_path: String::new(),
            extensions: Vec::new(),
        };

        assert_eq!(
            tree.build_fs_path_for_id(3).as_deref(),
            Some(Path::new("/Applications/Example.app"))
        );
        assert!(tree.build_fs_path_for_id(1).is_none());
    }

    #[test]
    fn root_display_name_ignores_trailing_slash() {
        assert_eq!(root_display_name(Path::new("/tmp/example/")), "example");
    }
}
