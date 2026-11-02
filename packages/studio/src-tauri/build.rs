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

    // Android：内核以预编译库 libexvlan_ffi.so 提供，必须显式链接，
    // 否则 libapp_lib.so 只在符号表里留下未定义项（DT_NEEDED 缺失）→ 启动即闪退。
    if target_os == "android" {
        let abi = match target_arch.as_str() {
            "aarch64" => "arm64-v8a",
            "arm" => "armeabi-v7a",
            "x86" => "x86",
            "x86_64" => "x86_64",
            other => panic!("exvlan: 不支持的 Android 架构 {other}"),
        };
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
        let jni_libs = std::path::Path::new(&manifest_dir)
            .join("gen/android/app/src/main/jniLibs")
            .join(abi);
        if !jni_libs.join("libexvlan_ffi.so").is_file() {
            panic!(
                "exvlan: 缺少内核预编译库 {}（ci 会从 mobile-kernel-{abi} 工件注入；本地请先放置）",
                jni_libs.join("libexvlan_ffi.so").display()
            );
        }
        println!("cargo:rustc-link-search=native={}", jni_libs.display());
        println!("cargo:rustc-link-lib=dylib=exvlan_ffi");
        println!(
            "cargo:rerun-if-changed={}",
            jni_libs.join("libexvlan_ffi.so").display()
        );
    }

    tauri_build::build();
}
