fn main() {
    // enable thunk-rs when target os is windows and arch is x86_64 or i686
    #[cfg(target_os = "windows")]
    if !std::env::var("TARGET")
        .unwrap_or_default()
        .contains("aarch64")
    {
        thunk::thunk();
    }

    #[cfg(target_os = "windows")]
    {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();

        println!("cargo:warning=BuildRS Path: {}", manifest_dir);
        println!("cargo:rustc-link-search=native={}", manifest_dir);

        if let Some(parent) = std::path::Path::new(&manifest_dir).parent() {
            println!("cargo:rustc-link-search=native={}", parent.display());
        }

        println!("cargo:rustc-link-lib=Packet");
    }

    tauri_build::build();
}
