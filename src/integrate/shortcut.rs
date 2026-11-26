//! Windows: a Start Menu shortcut (`.lnk`), written through the shell's
//! `IShellLinkW`.

use std::path::{Path, PathBuf};

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IPersistFile,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::core::{HSTRING, Interface};

/// The shortcut in the Programs folder `dir`. Its name is what Start search
/// shows.
pub(super) fn path(dir: &Path) -> PathBuf {
    dir.join("Copcat.lnk")
}

/// Writes a shortcut at `path` (over any there) that starts `viewer` in
/// `working_dir`, with the viewer's own icon.
pub(super) fn write(path: &Path, viewer: &Path, working_dir: &Path) -> Result<(), String> {
    // SAFETY: COM is initialised on this thread for the calls below and
    // uninitialised only if this call initialised it. If the thread already
    // has COM in another mode, that's fine to use as it is.
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    // SAFETY: the interfaces are created, used and released in `save`,
    // before COM is uninitialised.
    let saved = unsafe { save(path, viewer, working_dir) };
    if init.is_ok() {
        // SAFETY: balances the successful CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
    saved.map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

/// # Safety
/// COM must be initialised on this thread.
unsafe fn save(path: &Path, viewer: &Path, working_dir: &Path) -> windows::core::Result<()> {
    // SAFETY: COM is initialised (the caller's contract); every pointer
    // passed is an HSTRING that outlives its call.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(viewer))?;
        link.SetWorkingDirectory(&HSTRING::from(working_dir))?;
        link.SetIconLocation(&HSTRING::from(viewer), 0)?;
        link.SetDescription(&HSTRING::from("Copcat point cloud viewer"))?;
        link.cast::<IPersistFile>()?
            .Save(&HSTRING::from(path), true)
    }
}
