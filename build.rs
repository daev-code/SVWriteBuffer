//! Embeds a Windows version resource in the DLL (Properties > Details in Explorer), with
//! the version taken from Cargo.toml.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let v = |k: &str| env::var(format!("CARGO_PKG_VERSION_{k}")).unwrap();
    let (major, minor, patch) = (v("MAJOR"), v("MINOR"), v("PATCH"));
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let description = env::var("CARGO_PKG_DESCRIPTION").unwrap();

    let rc = format!(
        r#"#include <winver.h>

VS_VERSION_INFO VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK VS_FFI_FILEFLAGSMASK
FILEFLAGS 0
FILEOS VOS_NT_WINDOWS32
FILETYPE VFT_DLL
FILESUBTYPE VFT2_UNKNOWN
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "{description}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "SVWriteBuffer"
      VALUE "LegalCopyright", "MIT License"
      VALUE "OriginalFilename", "SVWriteBuffer.dll"
      VALUE "ProductName", "SVWriteBuffer"
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

    let path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("version.rc");
    fs::write(&path, rc).unwrap();
    embed_resource::compile_for_cdylib(&path, embed_resource::NONE).manifest_optional().unwrap();
}
