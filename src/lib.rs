pub mod app;
pub(crate) mod memory;
pub mod model;
#[cfg(target_os = "macos")]
pub(crate) mod objc_ffi;
pub mod scan;
pub mod settings;
pub mod ui;

use std::time::{SystemTime, UNIX_EPOCH};

/// Format a byte count as a human-readable string (e.g., "1.2 GB", "340.5 MB").
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

pub fn format_count(count: u64) -> String {
    let s = count.to_string();
    let mut result = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

pub fn format_compact_count(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format_count(count)
    } else {
        count.to_string()
    }
}

pub fn format_modified(time: SystemTime) -> String {
    let Ok(duration) = time.duration_since(UNIX_EPOCH) else {
        return String::new();
    };
    let secs = duration.as_secs() as libc::time_t;
    let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
    let ptr = unsafe { libc::localtime_r(&secs, tm.as_mut_ptr()) };
    if ptr.is_null() {
        return String::new();
    }
    let tm = unsafe { tm.assume_init() };
    let mut buf = [0i8; 32];
    let fmt = c"%Y-%m-%d %H:%M";
    let written = unsafe { libc::strftime(buf.as_mut_ptr(), buf.len(), fmt.as_ptr(), &tm) };
    if written == 0 {
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, written) };
    String::from_utf8_lossy(bytes).into_owned()
}
