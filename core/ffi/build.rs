//! Build script for generating C headers using cbindgen.

use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let package_name = env::var("CARGO_PKG_NAME").unwrap();

    let output_file = target_dir()
        .join("include")
        .join(format!("{}.h", package_name.replace('-', "_")));

    // Create the include directory if it doesn't exist
    if let Some(parent) = output_file.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // Generate C header
    let config = cbindgen::Config::from_file("cbindgen.toml").unwrap_or_default();

    match cbindgen::Builder::new()
        .with_crate(crate_dir)
        .with_config(config)
        .generate()
    {
        Ok(bindings) => {
            bindings.write_to_file(&output_file);
            println!(
                "cargo:warning=Generated C header at: {}",
                output_file.display()
            );
        }
        Err(e) => {
            println!("cargo:warning=Unable to generate C bindings: {}", e);
        }
    }

    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/types.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
}

/// Get the target directory
fn target_dir() -> PathBuf {
    if let Ok(target) = env::var("CARGO_TARGET_DIR") {
        PathBuf::from(target)
    } else {
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("target")
    }
}
