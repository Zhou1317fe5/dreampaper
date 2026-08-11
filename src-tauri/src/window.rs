//! Window chrome the OS does not give us for free. macOS handles corners and
//! shadows in AppKit, so everything here is Windows-only.

/// Ask DWM for rounded corners (Windows 11 only).
///
/// `DWMWCP_DEFAULT` means "let the system decide", not "always round", so the
/// preference has to be set explicitly. Windows 10 has no such attribute and
/// returns E_INVALIDARG — there is no rounded-window path there at all, so the
/// result is dropped rather than surfaced as an error.
///
/// The FFI is declared by hand instead of pulling in `windows`/`windows-sys`:
/// dwmapi's entry point is stable Win32 ABI, whereas the crates rename these
/// constants and types across major versions.
#[cfg(windows)]
pub fn round_corners(window: &tauri::WebviewWindow) {
    use std::ffi::c_void;

    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: u32 = 2;

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *mut c_void,
            attribute: u32,
            value: *const c_void,
            size: u32,
        ) -> i32;
    }

    let Ok(hwnd) = window.hwnd() else { return };
    let preference = DWMWCP_ROUND;
    // SAFETY: hwnd comes from Tauri and points at the live main window; the
    // attribute expects exactly one 4-byte enum, and both the pointer and the
    // length are taken from `preference`.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd.0,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::from_ref(&preference).cast(),
            std::mem::size_of_val(&preference) as u32,
        );
    }
}

#[cfg(not(windows))]
pub fn round_corners(_window: &tauri::WebviewWindow) {}
