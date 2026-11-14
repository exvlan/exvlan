//! 当前进程的身份（Windows SID / 其他平台 uid）。
//!
//! 用途：命名管道的 DACL 要收窄到「安装这个服务的那个用户」，但服务以
//! LocalSystem 运行，事后无法反推是哪个用户点的安装 —— 所以由 GUI 在提权前
//! 取自己的 SID 交给安装器，安装器写进 `service.toml`（root/SYSTEM 持有），
//! 服务启动时据此构造 ACL。
//!
//! 放在 control 而不是 GUI：这样 CLI / gateway 也能用，并且这个 crate
//! 只依赖 windows-sys（纯 Rust），可以交叉编译验证，不需要 MSVC 工具链。

/// 当前用户 uid（unix）；Windows 返回 0（与既有行为一致）。
pub fn current_uid() -> u32 {
    #[cfg(unix)]
    unsafe {
        libc::getuid() as u32
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// 当前用户的 Windows SID（形如 `S-1-5-21-...`）。
///
/// 非 Windows 或读取失败返回 `None` —— 调用方必须把「拿不到」当作正常情况处理
/// （管道 ACL 退回 `IU`），不能因此拒绝安装。
#[cfg(target_os = "windows")]
pub fn current_user_sid() -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        // 先问长度，再按长度取：TOKEN_USER 的大小随 SID 长度变化。
        let mut needed = 0u32;
        let _ = GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
        if needed == 0 {
            let _ = CloseHandle(token);
            return None;
        }
        let mut buf = vec![0u8; needed as usize];
        let ok = GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr() as *mut std::ffi::c_void,
            needed,
            &mut needed,
        );
        let _ = CloseHandle(token);
        if ok == 0 {
            return None;
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        sid_to_string(user.User.Sid as *const u8)
    }
}

#[cfg(not(target_os = "windows"))]
pub fn current_user_sid() -> Option<String> {
    None
}

/// 合法的 Windows SID（形如 `S-1-5-21-...`）。非法返回 `None`。
///
/// 安装器与迁移工具共用这一条判据：`service.toml` 的 `owner_sid` 只该来自
/// 安装者本人。空串、`--tools-dir`（历史 bug：macOS 把命令行开关当 SID 存了）
/// 或任何非 `S-1-` 开头的残渣都视为非法，写入侧归空、迁移侧抹掉。
pub fn valid_owner_sid(raw: &str) -> Option<&str> {
    let value = raw.trim();
    // 长度上限与管道 SDDL 的校验保持一致（SID 字符串 < 200 字节）。
    if !value.starts_with("S-1-") || value.len() > 184 {
        return None;
    }
    let numeric = value[4..]
        .split('-')
        .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    numeric.then_some(value)
}

/// 手工把 SID 结构转成字符串（不引入 `ConvertSidToStringSidW`，少一个 API 面）。
///
/// ```c
/// typedef struct _SID {
///   BYTE Revision; BYTE SubAuthorityCount;
///   SID_IDENTIFIER_AUTHORITY IdentifierAuthority;  // 6 字节，大端
///   DWORD SubAuthority[];
/// } SID;
/// ```
#[cfg(target_os = "windows")]
unsafe fn sid_to_string(sid: *const u8) -> Option<String> {
    if sid.is_null() {
        return None;
    }
    let revision = unsafe { *sid };
    let count = unsafe { *sid.add(1) } as usize;
    // 真实 SID 的 revision 恒为 1；子权限最多 15 个。
    if revision != 1 || count == 0 || count > 15 {
        return None;
    }
    let mut authority: u64 = 0;
    for i in 0..6 {
        authority = (authority << 8) | unsafe { *sid.add(2 + i) } as u64;
    }
    let mut out = format!("S-{revision}-{authority}");
    for i in 0..count {
        let mut raw = [0u8; 4];
        unsafe {
            std::ptr::copy_nonoverlapping(sid.add(8 + i * 4), raw.as_mut_ptr(), 4);
        }
        out.push_str(&format!("-{}", u32::from_le_bytes(raw)));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn uid_matches_getuid_on_unix() {
        // root(0) 也是合法值：只断言「读到的就是 getuid」，不假设非 0。
        #[cfg(unix)]
        unsafe {
            assert_eq!(super::current_uid(), libc::getuid() as u32);
        }
    }

    #[test]
    fn sid_is_optional_off_windows() {
        // 非 Windows 上必须安静地返回 None（安装流程不允许因此失败）。
        #[cfg(not(target_os = "windows"))]
        assert!(super::current_user_sid().is_none());
    }

    #[test]
    fn valid_owner_sid_rejects_installer_flags() {
        // dev.1 的脏值：macOS 把 `--tools-dir` 当 SID 写进了 service.toml。
        assert_eq!(super::valid_owner_sid("--tools-dir"), None);
        assert_eq!(super::valid_owner_sid(""), None);
        assert_eq!(super::valid_owner_sid("   "), None);
        assert_eq!(super::valid_owner_sid("root"), None);
        assert_eq!(super::valid_owner_sid("S-1-"), None);
        assert_eq!(super::valid_owner_sid("S-1-5-21-abc"), None);
        // 合法 SID：原样返回（去掉首尾空白）。
        assert_eq!(
            super::valid_owner_sid("  S-1-5-21-1004336348-1177238915-682003330-1001 "),
            Some("S-1-5-21-1004336348-1177238915-682003330-1001")
        );
    }
}
