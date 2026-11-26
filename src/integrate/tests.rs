//! Everything here works in temporary directories: nothing touches the real
//! Start Menu, `~/.local/share` or `~/.copcat`.

use std::cell::Cell;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::*;

fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
    move |name| {
        pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| OsString::from(value))
    }
}

#[test]
fn the_viewer_is_found_beside_either_program() {
    let dir = Path::new("bin");
    let viewer = dir.join(format!("copcat-viewer{}", std::env::consts::EXE_SUFFIX));
    let copcat = dir.join(format!("copcat{}", std::env::consts::EXE_SUFFIX));
    assert_eq!(viewer_beside(&copcat), viewer);
    assert_eq!(viewer_beside(&viewer), viewer);
}

#[test]
fn the_start_menu_is_under_appdata() {
    let env = env_of(&[("APPDATA", r"C:\Users\me\AppData\Roaming")]);
    assert_eq!(
        start_menu_dir(&env),
        Some(PathBuf::from(
            r"C:\Users\me\AppData\Roaming\Microsoft\Windows\Start Menu\Programs"
        ))
    );
    assert_eq!(start_menu_dir(&env_of(&[])), None);
}

#[test]
fn the_data_dir_is_xdg_data_home_if_absolute() {
    let home = Path::new("/home/me");
    let absolute = std::env::temp_dir().join("xdg-data");
    let absolute_env = move |name: &str| (name == "XDG_DATA_HOME").then(|| absolute.clone().into());
    assert_eq!(
        freedesktop::data_dir(&absolute_env, home),
        std::env::temp_dir().join("xdg-data")
    );
    assert_eq!(
        freedesktop::data_dir(&env_of(&[]), home),
        home.join(".local").join("share")
    );
    // A relative XDG_DATA_HOME is invalid and ignored.
    assert_eq!(
        freedesktop::data_dir(&env_of(&[("XDG_DATA_HOME", "relative/data")]), home),
        home.join(".local").join("share")
    );
}

#[test]
fn freedesktop_files_are_the_entry_and_a_hicolor_icon() {
    let data = Path::new("/home/me/.local/share");
    assert_eq!(
        freedesktop::files(data),
        [
            data.join("applications").join("copcat.desktop"),
            data.join("icons")
                .join("hicolor")
                .join("256x256")
                .join("apps")
                .join("copcat.png"),
        ]
    );
}

#[test]
fn the_desktop_entry_starts_the_viewer() {
    let entry = freedesktop::desktop_entry(
        Path::new("/home/me/.local/bin/copcat-viewer"),
        Path::new("/home/me"),
    )
    .unwrap();
    assert_eq!(
        entry,
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Copcat\n\
         GenericName=Point Cloud Viewer\n\
         Comment=View COPC LiDAR point clouds, local or streamed over HTTP\n\
         Keywords=LiDAR;COPC;point cloud;LAZ;\n\
         Exec=\"/home/me/.local/bin/copcat-viewer\" %f\n\
         Path=/home/me\n\
         Icon=copcat\n\
         Terminal=false\n\
         Categories=Graphics;Science;\n\
         StartupWMClass=copcat\n"
    );
}

#[test]
fn the_desktop_entry_quotes_awkward_paths() {
    let entry = freedesktop::desktop_entry(
        Path::new("/home/me/my $apps/100%/copcat-viewer"),
        Path::new("/home/me"),
    )
    .unwrap();
    // `$` is backslash-escaped for the Exec quoting, and that backslash is
    // escaped again as a string value; `%` is doubled.
    assert!(
        entry.contains("Exec=\"/home/me/my \\\\$apps/100%%/copcat-viewer\" %f\n"),
        "{entry}"
    );
}

#[test]
fn freedesktop_install_writes_both_files_and_remove_takes_them_out() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("share");
    let viewer = Path::new("/opt/copcat/copcat-viewer");
    let home = Path::new("/home/me");

    let written = freedesktop::install(&data, viewer, home).unwrap();
    assert_eq!(written, freedesktop::files(&data));
    let [entry, icon] = freedesktop::files(&data);
    assert_eq!(
        std::fs::read_to_string(&entry).unwrap(),
        freedesktop::desktop_entry(viewer, home).unwrap()
    );
    assert_eq!(std::fs::read(&icon).unwrap(), ICON_PNG);

    // Again, over the top: same files.
    assert_eq!(freedesktop::install(&data, viewer, home).unwrap(), written);

    assert_eq!(remove_files(&written).unwrap(), written);
    assert!(!entry.exists() && !icon.exists());
}

