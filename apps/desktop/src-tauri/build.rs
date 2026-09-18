fn main() {
    tauri_build::build();
    manifest_for_windows_tests();
}

/// Gives the test binaries the application manifest the app binary gets from
/// `tauri-build`.
///
/// `tauri-build` embeds a manifest asking for Common Controls version 6 into
/// the app binary alone (`rustc-link-arg-bins`). A test binary that links the
/// Wry runtime — the IPC test does, through `AppHandle` in `AppCore` — imports
/// `TaskDialogIndirect` from `comctl32.dll`, and without that manifest Windows
/// binds it to Common Controls version 5, which has no such export: the
/// binary exits 0xc0000139 (`STATUS_ENTRYPOINT_NOT_FOUND`) before running a
/// line. MSVC's linker embeds the manifest given here; the GNU toolchain
/// would need a resource compiler and is not a target v1 builds for.
fn manifest_for_windows_tests() {
    let windows = std::env::var("CARGO_CFG_WINDOWS").is_ok();
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !(windows && msvc) {
        return;
    }
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("windows")
        .join("common-controls.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
