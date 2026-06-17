use std::path::Path;
use std::time::{Duration, Instant};

use crate::model::color::ColorMap;
use crate::model::tree::{FileNode, FileTree};
use crate::settings::AppPrefs;
use crate::ui;

#[derive(Clone, Debug)]
pub struct ScanBench {
    pub duration: Duration,
    pub nodes: u64,
    pub bytes: u64,
    pub extensions: usize,
}

#[derive(Clone, Debug)]
pub struct TableSortBench {
    pub duration: Duration,
    pub directories: u64,
    pub sorted_children: u64,
}

#[derive(Clone, Debug)]
pub struct TreemapRenderBench {
    pub layout: Duration,
    pub render: Duration,
    pub total: Duration,
    pub leaves: usize,
    pub pixels: usize,
}

pub fn scan(path: &Path) -> (FileTree, ScanBench) {
    let start = Instant::now();
    let tree = FileTree::scan(path);
    let duration = start.elapsed();
    let bench = ScanBench {
        duration,
        nodes: tree.root.subtree_node_count(),
        bytes: tree.root.size,
        extensions: tree.extensions.len(),
    };
    (tree, bench)
}

pub fn table_sort(tree: &FileTree, prefs: &AppPrefs) -> TableSortBench {
    let mut stats = SortStats::default();
    let start = Instant::now();
    sort_node(&tree.root, prefs, &mut stats);
    TableSortBench {
        duration: start.elapsed(),
        directories: stats.directories,
        sorted_children: stats.sorted_children,
    }
}

pub fn treemap_render(
    tree: &FileTree,
    prefs: &AppPrefs,
    width: usize,
    height: usize,
) -> TreemapRenderBench {
    let color_map = ColorMap::from_extensions(&tree.extensions);
    let bench = ui::treemap_view::benchmark_render(tree, &color_map, prefs, width, height);
    TreemapRenderBench {
        layout: bench.layout,
        render: bench.render,
        total: bench.total,
        leaves: bench.leaves,
        pixels: bench.pixels,
    }
}

#[derive(Default)]
struct SortStats {
    directories: u64,
    sorted_children: u64,
}

fn sort_node(node: &FileNode, prefs: &AppPrefs, stats: &mut SortStats) {
    if node.children.is_empty() {
        return;
    }

    stats.directories += 1;
    stats.sorted_children += node.children.len() as u64;

    let mut indices = (0..node.children.len()).collect::<Vec<_>>();
    indices.sort_by(|&a, &b| {
        ui::tree_view::compare_nodes(&node.children[a], &node.children[b], prefs)
    });
    std::hint::black_box(&indices);

    for child in &node.children {
        sort_node(child, prefs, stats);
    }
}
