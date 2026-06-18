use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SplitOrientation {
    LeftRight,
    TopBottom,
}

impl SplitOrientation {
    fn as_str(self) -> &'static str {
        match self {
            Self::LeftRight => "left_right",
            Self::TopBottom => "top_bottom",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "left_right" => Some(Self::LeftRight),
            "top_bottom" => Some(Self::TopBottom),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilenameTruncation {
    Middle,
    End,
}

impl FilenameTruncation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Middle => "middle",
            Self::End => "end",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "middle" => Some(Self::Middle),
            "end" => Some(Self::End),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreemapPalette {
    Classic,
    Pastel,
    DesaturatedRedFrames,
}

impl TreemapPalette {
    pub const ALL: [Self; 3] = [Self::Classic, Self::Pastel, Self::DesaturatedRedFrames];

    pub fn label(self) -> &'static str {
        match self {
            Self::Classic => "Classic",
            Self::Pastel => "Pastel",
            Self::DesaturatedRedFrames => "Desaturated\n(Directory Focused)",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Pastel => "pastel",
            Self::DesaturatedRedFrames => "desaturated_red_frames",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "classic" => Some(Self::Classic),
            "pastel" => Some(Self::Pastel),
            "desaturated_red_frames" => Some(Self::DesaturatedRedFrames),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableColumn {
    Name,
    Size,
    PercentOfParent,
    Items,
    Files,
    Folders,
    Modified,
}

impl TableColumn {
    pub const ALL: [Self; 7] = [
        Self::Name,
        Self::Size,
        Self::PercentOfParent,
        Self::Items,
        Self::Files,
        Self::Folders,
        Self::Modified,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Size => "Size on Disk",
            Self::PercentOfParent => "% Parent",
            Self::Items => "Items",
            Self::Files => "Files",
            Self::Folders => "Folders",
            Self::Modified => "Modified",
        }
    }

    pub fn default_width(self) -> f32 {
        match self {
            Self::Name => 240.0,
            Self::Size => 100.0,
            Self::PercentOfParent => 74.0,
            Self::Items => 70.0,
            Self::Files => 70.0,
            Self::Folders => 70.0,
            Self::Modified => 132.0,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Size => "size",
            Self::PercentOfParent => "parent_pct",
            Self::Items => "items",
            Self::Files => "files",
            Self::Folders => "folders",
            Self::Modified => "modified",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "name" => Some(Self::Name),
            "size" => Some(Self::Size),
            "parent_pct" => Some(Self::PercentOfParent),
            "items" => Some(Self::Items),
            "files" => Some(Self::Files),
            "folders" => Some(Self::Folders),
            "modified" => Some(Self::Modified),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ColumnPrefs {
    pub column: TableColumn,
    pub width: f32,
}

#[derive(Clone, Debug)]
pub struct AppPrefs {
    pub split_orientation: SplitOrientation,
    pub sort_column: TableColumn,
    pub sort_descending: bool,
    pub columns: Vec<ColumnPrefs>,
    pub top_bottom_table_height: f32,
    pub treemap_folder_depth: usize,
    pub treemap_label_depth: usize,
    pub filename_truncation: FilenameTruncation,
    pub treemap_palette: TreemapPalette,
}

impl Default for AppPrefs {
    fn default() -> Self {
        Self {
            split_orientation: SplitOrientation::LeftRight,
            sort_column: TableColumn::Size,
            sort_descending: true,
            columns: TableColumn::ALL
                .into_iter()
                .map(|column| ColumnPrefs {
                    column,
                    width: column.default_width(),
                })
                .collect(),
            top_bottom_table_height: 320.0,
            treemap_folder_depth: 2,
            treemap_label_depth: 1,
            filename_truncation: FilenameTruncation::Middle,
            treemap_palette: TreemapPalette::Classic,
        }
    }
}

impl AppPrefs {
    pub fn load() -> Self {
        let Some(path) = settings_path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };

        let mut prefs = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "split" => {
                    if let Some(split) = SplitOrientation::parse(value.trim()) {
                        prefs.split_orientation = split;
                    }
                }
                "sort" => {
                    if let Some((column, direction)) = value.trim().split_once(':')
                        && let Some(column) = TableColumn::parse(column)
                    {
                        prefs.sort_column = column;
                        prefs.sort_descending = direction == "desc";
                    }
                }
                "columns" => {
                    let columns = parse_columns(value);
                    if !columns.is_empty() {
                        prefs.columns = columns;
                    }
                }
                "top_bottom_table_height" => {
                    if let Ok(height) = value.trim().parse::<f32>() {
                        prefs.top_bottom_table_height = height.clamp(180.0, 1200.0);
                    }
                }
                "treemap_folder_depth" => {
                    if let Ok(depth) = value.trim().parse() {
                        prefs.treemap_folder_depth = depth;
                    }
                }
                "treemap_label_depth" => {
                    if let Ok(depth) = value.trim().parse() {
                        prefs.treemap_label_depth = depth;
                    }
                }
                "filename_truncation" => {
                    if let Some(truncation) = FilenameTruncation::parse(value.trim()) {
                        prefs.filename_truncation = truncation;
                    }
                }
                "treemap_palette" => {
                    if let Some(palette) = TreemapPalette::parse(value.trim()) {
                        prefs.treemap_palette = palette;
                    }
                }
                _ => {}
            }
        }
        prefs.ensure_all_columns();
        prefs
    }

    pub fn save(&self) {
        let Some(path) = settings_path() else {
            return;
        };
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            eprintln!("Failed to create settings directory {:?}: {}", parent, e);
            return;
        }

        let columns = self
            .columns
            .iter()
            .map(|pref| format!("{}:{:.1}", pref.column.id(), pref.width))
            .collect::<Vec<_>>()
            .join(",");
        let direction = if self.sort_descending { "desc" } else { "asc" };
        let text = format!(
            "split={}\nsort={}:{}\ncolumns={}\ntop_bottom_table_height={:.1}\ntreemap_folder_depth={}\ntreemap_label_depth={}\nfilename_truncation={}\ntreemap_palette={}\n",
            self.split_orientation.as_str(),
            self.sort_column.id(),
            direction,
            columns,
            self.top_bottom_table_height,
            self.treemap_folder_depth,
            self.treemap_label_depth,
            self.filename_truncation.as_str(),
            self.treemap_palette.as_str(),
        );
        if let Err(e) = std::fs::write(&path, text) {
            eprintln!("Failed to save settings {:?}: {}", path, e);
        }
    }

    pub fn ensure_all_columns(&mut self) {
        for column in TableColumn::ALL {
            if !self.columns.iter().any(|pref| pref.column == column) {
                self.columns.push(ColumnPrefs {
                    column,
                    width: column.default_width(),
                });
            }
        }
        for pref in self.columns.iter_mut() {
            if pref.column == TableColumn::Size && pref.width < TableColumn::Size.default_width() {
                pref.width = TableColumn::Size.default_width();
            }
        }
    }

    pub fn move_column_left(&mut self, column: TableColumn) {
        if let Some(index) = self.columns.iter().position(|pref| pref.column == column)
            && index > 0
        {
            self.columns.swap(index - 1, index);
        }
    }

    pub fn move_column_right(&mut self, column: TableColumn) {
        if let Some(index) = self.columns.iter().position(|pref| pref.column == column)
            && index + 1 < self.columns.len()
        {
            self.columns.swap(index, index + 1);
        }
    }
}

fn parse_columns(value: &str) -> Vec<ColumnPrefs> {
    value
        .split(',')
        .filter_map(|part| {
            let (column, width) = part.trim().split_once(':')?;
            let column = TableColumn::parse(column)?;
            let width = width.parse().unwrap_or_else(|_| column.default_width());
            Some(ColumnPrefs { column, width })
        })
        .collect()
}

fn settings_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("AppleTree")
            .join("settings.txt"),
    )
}
