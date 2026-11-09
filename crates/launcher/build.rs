//! The launcher's icon (`icon.ico`, our own drawing) put in the exe, for
//! Explorer and the Desktop to show: written as a Windows `.res` file,
//! which MSVC's linker takes as it is (no resource compiler needed).

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=icon.ico");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !(windows && msvc) {
        return;
    }
    let ico = std::fs::read("icon.ico").expect("icon.ico");
    let res = icon_res(&ico).expect("icon.ico isn't an icon");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("icon.res");
    std::fs::write(&out, res).unwrap();
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

/// An `.ico` file's pictures as resources: each an `RT_ICON` (3), and
/// the `RT_GROUP_ICON` (14) listing them, named 1 (the exe's icon).
fn icon_res(ico: &[u8]) -> Option<Vec<u8>> {
    let u16_at = |at: usize| Some(u16::from_le_bytes(ico.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(ico.get(at..at + 4)?.try_into().ok()?));
    if u16_at(2)? != 1 {
        return None;
    }
    let count = u16_at(4)? as usize;
    let mut res = Vec::new();
    // The empty first entry every `.res` starts with.
    resource(&mut res, 0, 0, 0, &[]);
    let mut group = vec![0, 0, 1, 0];
    group.extend_from_slice(&(count as u16).to_le_bytes());
    for i in 0..count {
        let entry = 6 + i * 16;
        let size = u32_at(entry + 8)? as usize;
        let offset = u32_at(entry + 12)? as usize;
        let id = (i + 1) as u16;
        resource(&mut res, 3, id, 0x1010, ico.get(offset..offset + size)?);
        group.extend_from_slice(ico.get(entry..entry + 12)?);
        group.extend_from_slice(&id.to_le_bytes());
    }
    resource(&mut res, 14, 1, 0x1030, &group);
    Some(res)
}

/// One resource: its header (type and name by number, US English) and
/// its data, padded to four bytes.
fn resource(out: &mut Vec<u8>, kind: u16, name: u16, flags: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    for n in [kind, name] {
        out.extend_from_slice(&0xffffu16.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // data version
    out.extend_from_slice(&flags.to_le_bytes());
    let language: u16 = if kind == 0 { 0 } else { 0x0409 };
    out.extend_from_slice(&language.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // version
    out.extend_from_slice(&0u32.to_le_bytes()); // characteristics
    out.extend_from_slice(data);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}
