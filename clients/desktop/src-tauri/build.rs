use std::process::Command;

fn main() {
    // Auto-detect FUSE support and enable the feature if available
    if has_fuse() {
        println!("cargo:rustc-cfg=has_fuse");
    }

    tauri_build::build()
}

fn has_fuse() -> bool {
    let os = std::env::consts::OS;
    match os {
        "macos" => has_macfuse(),
        "linux" => has_libfuse3(),
        _ => false,
    }
}

fn has_macfuse() -> bool {
    // Check for macFUSE installation paths
    std::path::Path::new("/usr/local/include/fuse/fuse.h").exists()
        || std::path::Path::new("/usr/local/include/osxfuse/fuse.h").exists()
        || std::path::Path::new("/opt/homebrew/include/fuse/fuse.h").exists()
        || std::path::Path::new("/Library/Frameworks/macFUSE.framework").exists()
        || pkg_config_has("fuse")
}

fn has_libfuse3() -> bool {
    pkg_config_has("fuse3") || pkg_config_has("fuse")
}

fn pkg_config_has(lib: &str) -> bool {
    Command::new("pkg-config")
        .args(["--exists", lib])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
