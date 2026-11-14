pub const MANUAL_CONNECTOR_RECONNECT_INTERVAL_MS: u64 = 1000;

pub const OSPF_UPDATE_MY_GLOBAL_FOREIGN_NETWORK_INTERVAL_SEC: u64 = 10;

pub const MAX_DIRECT_CONNS_PER_PEER_IN_FOREIGN_NETWORK: u32 = 3;

pub const DIRECT_CONNECT_TO_PUBLIC_SERVER: bool = true;

// must make it true in future.
pub const HMAC_SECRET_DIGEST: bool = false;

pub const WIN_SERVICE_WORK_DIR_REG_KEY: &str = "SOFTWARE\\Exvlan\\Service\\WorkDir";

/// 内核版本串：`<标准版本>[-<channel>.<n>][+<sha6>]`。
///
/// 由 `build/main.rs` 注入 `EXVLAN_BUILD_VERSION`：
/// - CI（tag）：`26.410.20713-pre.8+abc123`
/// - 本地构建：`26.410.20713+abc123`
///
/// 旧实现用 `git_version!` + `git describe`，在有 `v*` tag 的工作区会返回
/// tag 名再被前缀拼接，产出 `26.410.20711-v26.410.20711` 这类重复串；
/// `--dirty=~` 还会给每个脏构建追加 `~`。两者都已移除。
pub const EXVLAN_VERSION: &str = env!("EXVLAN_BUILD_VERSION");
