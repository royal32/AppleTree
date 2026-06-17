#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn malloc_zone_pressure_relief(zone: *mut libc::c_void, goal: usize) -> usize;
}

#[cfg(target_os = "macos")]
pub(crate) fn pressure_relief() -> usize {
    // NULL zone means all malloc zones; zero goal asks for maximum relief.
    unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn pressure_relief() -> usize {
    0
}
