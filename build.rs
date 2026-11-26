//! Gives the Windows programs Copcat's icon (`assets/icon.ico`), which
//! Explorer, the taskbar and the Start Menu shortcut show.
//!
//! The icon goes in as a compiled resource (`.res`) written here, which the
//! MSVC linker takes as an input like an object file. That needs no resource
//! compiler (`rc.exe`) and no build dependencies.

use std::path::PathBuf;

const ICON: &str = "assets/icon.ico";

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed={ICON}");

    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !windows_msvc {
        return;
    }
    let ico = std::fs::read(ICON).expect("read assets/icon.ico");
    let res = icon_resource(&ico);
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("icon.res");
    std::fs::write(&out, res).expect("write icon.res");
    println!("cargo::rustc-link-arg-bins={}", out.display());
}

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
/// English (United States), what `rc.exe` uses by default.
const LANGUAGE: u16 = 0x0409;
/// MOVEABLE | DISCARDABLE, plus PURE for the group, as `rc.exe` writes them.
const ICON_FLAGS: u16 = 0x1010;
const GROUP_FLAGS: u16 = 0x1030;

/// A `.res` file holding the `.ico` file's images as an icon group, with id
/// 1. The lowest-numbered group is the one Windows shows for the program.
fn icon_resource(ico: &[u8]) -> Vec<u8> {
    let u16_at = |at: usize| u16::from_le_bytes([ico[at], ico[at + 1]]);
    let u32_at = |at: usize| u32::from_le_bytes(ico[at..at + 4].try_into().unwrap());
    assert_eq!((u16_at(0), u16_at(2)), (0, 1), "{ICON} isn't an icon file");
    let count = u16_at(4);

    let mut res = Vec::new();
    // A .res file starts with an empty resource.
    resource(&mut res, 0, 0, 0, &[]);

    let mut group = Vec::new();
    group.extend_from_slice(&0u16.to_le_bytes());
    group.extend_from_slice(&1u16.to_le_bytes());
    group.extend_from_slice(&count.to_le_bytes());
    for index in 0..count {
        let entry = 6 + 16 * usize::from(index);
        let size = u32_at(entry + 8) as usize;
        let offset = u32_at(entry + 12) as usize;
        let id = index + 1;
        resource(
            &mut res,
            RT_ICON,
            id,
            ICON_FLAGS,
            &ico[offset..offset + size],
        );
        // The directory entry as in the .ico, with the image's resource id
        // in place of its file offset.
        group.extend_from_slice(&ico[entry..entry + 12]);
        group.extend_from_slice(&id.to_le_bytes());
    }
    resource(&mut res, RT_GROUP_ICON, 1, GROUP_FLAGS, &group);
    res
}

/// Appends one resource, with a numeric type and name, padded to 4 bytes.
fn resource(res: &mut Vec<u8>, kind: u16, name: u16, flags: u16, data: &[u8]) {
    const HEADER_SIZE: u32 = 32;
    res.extend_from_slice(&(data.len() as u32).to_le_bytes());
    res.extend_from_slice(&HEADER_SIZE.to_le_bytes());
    for ordinal in [kind, name] {
        res.extend_from_slice(&0xffffu16.to_le_bytes());
        res.extend_from_slice(&ordinal.to_le_bytes());
    }
    res.extend_from_slice(&0u32.to_le_bytes()); // data version
    res.extend_from_slice(&flags.to_le_bytes());
    let language = if kind == 0 { 0 } else { LANGUAGE };
    res.extend_from_slice(&language.to_le_bytes());
    res.extend_from_slice(&0u32.to_le_bytes()); // version
    res.extend_from_slice(&0u32.to_le_bytes()); // characteristics
    res.extend_from_slice(data);
    while !res.len().is_multiple_of(4) {
        res.push(0);
    }
}
