use std::{env, fs, path::PathBuf};

const ICON_RESOURCE_ID: u16 = 101;

fn main() {
    println!("cargo:rerun-if-changed=resources/app.manifest");
    println!("cargo:rerun-if-changed=build.rs");

    if env::var("CARGO_CFG_WINDOWS").is_err() {
        return;
    }

    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo provides OUT_DIR"));
    let icon = output.join("poe-alarm-preview.ico");
    fs::write(&icon, make_icon()).expect("write deterministic application icon");

    let manifest =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("resources/app.manifest");
    let resource_script = output.join("poe-alarm-preview.rc");
    let rc = format!(
        r#"#include <windows.h>
{ICON_RESOURCE_ID} ICON "{icon}"
1 RT_MANIFEST "{manifest}"
1 VERSIONINFO
 FILEVERSION 0,1,0,0
 PRODUCTVERSION 0,1,0,0
 FILEFLAGSMASK 0x3fL
 FILEFLAGS 0x0L
 FILEOS VOS_NT_WINDOWS32
 FILETYPE VFT_APP
 FILESUBTYPE 0x0L
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "SouNd\0"
      VALUE "FileDescription", "POE Alarm - Rust Preview\0"
      VALUE "FileVersion", "0.1.0.0\0"
      VALUE "InternalName", "PoeAlarm\0"
      VALUE "LegalCopyright", "Copyright (C) 2026 SouNd\0"
      VALUE "OriginalFilename", "PoeAlarm.exe\0"
      VALUE "ProductName", "POE Alarm - Rust Preview\0"
      VALUE "ProductVersion", "0.1.0\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0409, 1200
  END
END
"#,
        icon = rc_path(&icon),
        manifest = rc_path(&manifest),
    );
    fs::write(&resource_script, rc).expect("write Windows resource script");

    embed_resource::compile(&resource_script, embed_resource::NONE)
        .manifest_required()
        .expect("compile required Windows manifest, icon, and version resources");
}

fn rc_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Produces a deterministic, original alert icon without importing game or third-party artwork.
/// Each entry is a 32-bit DIB containing a red diamond and a white exclamation mark.
fn make_icon() -> Vec<u8> {
    let sizes = [16_u32, 32, 48];
    let images: Vec<Vec<u8>> = sizes.into_iter().map(make_icon_dib).collect();
    let directory_bytes = 6 + images.len() * 16;
    let mut bytes =
        Vec::with_capacity(directory_bytes + images.iter().map(Vec::len).sum::<usize>());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&(images.len() as u16).to_le_bytes());

    let mut offset = directory_bytes as u32;
    for (&size, image) in sizes.iter().zip(&images) {
        bytes.push(size as u8);
        bytes.push(size as u8);
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&32_u16.to_le_bytes());
        bytes.extend_from_slice(&(image.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&offset.to_le_bytes());
        offset += image.len() as u32;
    }
    for image in images {
        bytes.extend_from_slice(&image);
    }
    bytes
}

fn make_icon_dib(size: u32) -> Vec<u8> {
    let xor_bytes = size * size * 4;
    let and_stride = size.div_ceil(32) * 4;
    let and_bytes = and_stride * size;
    let mut dib = Vec::with_capacity((40 + xor_bytes + and_bytes) as usize);
    dib.extend_from_slice(&40_u32.to_le_bytes());
    dib.extend_from_slice(&(size as i32).to_le_bytes());
    dib.extend_from_slice(&((size * 2) as i32).to_le_bytes());
    dib.extend_from_slice(&1_u16.to_le_bytes());
    dib.extend_from_slice(&32_u16.to_le_bytes());
    dib.extend_from_slice(&0_u32.to_le_bytes());
    dib.extend_from_slice(&xor_bytes.to_le_bytes());
    dib.extend_from_slice(&0_i32.to_le_bytes());
    dib.extend_from_slice(&0_i32.to_le_bytes());
    dib.extend_from_slice(&0_u32.to_le_bytes());
    dib.extend_from_slice(&0_u32.to_le_bytes());

    for stored_y in 0..size {
        let y = size - 1 - stored_y;
        for x in 0..size {
            let center = (size as i32 - 1) / 2;
            let radius = (size as i32 * 43) / 100;
            let in_diamond = (x as i32 - center).abs() + (y as i32 - center).abs() <= radius;
            let stroke = (size / 8).max(2);
            let mark_x = x >= (size - stroke) / 2 && x < (size + stroke) / 2;
            let mark_bar = mark_x && y >= size / 5 && y < (size * 63) / 100;
            let mark_dot = mark_x && y >= (size * 72) / 100 && y < (size * 84) / 100;
            let (red, green, blue, alpha) = if in_diamond && (mark_bar || mark_dot) {
                (255, 255, 255, 255)
            } else if in_diamond {
                (218, 45, 64, 255)
            } else {
                (0, 0, 0, 0)
            };
            dib.extend_from_slice(&[blue, green, red, alpha]);
        }
    }
    dib.resize((40 + xor_bytes + and_bytes) as usize, 0);
    dib
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_icon_has_stable_structure() {
        let icon = make_icon();
        assert_eq!(&icon[..6], &[0, 0, 1, 0, 3, 0]);
        assert_eq!(icon.len(), 15_086);
    }
}
