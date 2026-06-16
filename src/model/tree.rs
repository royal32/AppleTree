use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

use crate::scan::getattrlistbulk::{self, DirEntry};

/// Stable scan-local identity for a node.
pub type NodeId = u64;

/// Index path from root to a node in the tree (e.g. [2, 0, 1] = root's 3rd child → 1st child → 2nd child).
pub type TreePath = Vec<usize>;

thread_local! {
    static LOCAL_EXT_MAP: RefCell<HashMap<Box<str>, u64>> = RefCell::new(HashMap::new());
}

fn raw_extension(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() => ext,
        _ => "",
    }
}

/// A node in the file tree. Uses compact representation (Box<str> + Box<[T]>).
pub struct FileNode {
    pub id: NodeId,
    pub name: Box<str>,
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
        for child in self.children.iter() {
            if let Some(node) = child.resolve_id(id) {
                return Some(node);
            }
        }
        None
    }

    pub fn path_to_id(&self, id: NodeId) -> Option<TreePath> {
        let mut path = Vec::new();
        if self.find_path_to_id(id, &mut path) {
            Some(path)
        } else {
            None
        }
    }

    fn find_path_to_id(&self, id: NodeId, path: &mut TreePath) -> bool {
        if self.id == id {
            return true;
        }
        for (idx, child) in self.children.iter().enumerate() {
            path.push(idx);
            if child.find_path_to_id(id, path) {
                return true;
            }
            path.pop();
        }
        false
    }
}

/// The complete scanned file tree with precomputed extension statistics.
pub struct FileTree {
    pub root: FileNode,
    pub root_path: String,
    /// Extension -> total bytes mapping, sorted by size descending.
    pub extensions: Vec<(Box<str>, u64)>,
}

impl FileTree {
    /// Build a file tree by scanning the given path using getattrlistbulk.
    pub fn scan(root: &Path) -> Self {
        let ext_map = Mutex::new(HashMap::<Box<str>, u64>::new());
        let mut root_node = build_root_node(root);
        let mut next_id = 1;
        assign_node_ids(&mut root_node, &mut next_id);

        // Drain the main thread's local ext map
        LOCAL_EXT_MAP.with(|m| {
            let local = m.replace(HashMap::new());
            if !local.is_empty() {
                let mut global = ext_map.lock().unwrap_or_else(|e| e.into_inner());
                for (k, v) in local {
                    *global.entry(k).or_default() += v;
                }
            }
        });

        // Drain all rayon worker thread local ext maps
        rayon::broadcast(|_| {
            LOCAL_EXT_MAP.with(|m| {
                let local = m.replace(HashMap::new());
                if !local.is_empty() {
                    let mut global = ext_map.lock().unwrap_or_else(|e| e.into_inner());
                    for (k, v) in local {
                        *global.entry(k).or_default() += v;
                    }
                }
            });
        });

        let mut extensions: Vec<(Box<str>, u64)> = ext_map
            .into_inner()
            .unwrap_or_else(|e| e.into_inner())
            .into_iter()
            .collect();
        extensions.sort_unstable_by_key(|(_, size)| Reverse(*size));

        FileTree {
            root: root_node,
            root_path: root.display().to_string(),
            extensions,
        }
    }

    /// Build the full filesystem path for a node identified by index path.
    pub fn build_fs_path(&self, path: &[usize]) -> Option<std::path::PathBuf> {
        let mut fs_path = std::path::PathBuf::from(&self.root_path);
        let mut node = &self.root;
        for &idx in path {
            let child = node.children.get(idx)?;
            fs_path.push(&*child.name);
            node = child;
        }
        Some(fs_path)
    }

    pub fn build_fs_path_for_id(&self, id: NodeId) -> Option<std::path::PathBuf> {
        self.root
            .path_to_id(id)
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
    let node = build_node_fd(fd, name, modified);
    getattrlistbulk::close_dir(fd);
    node
}

fn root_display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

/// Build a FileNode from an already-opened directory fd.
/// `node_name` is the display name for this node.
fn build_node_fd(
    parent_fd: libc::c_int,
    node_name: Box<str>,
    modified: Option<SystemTime>,
) -> FileNode {
    use rayon::prelude::*;

    let entries = getattrlistbulk::scan_dir_entries_fd(parent_fd);

    // Separate files and directories
    let mut file_nodes: Vec<FileNode> = Vec::new();
    let mut dir_names: Vec<&DirEntry> = Vec::new();
    let mut total_size: u64 = 0;
    let mut total_file_count: u64 = 0;

    for entry in &entries {
        if entry.is_dir {
            dir_names.push(entry);
        } else {
            total_size += entry.file_size;
            total_file_count += 1;
            LOCAL_EXT_MAP.with(|m| {
                let mut map = m.borrow_mut();
                let ext = raw_extension(&entry.name);
                let key: Box<str> = if ext.is_empty() {
                    "(no ext)".into()
                } else {
                    ext.into()
                };
                *map.entry(key).or_default() += entry.file_size;
            });
            file_nodes.push(FileNode {
                id: 0,
                name: entry.name.clone(),
                size: entry.file_size,
                is_dir: false,
                children: Box::new([]),
                modified: entry.modified,
                file_count: 1,
                dir_count: 0,
            });
        }
    }

    // Recurse into subdirectories — use openat() relative to parent fd
    let build_child = |entry: &&DirEntry| -> FileNode {
        let child_fd = getattrlistbulk::openat_dir(parent_fd, &entry.name);
        let node = build_node_fd(child_fd, entry.name.clone(), entry.modified);
        getattrlistbulk::close_dir(child_fd);
        node
    };

    let dir_nodes: Vec<FileNode> = if dir_names.len() >= 2 {
        dir_names.par_iter().map(build_child).collect()
    } else {
        dir_names.iter().map(build_child).collect()
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
            size,
            is_dir: true,
            children: children.into(),
            modified: None,
            file_count,
            dir_count,
        }
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
    fn root_display_name_ignores_trailing_slash() {
        assert_eq!(root_display_name(Path::new("/tmp/example/")), "example");
    }
}
