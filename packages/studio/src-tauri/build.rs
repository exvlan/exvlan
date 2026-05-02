use std::env;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    // enable thunk-rs when target os is windows and arch is x86_64 or i686
    if target_os == "windows" && (target_arch == "x86" || target_arch == "x86_64") {
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
