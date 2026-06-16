use std::collections::HashMap;

use egui::{Color32, ColorImage, Rect, TextureHandle, TextureOptions, pos2, vec2};

const ICON_PX: u32 = 32;

#[cfg(target_os = "macos")]
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    fn NSFileTypeForHFSTypeCode(hfs_file_type_code: u32) -> crate::objc_ffi::Id;
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum IconKey {
    Folder,
    Extension(Box<str>),
}

impl IconKey {
    fn for_name(is_dir: bool, name: &str) -> Self {
        if is_dir {
            return Self::Folder;
        }

        let ext = name
            .rsplit_once('.')
            .map(|(_, ext)| ext)
            .filter(|ext| !ext.is_empty())
            .unwrap_or("")
            .to_ascii_lowercase()
            .into_boxed_str();
        Self::Extension(ext)
    }

    fn texture_name(&self) -> String {
        match self {
            Self::Folder => "finder_icon_folder".to_owned(),
            Self::Extension(ext) if ext.is_empty() => "finder_icon_file".to_owned(),
            Self::Extension(ext) => format!("finder_icon_ext_{ext}"),
        }
    }
}

#[derive(Default)]
pub struct FileIconCache {
    textures: HashMap<IconKey, Option<TextureHandle>>,
}

impl FileIconCache {
    pub fn texture_for(
        &mut self,
        ctx: &egui::Context,
        is_dir: bool,
        name: &str,
    ) -> Option<&TextureHandle> {
        let key = IconKey::for_name(is_dir, name);
        if !self.textures.contains_key(&key) {
            let texture = load_icon_image(&key)
                .map(|image| ctx.load_texture(key.texture_name(), image, TextureOptions::LINEAR));
            self.textures.insert(key.clone(), texture);
        }
        self.textures.get(&key).and_then(Option::as_ref)
    }
}

pub fn paint_fallback_folder_icon(painter: &egui::Painter, rect: Rect) {
    let tab_color = Color32::from_rgb(64, 152, 226);
    let body_color = Color32::from_rgb(86, 182, 249);
    let x = rect.min.x;
    let y = rect.min.y;
    let w = rect.width();
    let h = rect.height();

    let tab = Rect::from_min_size(pos2(x, y + 0.5), vec2(w * 0.42, h * 0.28));
    painter.rect_filled(tab, 1.5, tab_color);

    let body = Rect::from_min_size(pos2(x, y + h * 0.22), vec2(w, h * 0.78));
    painter.rect_filled(body, 2.0, body_color);
}

pub fn paint_fallback_file_icon(painter: &egui::Painter, rect: Rect) {
    let fill = Color32::from_rgb(214, 218, 224);
    let fold = Color32::from_rgb(168, 176, 188);
    painter.rect_filled(rect, 2.0, fill);

    let fold_size = rect.width().min(rect.height()) * 0.34;
    painter.add(egui::Shape::convex_polygon(
        vec![
            pos2(rect.right() - fold_size, rect.top()),
            pos2(rect.right(), rect.top() + fold_size),
            pos2(rect.right() - fold_size, rect.top() + fold_size),
        ],
        fold,
        egui::Stroke::NONE,
    ));

    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, Color32::from_rgb(130, 138, 150)),
        egui::StrokeKind::Inside,
    );
}

fn load_icon_image(key: &IconKey) -> Option<ColorImage> {
    let png = load_platform_icon_png(key)?;
    let rgba = image::load_from_memory(&png).ok()?.to_rgba8();
    let resized = image::imageops::resize(
        &rgba,
        ICON_PX,
        ICON_PX,
        image::imageops::FilterType::Lanczos3,
    );
    let pixels = resized.into_raw();
    Some(ColorImage::from_rgba_unmultiplied(
        [ICON_PX as usize, ICON_PX as usize],
        &pixels,
    ))
}

#[cfg(target_os = "macos")]
fn load_platform_icon_png(key: &IconKey) -> Option<Vec<u8>> {
    use crate::objc_ffi::*;

    unsafe {
        let pool_cls = objc_getClass(c"NSAutoreleasePool".as_ptr());
        let pool = send0(pool_cls, sel_registerName(c"new".as_ptr()));
        let result = load_macos_icon_png_inner(key);
        if !pool.is_null() {
            send0_void(pool, sel_registerName(c"drain".as_ptr()));
        }
        result
    }
}

#[cfg(target_os = "macos")]
unsafe fn load_macos_icon_png_inner(key: &IconKey) -> Option<Vec<u8>> {
    use std::slice;

    use crate::objc_ffi::*;

    let workspace_cls = unsafe { objc_getClass(c"NSWorkspace".as_ptr()) };
    let workspace = unsafe { send0(workspace_cls, sel_registerName(c"sharedWorkspace".as_ptr())) };
    if workspace.is_null() {
        return None;
    }

    let file_type = match key {
        IconKey::Folder => unsafe { generic_hfs_file_type(*b"fldr") },
        IconKey::Extension(ext) if ext.is_empty() => unsafe { generic_hfs_file_type(*b"docu") },
        IconKey::Extension(ext) => unsafe { nsstring(ext) },
    };
    let icon = unsafe {
        send1(
            workspace,
            sel_registerName(c"iconForFileType:".as_ptr()),
            file_type,
        )
    };
    if icon.is_null() {
        return None;
    }

    let tiff = unsafe { send0(icon, sel_registerName(c"TIFFRepresentation".as_ptr())) };
    if tiff.is_null() {
        return None;
    }

    let rep_cls = unsafe { objc_getClass(c"NSBitmapImageRep".as_ptr()) };
    let rep = unsafe {
        send1(
            rep_cls,
            sel_registerName(c"imageRepWithData:".as_ptr()),
            tiff,
        )
    };
    if rep.is_null() {
        return None;
    }

    let dict_cls = unsafe { objc_getClass(c"NSDictionary".as_ptr()) };
    let props = unsafe { send0(dict_cls, sel_registerName(c"dictionary".as_ptr())) };
    let png_data = unsafe {
        send2_isize_id(
            rep,
            sel_registerName(c"representationUsingType:properties:".as_ptr()),
            4,
            props,
        )
    };
    if png_data.is_null() {
        return None;
    }

    let len = unsafe { send0_usize(png_data, sel_registerName(c"length".as_ptr())) };
    let bytes = unsafe { send0_ptr::<u8>(png_data, sel_registerName(c"bytes".as_ptr())) };
    if len == 0 || bytes.is_null() {
        return None;
    }

    Some(unsafe { slice::from_raw_parts(bytes, len) }.to_vec())
}

#[cfg(target_os = "macos")]
unsafe fn generic_hfs_file_type(code: [u8; 4]) -> crate::objc_ffi::Id {
    unsafe { NSFileTypeForHFSTypeCode(u32::from_be_bytes(code)) }
}

#[cfg(not(target_os = "macos"))]
fn load_platform_icon_png(_key: &IconKey) -> Option<Vec<u8>> {
    None
}
