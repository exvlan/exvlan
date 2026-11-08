use cfg_aliases::cfg_aliases;
use std::env;

#[cfg(target_os = "windows")]
struct WindowsBuild {}

#[cfg(target_os = "windows")]
impl WindowsBuild {
    pub fn check_for_win() {
        // add third_party dir to link search path
        let target = std::env::var("TARGET").unwrap_or_default();
        let variant = if target.contains("x86_64") {
            "x86_64"
        } else if target.contains("i686") {
            "i686"
        } else if target.contains("aarch64") {
            "arm64"
        } else {
            return;
        };

        let mut path = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        path.push("third_party");
        path.push(variant);
        println!("cargo:rustc-link-search=native={}", path.to_str().unwrap());
    }
}

fn workdir() -> Option<String> {
    if let Ok(cargo_manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        return Some(cargo_manifest_dir);
    }

    let dest = std::env::var("OUT_DIR");
    if dest.is_err() {
        return None;
    }
    let dest = dest.unwrap();

    let seperator = regex::Regex::new(r"(/target/(.+?)/build/)|(\\target\\(.+?)\\build\\)")
        .expect("Invalid regex");
    let parts = seperator.split(dest.as_str()).collect::<Vec<_>>();

    if parts.len() >= 2 {
        return Some(parts[0].to_string());
    }

    None
}

fn check_locale() {
    let workdir = workdir().unwrap_or("./".to_string());

    let locale_path = format!("{workdir}/**/locales/**/*");
    if let Ok(globs) = globwalk::glob(locale_path) {
        for entry in globs {
            if let Err(e) = entry {
                println!("cargo:i18n-error={e}");
                continue;
            }

            let entry = entry.unwrap().into_path();
            println!("cargo:rerun-if-changed={}", entry.display());
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    cfg_aliases! {
        mobile: {
            any(
                target_os = "android",
                target_os = "ios",
                all(target_os = "macos", feature = "macos-ne"),
                target_env = "ohos"
            )
        }
    }

    println!(
        "cargo:rustc-env=EXVLAN_BUILD_VERSION={}",
        build_version()
    );

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    // enable thunk-rs when target os is windows and arch is x86_64 or i686
    if target_os == "windows" && (target_arch == "x86" || target_arch == "x86_64") {
        thunk::thunk();
    }

    #[cfg(target_os = "windows")]
    WindowsBuild::check_for_win();

    check_locale();
    Ok(())
}

/// 构建版本串：`<标准版本>[-<channel>.<n>][+<sha6>]`。
///
/// CI 通过 `EXVLAN_BUILD_VERSION` 注入（tag `v26.410.20712-pre.8` →
/// `26.410.20712-pre.8+abc123`）；本地构建回落到
/// `<CARGO_PKG_VERSION>+<git short sha 6>`。
///
/// 刻意不使用 `git describe`：仓库里存在 `v*` tag 时它返回 tag 名，
/// 再被前缀拼接会得到 `26.410.20711-v26.410.20711` 这类重复串。
fn build_version() -> String {
    println!("cargo:rerun-if-env-changed=EXVLAN_BUILD_VERSION");
    if let Ok(injected) = env::var("EXVLAN_BUILD_VERSION") {
        let injected = injected.trim();
        if !injected.is_empty() {
            return injected.to_string();
        }
    }

    let base = env::var("CARGO_PKG_VERSION").unwrap_or_default();
    match git_short_sha() {
        Some(sha) => format!("{base}+{sha}"),
        None => base,
    }
}

/// 当前提交的 6 位短 sha（非 git 工作区/无 git 时返回 `None`）。
fn git_short_sha() -> Option<String> {
    let manifest = env::var("CARGO_MANIFEST_DIR").ok()?;
    let out = std::process::Command::new("git")
        .args(["-C", &manifest, "rev-parse", "--short=6", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}
