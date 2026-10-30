//! Embeds the icon and version information into the Windows executable; does nothing elsewhere.
//! GPUI loads icon resource 1 as the window icon.

fn main() {
    println!("cargo:rerun-if-changed=resources/windows/app.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let env = |name: &str| std::env::var(name).unwrap();
    let icon = std::path::Path::new(&env("CARGO_MANIFEST_DIR")).join("resources/windows/app.ico");
    let icon = icon.to_str().unwrap().replace('\\', "\\\\");
    let (major, minor, patch) =
        (env("CARGO_PKG_VERSION_MAJOR"), env("CARGO_PKG_VERSION_MINOR"), env("CARGO_PKG_VERSION_PATCH"));
    let version = env("CARGO_PKG_VERSION");
    let rc = format!(
        r#"#include <winver.h>
1 ICON "{icon}"
VS_VERSION_INFO VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS VOS_NT_WINDOWS32
FILETYPE VFT_APP
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904B0"
        BEGIN
            VALUE "FileDescription", "Orca Profile Studio"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "profile-studio"
            VALUE "OriginalFilename", "Orca Profile Studio.exe"
            VALUE "ProductName", "Orca Profile Studio"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    );
    let path = std::path::Path::new(&env("OUT_DIR")).join("app.rc");
    std::fs::write(&path, rc).unwrap();
    embed_resource::compile(&path, embed_resource::NONE).manifest_optional().unwrap();
}
