//! backend 探测：`core`（内核直跑）还是 `service`（特权服务托管）。
//!
//! 判据只有「服务装没装」，不做任何策略判断（policy 由服务持有）。

use crate::paths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    /// 裸核：内核由 systemd / GUI 子进程监督，没有特权服务。
    Core,
    /// 特权服务托管内核（macOS launchd / Windows SCM）。
    Service,
}

impl Backend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Backend::Core => "core",
            Backend::Service => "service",
        }
    }

    pub fn has_service(&self) -> bool {
        matches!(self, Backend::Service)
    }
}

/// 探测当前 backend。
pub fn detect() -> Backend {
    let p = paths::paths();
    if !p.service_bin.is_file() {
        return Backend::Core;
    }
    #[cfg(target_os = "macos")]
    {
        // 二进制存在但 plist 没了 → 服务实际不可用，按 core 处理（避免误导 doctor）。
        if !paths::service_plist().is_file() {
            return Backend::Core;
        }
    }
    Backend::Service
}

/// supervisor 名称（诊断/`/api/v1/system` 用）。
pub fn supervisor() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "launchd"
    }
    #[cfg(target_os = "windows")]
    {
        "scm"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "systemd"
    }
}
