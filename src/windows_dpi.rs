//! Works around the window flickering between sizes when it's dragged between
//! monitors with different scaling on Windows.
//!
//! When the window crosses monitors, Windows reports a DPI change and winit
//! resizes the window to keep its logical size. If that resize leaves most of
//! the window on the old monitor (easy with monitors stacked vertically, since
//! the window grows downwards from the title bar), Windows reports the DPI
//! changing back, and the two fight for as long as the drag continues.
//!
//! The fix holds back DPI changes while the window is being dragged, then
//! replays the final one when the drag ends, so the window resizes once.

use std::mem;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, GWLP_WNDPROC, GetWindowLongPtrW, GetWindowRect, SetWindowLongPtrW,
    WM_DPICHANGED, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WNDPROC,
};

/// winit's window procedure, which ours forwards messages on to.
static WINIT_PROC: AtomicIsize = AtomicIsize::new(0);
/// Whether the user is dragging or resizing the window.
static IN_SIZE_MOVE: AtomicBool = AtomicBool::new(false);

/// Puts our window procedure in front of winit's. Only call this once, with
/// the app's main window.
pub fn defer_dpi_changes_while_dragging(window: &impl HasWindowHandle) {
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = handle.hwnd.get() as HWND;

    // SAFETY: `hwnd` is the live window eframe created on this thread, and
    // `window_proc` matches the signature Windows expects. winit's procedure is
    // stored before ours is installed, so it's always there to forward to.
    unsafe {
        WINIT_PROC.store(GetWindowLongPtrW(hwnd, GWLP_WNDPROC), Ordering::Relaxed);
        SetWindowLongPtrW(hwnd, GWLP_WNDPROC, window_proc as *const () as isize);
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ENTERSIZEMOVE => IN_SIZE_MOVE.store(true, Ordering::Relaxed),
        // Returning 0 tells Windows we handled it; it's replayed after the drag.
        WM_DPICHANGED if IN_SIZE_MOVE.load(Ordering::Relaxed) => return 0,
        WM_EXITSIZEMOVE => {
            IN_SIZE_MOVE.store(false, Ordering::Relaxed);
            // winit must see the drag end first, or it treats the replayed
            // change as happening mid-drag.
            let result = unsafe { forward(hwnd, msg, wparam, lparam) };
            unsafe { replay_dpi_change(hwnd) };
            return result;
        }
        _ => {}
    }
    unsafe { forward(hwnd, msg, wparam, lparam) }
}

/// Passes a message to winit's window procedure.
unsafe fn forward(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: WINIT_PROC holds the function pointer read from the window in
    // `defer_dpi_changes_while_dragging`, and `WNDPROC` is a nullable function
    // pointer with the same size as `isize`.
    let winit_proc: WNDPROC = unsafe { mem::transmute(WINIT_PROC.load(Ordering::Relaxed)) };
    unsafe { CallWindowProcW(winit_proc, hwnd, msg, wparam, lparam) }
}

/// Tells winit the DPI of the monitor the window was dropped on. winit ignores
/// it when the DPI hasn't changed, e.g. when the drag stayed on one monitor.
unsafe fn replay_dpi_change(hwnd: HWND) {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let mut rect = RECT::default();
    if dpi == 0 || unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return;
    }
    // Like Windows, pack the DPI into both halves of `wparam` (x and y), and
    // suggest the current rect so the window stays where it was dropped.
    let wparam = ((dpi << 16) | dpi) as WPARAM;
    unsafe { forward(hwnd, WM_DPICHANGED, wparam, &rect as *const RECT as LPARAM) };
}
