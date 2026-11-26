//! Linux (and other freedesktop.org desktops): a desktop entry in
//! `~/.local/share/applications` and an icon in the hicolor theme.
//!
//! Built on every platform for tests; used only on Linux.

use std::path::{Path, PathBuf};

use super::{APP_ID, Env, ICON_PNG};

/// `$XDG_DATA_HOME`, or `~/.local/share`. A relative `XDG_DATA_HOME` is
/// invalid by the spec and ignored.
pub(super) fn data_dir(env: Env, home: &Path) -> PathBuf {
    env("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(".local").join("share"))
}

/// The desktop entry and the icon, under the data directory `data`.
pub(super) fn files(data: &Path) -> [PathBuf; 2] {
    [
        data.join("applications").join(format!("{APP_ID}.desktop")),
        data.join("icons")
            .join("hicolor")
            .join("256x256")
            .join("apps")
            .join(format!("{APP_ID}.png")),
    ]
}

/// The desktop entry that starts `viewer` in `home`.
pub(super) fn desktop_entry(viewer: &Path, home: &Path) -> Result<String, String> {
    let utf8 = |path: &Path| {
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("{} isn't valid UTF-8", path.display()))
    };
    let exec = format!("{} %f", exec_argument(&utf8(viewer)?));
    Ok(format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Copcat\n\
         GenericName=Point Cloud Viewer\n\
         Comment=View COPC LiDAR point clouds, local or streamed over HTTP\n\
         Keywords=LiDAR;COPC;point cloud;LAZ;\n\
         Exec={}\n\
         Path={}\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         Categories=Graphics;Science;\n\
         StartupWMClass={APP_ID}\n",
        escape_value(&exec),
        escape_value(&utf8(home)?),
    ))
}

/// Writes the desktop entry and the icon over any there, and returns them.
pub(super) fn install(data: &Path, viewer: &Path, home: &Path) -> Result<Vec<PathBuf>, String> {
    let [entry, icon] = files(data);
    let contents = desktop_entry(viewer, home)?;
    write(&entry, contents.as_bytes())?;
    write(&icon, ICON_PNG)?;
    Ok(vec![entry, icon])
}

fn write(path: &Path, contents: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    }
    std::fs::write(path, contents).map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

/// One `Exec` argument, quoted: inside quotes `"`, `` ` ``, `$` and `\`
/// are backslash-escaped, and `%` is doubled everywhere.
fn exec_argument(arg: &str) -> String {
    let mut out = String::from("\"");
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' | '\\' => out.push('\\'),
            '%' => out.push('%'),
            _ => {}
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// A string value's escapes, applied after any `Exec` quoting.
fn escape_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}
