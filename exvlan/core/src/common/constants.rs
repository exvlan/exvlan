macro_rules! define_global_var {
    ($name:ident, $type:ty, $init:expr) => {
        pub static $name: once_cell::sync::Lazy<std::sync::Mutex<$type>> =
            once_cell::sync::Lazy::new(|| std::sync::Mutex::new($init));
    };
}

#[macro_export]
macro_rules! use_global_var {
    ($name:ident) => {
        $crate::common::constants::$name.lock().unwrap().to_owned()
    };
}

#[macro_export]
macro_rules! set_global_var {
    ($name:ident, $val:expr) => {
        *$crate::common::constants::$name.lock().unwrap() = $val
    };
}

define_global_var!(MANUAL_CONNECTOR_RECONNECT_INTERVAL_MS, u64, 1000);

define_global_var!(OSPF_UPDATE_MY_GLOBAL_FOREIGN_NETWORK_INTERVAL_SEC, u64, 10);

define_global_var!(MAX_DIRECT_CONNS_PER_PEER_IN_FOREIGN_NETWORK, u32, 3);

define_global_var!(DIRECT_CONNECT_TO_PUBLIC_SERVER, bool, true);

// must make it true in future.
define_global_var!(HMAC_SECRET_DIGEST, bool, false);

pub const UDP_HOLE_PUNCH_CONNECTOR_SERVICE_ID: u32 = 2;

pub const WIN_SERVICE_WORK_DIR_REG_KEY: &str = "SOFTWARE\\EXVlan\\Service\\WorkDir";

pub const EXVLAN_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

// 在 release tag 上，git describe 输出 v<版本>，拼接得到 "<版本>-v<版本>"（冗余）。
// 此处检测该形态并还原为纯 "<版本>"；其他情况（如 "<版本>-<commit>"）原样保留。
const fn normalize_version(pkg: &'static str, full: &'static str) -> &'static str {
    if pkg.len() + 2 + pkg.len() == full.len() {
        let pkg_b = pkg.as_bytes();
        let full_b = full.as_bytes();
        let mut i = 0;
        while i < pkg.len() && full_b[i] == pkg_b[i] {
            i += 1;
        }
        if i == pkg.len() && full_b[pkg.len()] == b'-' && full_b[pkg.len() + 1] == b'v' {
            let mut j = 0;
            while j < pkg.len() && full_b[pkg.len() + 2 + j] == pkg_b[j] {
                j += 1;
            }
            if j == pkg.len() {
                return pkg;
            }
        }
    }
    full
}

pub const EXVLAN_VERSION: &str = normalize_version(
    EXVLAN_PKG_VERSION,
    git_version::git_version!(
        args = ["--abbrev=8", "--always", "--dirty=~"],
        prefix = concat!(env!("CARGO_PKG_VERSION"), "-"),
        suffix = "",
        fallback = env!("CARGO_PKG_VERSION")
    ),
);
