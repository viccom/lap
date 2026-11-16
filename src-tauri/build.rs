/**
 * Desktop shell build script: Tauri codegen plus the OS clipboard shims
 * (t_pasteboard). The four C image libraries moved to lap-core with the
 * modules that link them (phase 3).
 */
use std::env;

fn main() {
    build_clipboard_shims();

    // build tauri
    tauri_build::build();
}

/// Native clipboard shims for formats that must be published together.
fn build_clipboard_shims() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" {
        println!("cargo:rerun-if-changed=src/pasteboard.mm");
        cc::Build::new()
            .file("src/pasteboard.mm")
            .compile("lap_pasteboard");
        println!("cargo:rustc-link-lib=framework=AppKit");
    } else if target_os == "linux" {
        println!("cargo:rerun-if-changed=src/clipboard_linux.c");
        let gtk = pkg_config::Config::new()
            .probe("gtk+-3.0")
            .expect("gtk+-3.0 is required to build the Linux clipboard shim");
        let mut build = cc::Build::new();
        build.file("src/clipboard_linux.c");
        for include_path in gtk.include_paths {
            build.include(include_path);
        }
        build.compile("lap_clipboard");
    }
}
