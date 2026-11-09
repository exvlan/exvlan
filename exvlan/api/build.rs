use std::env;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    // enable thunk-rs when target os is windows and arch is x86_64 or i686
    //
    // 同 exvlan/exvlan/build/main.rs：CI 预取不到 VC-LTL / YY-Thunks 时
    // 用 EXVLAN_SKIP_THUNK 跳过（只是不带 Win7 垫片，不影响 Windows 10/11）。
    if target_os == "windows" && (target_arch == "x86" || target_arch == "x86_64") {
        if env::var("EXVLAN_SKIP_THUNK").is_ok() {
            println!("cargo:warning=thunk-rs skipped (EXVLAN_SKIP_THUNK set)");
        } else {
            thunk::thunk();
        }
    }
}
