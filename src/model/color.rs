use egui::Color32;

use crate::settings::TreemapPalette;

pub const PALETTE_BRIGHTNESS: f64 = 0.6;

// 18 distinct hues, normalized below as cushion treemap base colors.
const RAW_PALETTE: [(u8, u8, u8); 18] = [
    (0, 0, 200),    // Blue
    (200, 0, 0),    // Red
    (0, 130, 0),    // Green
    (160, 0, 160),  // Purple
    (180, 100, 0),  // Brown/Orange
    (0, 130, 130),  // Teal
    (180, 0, 80),   // Crimson
    (80, 80, 180),  // Slate Blue
    (0, 100, 60),   // Forest Green
    (150, 60, 0),   // Rust
    (100, 0, 180),  // Violet
    (0, 110, 170),  // Cerulean
    (160, 60, 100), // Mauve
    (100, 120, 0),  // Olive
    (170, 0, 130),  // Magenta
    (0, 90, 120),   // Dark Cyan
    (120, 70, 40),  // Sienna
    (80, 0, 120),   // Indigo
];

/// Normalized palette (brightness 0.6) — used as base for cushion treemap shading.
const PALETTE_TREEMAP: [Color32; 18] = {
    let mut result = [Color32::BLACK; 18];
    let mut i = 0;
    while i < 18 {
        let (r, g, b) = RAW_PALETTE[i];
        let (nr, ng, nb) = normalize_color(r, g, b, PALETTE_BRIGHTNESS);
        result[i] = Color32::from_rgb(nr, ng, nb);
        i += 1;
    }
    result
};

const PASTEL_PALETTE: [Color32; 18] = [
    Color32::from_rgb(126, 182, 255),
    Color32::from_rgb(255, 142, 142),
    Color32::from_rgb(132, 216, 157),
    Color32::from_rgb(206, 158, 232),
    Color32::from_rgb(241, 188, 118),
    Color32::from_rgb(119, 210, 210),
    Color32::from_rgb(239, 139, 179),
    Color32::from_rgb(160, 170, 232),
    Color32::from_rgb(130, 194, 159),
    Color32::from_rgb(224, 160, 118),
    Color32::from_rgb(188, 146, 238),
    Color32::from_rgb(120, 190, 226),
    Color32::from_rgb(224, 154, 184),
    Color32::from_rgb(189, 204, 120),
    Color32::from_rgb(230, 138, 211),
    Color32::from_rgb(116, 176, 197),
    Color32::from_rgb(198, 158, 135),
    Color32::from_rgb(176, 136, 210),
];

const DESATURATED_PALETTE: [Color32; 18] = [
    Color32::from_rgb(98, 118, 152),
    Color32::from_rgb(154, 100, 96),
    Color32::from_rgb(101, 135, 103),
    Color32::from_rgb(134, 104, 142),
    Color32::from_rgb(148, 125, 93),
    Color32::from_rgb(94, 134, 134),
    Color32::from_rgb(148, 96, 118),
    Color32::from_rgb(116, 118, 148),
    Color32::from_rgb(92, 126, 111),
    Color32::from_rgb(142, 112, 88),
    Color32::from_rgb(128, 100, 150),
    Color32::from_rgb(88, 122, 146),
    Color32::from_rgb(145, 108, 124),
    Color32::from_rgb(126, 132, 90),
    Color32::from_rgb(146, 96, 134),
    Color32::from_rgb(84, 116, 128),
    Color32::from_rgb(132, 112, 98),
    Color32::from_rgb(116, 94, 132),
];