#[test]
fn removing_what_isnt_there_is_fine() {
    let temp = tempfile::tempdir().unwrap();
    let files = [temp.path().join("a.lnk"), temp.path().join("b.desktop")];
    assert_eq!(remove_files(&files).unwrap(), Vec::<PathBuf>::new());

    let integration = Integration {
        viewer: temp.path().join("copcat-viewer"),
        home: temp.path().to_owned(),
        dir: temp.path().join("launcher"),
    };
    assert_eq!(integration.remove().unwrap(), Vec::<PathBuf>::new());
}

#[test]
fn first_run_registers_once_and_leaves_a_marker() {
    let temp = tempfile::tempdir().unwrap();
    let user_dir = temp.path().join(".copcat");
    let calls = Cell::new(0);
    let register = || {
        calls.set(calls.get() + 1);
        Ok(vec![PathBuf::from("written")])
    };

    assert_eq!(
        first_run(&user_dir, register),
        FirstRun::Registered(vec![PathBuf::from("written")])
    );
    assert!(user_dir.join(MARKER_FILE).is_file());
    assert_eq!(first_run(&user_dir, register), FirstRun::AlreadyDone);
    assert_eq!(calls.get(), 1);
}

#[test]
fn a_failed_first_run_is_not_retried() {
    let temp = tempfile::tempdir().unwrap();
    let calls = Cell::new(0);
    let register = || {
        calls.set(calls.get() + 1);
        Err("no Start Menu".to_owned())
    };
    assert_eq!(
        first_run(temp.path(), register),
        FirstRun::Failed("no Start Menu".to_owned())
    );
    assert_eq!(first_run(temp.path(), register), FirstRun::AlreadyDone);
    assert_eq!(calls.get(), 1);
}

#[test]
fn a_removed_entry_stays_removed() {
    // What `copcat integrate --remove` does: it removes the files and leaves
    // the marker, so the next start doesn't put them back.
    let temp = tempfile::tempdir().unwrap();
    let user_dir = temp.path().join(".copcat");
    let data = temp.path().join("share");
    let install = || freedesktop::install(&data, Path::new("/x/copcat-viewer"), temp.path());

    let FirstRun::Registered(files) = first_run(&user_dir, install) else {
        panic!("didn't register");
    };
    remove_files(&files).unwrap();
    assert_eq!(first_run(&user_dir, install), FirstRun::AlreadyDone);
    assert!(files.iter().all(|file| !file.exists()));
}

#[test]
fn nothing_is_registered_without_a_marker_to_write() {
    let temp = tempfile::tempdir().unwrap();
    // A file where the settings directory should be.
    let not_a_dir = temp.path().join("file");
    std::fs::write(&not_a_dir, "").unwrap();
    let calls = Cell::new(0);
    let register = || {
        calls.set(calls.get() + 1);
        Ok(Vec::new())
    };
    assert_eq!(first_run(&not_a_dir, register), FirstRun::Skipped);
    assert_eq!(calls.get(), 0);
}

#[test]
fn cargo_target_dirs_are_recognised() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    let exe = target.join("debug").join("copcat");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    assert!(!in_cargo_target_dir(&exe));
    std::fs::write(
        target.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55",
    )
    .unwrap();
    assert!(in_cargo_target_dir(&exe));
    // This test itself runs from one.
    assert!(in_cargo_target_dir(&std::env::current_exe().unwrap()));
}

#[test]
fn the_icon_is_a_256_pixel_png() {
    let icon = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png).unwrap();
    assert_eq!((icon.width(), icon.height()), (256, 256));
}

#[cfg(windows)]
#[test]
fn a_real_shortcut_is_written_and_removed() {
    let temp = tempfile::tempdir().unwrap();
    let integration = Integration {
        viewer: std::env::current_exe().unwrap(),
        home: temp.path().to_owned(),
        dir: temp.path().join("Programs"),
    };
    let written = integration.install().unwrap();
    assert_eq!(written, integration.files());
    assert_eq!(written, [temp.path().join("Programs").join("Copcat.lnk")]);
    assert!(std::fs::metadata(&written[0]).unwrap().len() > 0);
    // Rewriting is fine.
    assert_eq!(integration.install().unwrap(), written);
    assert_eq!(integration.remove().unwrap(), written);
    assert!(!written[0].exists());
}
