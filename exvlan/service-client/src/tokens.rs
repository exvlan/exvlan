//! 远程访问用的 token 存储（scoped、可过期、可吊销、只存 hash）。
//!
//! 位置：`<user>/gateway-tokens.json`（0600，属主 = 当前用户）。
//! 为什么由用户持有而不是服务：token 的信任域就是「这个用户能读的文件」，
//! 与服务 socket 的 `0600 owner` 同级；关键性质是**远端不能自行签发**。
//!
//! 明文只在 `create()` 返回一次；磁盘上只有 `sha256(secret)`。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 全部可用 scope（新增只会追加，不改变既有语义）。
pub const ALL_SCOPES: &[&str] = &[
    "kernel:read",
    "kernel:write",
    "service:read",
    "service:control",
    "service:admin",
    "gateway:admin",
    "net:probe",
];

/// 角色 = scope 预设。
pub fn scopes_for_role(role: &str) -> Option<Vec<String>> {
    let readonly = ["kernel:read", "service:read", "net:probe"];
    let s = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match role {
        "readonly" => Some(s(&readonly)),
        "config" => {
            let mut v = s(&readonly);
            v.push("kernel:write".into());
            Some(v)
        }
        "admin" => Some(ALL_SCOPES.iter().map(|s| s.to_string()).collect()),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredToken {
    pub id: String,
    pub name: String,
    pub scopes: Vec<String>,
    /// unix 秒
    pub created_at: i64,
    /// None = 不过期
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub revoked: bool,
    /// `sha256(secret)` 的小写 hex
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub tokens: Vec<StoredToken>,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, PartialEq, Eq)]
pub enum VerifyError {
    Unknown,
    Revoked,
    Expired,
}

/// 验证通过后的声明。
#[derive(Debug, Clone)]
pub struct Claims {
    pub id: String,
    pub name: String,
    pub scopes: Vec<String>,
}

impl Claims {
    pub fn allows(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

impl Store {
    /// token store 路径。
    ///
    /// `EXVLAN_TOKEN_STORE` 可覆盖：服务托管 gateway 时以 **root** 身份跑，
    /// `$HOME` 是 `/var/root`，token 若跟着 HOME 走就会和「服务状态」分家
    /// （换启动方式/换 HOME 后所有配对 token 集体失效）。服务因此显式指定
    /// `<data>/gateway-tokens.json`。
    pub fn path() -> PathBuf {
        if let Ok(custom) = std::env::var("EXVLAN_TOKEN_STORE") {
            let custom = custom.trim();
            if !custom.is_empty() {
                return PathBuf::from(custom);
            }
        }
        crate::paths::user_paths().data.join("gateway-tokens.json")
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        // 原子写：先写临时文件再 rename，避免并发读到半截 JSON（读侧解析失败会退化成
        // 空 store —— 表现为「所有 token 突然失效」）。
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).ok();
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// 签发新 token。返回 `(明文, 记录)`；明文只此一次。
    pub fn create(
        &mut self,
        name: &str,
        scopes: Vec<String>,
        ttl_secs: Option<i64>,
    ) -> (String, StoredToken) {
        let now = now_secs();
        let id = random_hex(8);
        let secret = random_hex(32);
        let token = format!("exv_{id}_{secret}");
        let record = StoredToken {
            id,
            name: name.to_string(),
            scopes,
            created_at: now,
            expires_at: ttl_secs.map(|ttl| now + ttl),
            revoked: false,
            hash: hash_secret(&secret),
            last_used_at: None,
        };
        self.tokens.push(record.clone());
        (token, record)
    }

    pub fn revoke(&mut self, id: &str) -> bool {
        let mut hit = false;
        for t in self.tokens.iter_mut() {
            let bare = t.id.trim_start_matches('t').trim_start_matches('_');
            if t.id == id || bare == id {
                t.revoked = true;
                hit = true;
            }
        }
        hit
    }

    /// 校验明文 token。
    pub fn verify(&mut self, token: &str) -> Result<Claims, VerifyError> {
        let rest = token.strip_prefix("exv_").ok_or(VerifyError::Unknown)?;
        let (id, secret) = rest.split_once('_').ok_or(VerifyError::Unknown)?;
        let hash = hash_secret(secret);
        let now = now_secs();
        let Some(record) = self
            .tokens
            .iter_mut()
            .find(|t| t.id == id && t.hash == hash)
        else {
            return Err(VerifyError::Unknown);
        };
        if record.revoked {
            return Err(VerifyError::Revoked);
        }
        if let Some(exp) = record.expires_at
            && now >= exp
        {
            return Err(VerifyError::Expired);
        }
        record.last_used_at = Some(now);
        Ok(Claims {
            id: record.id.clone(),
            name: record.name.clone(),
            scopes: record.scopes.clone(),
        })
    }
}

fn hash_secret(secret: &str) -> String {
    use sha2::{Digest as _, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn random_hex(bytes: usize) -> String {
    // 用 uuid v4（getrandom）凑够熵；每 16 字节一次。
    let mut out = String::new();
    while out.len() < bytes * 2 {
        out.push_str(&uuid::Uuid::new_v4().simple().to_string());
    }
    out.truncate(bytes * 2);
    out
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 解析 `30d` / `24h` / `90m` / `3600`（秒）。
pub fn parse_ttl(text: &str) -> Option<i64> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let (num, mult) = match t.chars().last()? {
        'd' => (&t[..t.len() - 1], 86_400),
        'h' => (&t[..t.len() - 1], 3_600),
        'm' => (&t[..t.len() - 1], 60),
        's' => (&t[..t.len() - 1], 1),
        _ => (t, 1),
    };
    num.trim().parse::<i64>().ok().map(|n| n * mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_verify() {
        let mut store = Store::default();
        let (token, rec) = store.create("test", scopes_for_role("config").unwrap(), None);
        assert!(token.starts_with("exv_"));
        assert_eq!(rec.name, "test");
        let claims = store.verify(&token).expect("verify");
        assert!(claims.allows("kernel:write"));
        assert!(!claims.allows("service:admin"));
    }

    #[test]
    fn wrong_secret_rejected() {
        let mut store = Store::default();
        let (token, rec) = store.create("t", vec!["kernel:read".into()], None);
        let bad = format!("exv_{}_deadbeef", rec.id);
        assert!(matches!(store.verify(&bad), Err(VerifyError::Unknown)));
        assert!(store.verify(&token).is_ok());
    }

    #[test]
    fn expired_rejected() {
        let mut store = Store::default();
        let (token, rec) = store.create("t", vec!["kernel:read".into()], Some(1));
        store.tokens[0].expires_at = Some(now_secs() - 1);
        assert!(matches!(store.verify(&token), Err(VerifyError::Expired)));
        assert_eq!(rec.id, store.tokens[0].id);
    }

    #[test]
    fn revoked_rejected() {
        let mut store = Store::default();
        let (token, rec) = store.create("t", vec!["kernel:read".into()], None);
        assert!(store.revoke(&rec.id));
        assert!(matches!(store.verify(&token), Err(VerifyError::Revoked)));
    }

    #[test]
    fn store_round_trip_and_hash_only() {
        let dir = std::env::temp_dir().join(format!("exvlan-tokens-{}", std::process::id()));
        let path = dir.join("gateway-tokens.json");
        let mut store = Store::default();
        let (token, _) = store.create("rt", vec!["kernel:read".into()], None);
        store.save(&path).expect("save");
        let text = std::fs::read_to_string(&path).unwrap();
        let secret = token.rsplit('_').next().unwrap();
        assert!(!text.contains(secret), "磁盘上不能出现明文 secret");
        let mut loaded = Store::load(&path);
        assert!(loaded.verify(&token).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ttl_parsing() {
        assert_eq!(parse_ttl("30d"), Some(30 * 86_400));
        assert_eq!(parse_ttl("24h"), Some(86_400));
        assert_eq!(parse_ttl("3600"), Some(3600));
        assert_eq!(parse_ttl(""), None);
    }
}
