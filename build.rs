use std::path::PathBuf;

fn main() {
    slint_build::compile("ui/app.slint").unwrap();
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=Cargo.toml");

    // give the .exe its icon and Windows "Details" (name, version) so Explorer, the
    // taskbar and file properties show it properly
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
        let mut numbers: Vec<u32> = version.split('.').map(|p| p.parse().unwrap_or(0)).collect();
        numbers.resize(4, 0);
        let (a, b, c, d) = (numbers[0], numbers[1], numbers[2], numbers[3]);
        let icon = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets").join("icon.ico");
        let rc = format!(
            r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {a},{b},{c},{d}
PRODUCTVERSION {a},{b},{c},{d}
FILEOS 0x4
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "ytmusic-lite"
      VALUE "ProductName", "ytmusic-lite"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
      VALUE "OriginalFilename", "ytmusic-lite.exe"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
            icon = icon.display().to_string().replace('\\', "\\\\"),
        );
        let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("app.rc");
        std::fs::write(&out, rc).unwrap();
        embed_resource::compile(&out, embed_resource::NONE).manifest_optional().unwrap();
    }
}