/// WinDirStat MakeBrightColor + NormalizeColor algorithm.
/// Scales RGB so average brightness = target, then redistributes overflow.
const fn normalize_color(r: u8, g: u8, b: u8, target: f64) -> (u8, u8, u8) {
    let sum = r as f64 + g as f64 + b as f64;
    if sum < 0.001 {
        let v = (target * 255.0) as u8;
        return (v, v, v);
    }

    let f = 3.0 * target * 255.0 / sum;
    let mut rf = r as f64 * f;
    let mut gf = g as f64 * f;
    let mut bf = b as f64 * f;

    // NormalizeColor: redistribute overflow
    let mut iterations = 0;
    while iterations < 10 {
        let mut overflow = 0.0;
        let mut under_count = 0;

        if rf > 255.0 {
            overflow += rf - 255.0;
            rf = 255.0;
        } else {
            under_count += 1;
        }
        if gf > 255.0 {
            overflow += gf - 255.0;
            gf = 255.0;
        } else {
            under_count += 1;
        }
        if bf > 255.0 {
            overflow += bf - 255.0;
            bf = 255.0;
        } else {
            under_count += 1;
        }

        if overflow < 0.5 || under_count == 0 {
            break;
        }

        let add = overflow / under_count as f64;
        if rf < 255.0 {
            rf += add;
        }
        if gf < 255.0 {
            gf += add;
        }
        if bf < 255.0 {
            bf += add;
        }

        iterations += 1;
    }

    let cr = if rf < 0.0 {
        0
    } else if rf > 255.0 {
        255
    } else {
        rf as u8
    };
    let cg = if gf < 0.0 {
        0
    } else if gf > 255.0 {
        255
    } else {
        gf as u8
    };
    let cb = if bf < 0.0 {
        0
    } else if bf > 255.0 {
        255
    } else {
        bf as u8
    };

    (cr, cg, cb)
}

const DIR_COLOR: Color32 = Color32::from_rgb(100, 100, 100);

/// Maps extensions to cushion treemap base colors.
pub struct ColorMap {
    map: std::collections::HashMap<Box<str>, usize>,
}

impl ColorMap {
    /// Build a color map from sorted extensions (largest first).
    pub fn from_extensions(extensions: &[(Box<str>, u64)]) -> Self {
        let mut map = std::collections::HashMap::new();
        for (i, (ext, _)) in extensions.iter().enumerate() {
            map.insert(ext.clone(), i);
        }
        Self { map }
    }

    /// Get treemap base color for an extension (normalized to brightness 0.6 for cushion shading).
    pub fn get_treemap(&self, extension: &str, palette: TreemapPalette) -> Color32 {
        if extension.is_empty() {
            return DIR_COLOR;
        }
        let index = self
            .map
            .get(extension)
            .copied()
            .unwrap_or_else(|| palette_colors(palette).len() - 1);
        palette_color(palette, index)
    }
}

pub fn palette_colors(palette: TreemapPalette) -> &'static [Color32; 18] {
    match palette {
        TreemapPalette::Classic => &PALETTE_TREEMAP,
        TreemapPalette::Pastel => &PASTEL_PALETTE,
        TreemapPalette::DesaturatedRedFrames => &DESATURATED_PALETTE,
    }
}

pub fn palette_color(palette: TreemapPalette, index: usize) -> Color32 {
    let colors = palette_colors(palette);
    colors[index % colors.len()]
}

pub fn folder_frame_color(palette: TreemapPalette) -> Color32 {
    match palette {
        TreemapPalette::Classic | TreemapPalette::Pastel => Color32::from_rgb(76, 76, 76),
        TreemapPalette::DesaturatedRedFrames => Color32::from_rgb(255, 38, 52),
    }
}

pub fn folder_shell_color(palette: TreemapPalette) -> Color32 {
    match palette {
        TreemapPalette::Classic | TreemapPalette::Pastel => Color32::from_rgb(40, 40, 40),
        TreemapPalette::DesaturatedRedFrames => Color32::from_rgb(82, 24, 28),
    }
}

pub fn folder_header_color(palette: TreemapPalette) -> Color32 {
    match palette {
        TreemapPalette::Classic | TreemapPalette::Pastel => Color32::from_rgb(50, 50, 50),
        TreemapPalette::DesaturatedRedFrames => Color32::from_rgb(122, 26, 34),
    }
}

pub fn folder_content_color(palette: TreemapPalette) -> Color32 {
    match palette {
        TreemapPalette::Classic | TreemapPalette::Pastel => Color32::from_rgb(28, 28, 28),
        TreemapPalette::DesaturatedRedFrames => Color32::from_rgb(30, 26, 26),
    }
}
