# 应用更新（桌面官方 updater + 自建回退 + Android 安装）

Studio 的应用更新有**两条路径**，桌面上官方 updater 优先，失败自动回退自建流程；
移动端只有自建流程（官方 updater 不支持应用商店分发之外的自更新）。

## 路径 1：官方 `tauri-plugin-updater`（桌面主路径）

```
tauri.conf.json
  bundle.createUpdaterArtifacts = true      → 构建时产出 .sig（minisign）
  plugins.updater.pubkey                    → 公钥（校验）
  plugins.updater.endpoints                 → https://dl.exvlan.era-x.cn/upgrade/latest.json
```

`latest.json`（由 `release.yml` 生成，镜像到 `/upgrade/latest.json`）：

```jsonc
{
  "version": "26.410.20706",     // 不带 v 前缀（semver）
  "notes": "…",
  "pub_date": "2026-11-02T14:57:55Z",
  "platforms": {
    "darwin-x86_64":  { "signature": "<.app.tar.gz.sig 内容>", "url": "https://…/X.app.tar.gz" },
    "windows-x86_64": { "signature": "<-setup.exe.sig 内容>",  "url": "https://…/X-setup.exe" }
  }
}
```

签名密钥：

- 私钥 → 本机 `.secrets`（gitignored）+ GitHub Secret `TAURI_SIGNING_PRIVATE_KEY`
- 密码为空（生成时未设密码），构建时传 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""`
- 公钥 → `packages/studio/src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey`

> 私钥丢失 = 已发布客户端永久无法验证新版本，务必备份 `.secrets`。

## 路径 2：自建 manifest（回退 / 移动端）

```
upgrade::app::check_update()        → <base>manifest.json
upgrade::app::download_installer()  → 下载 + sha256 校验
upgrade::app::install_installer()   → DMG/MSI 交给系统；Android 走 FileProvider
```

`upgrade-manifest.json`（镜像为 `/upgrade/manifest.json`）：

```jsonc
{
  "schema": 1,
  "latest": "26.410.20706",
  "versions": [{
    "version": "26.410.20706",
    "assets": {
      "macos-x86_64":      { "file": "EXVlan Studio_…_x64.dmg",  "sha256": "…" },
      "windows-x86_64":    { "file": "EXVlan Studio_…_x64_en-US.msi", "sha256": "…" },
      "android-arm64-v8a": { "file": "app-arm64-release.apk", "sha256": "…" }
    }
  }]
}
```

平台键与 `upgrade::platform_key()` 一致（`macos-<arch>` / `windows-<arch>` / `android-<abi>`）。

## 镜像目录

`https://dl.exvlan.era-x.cn/upgrade/`（Caddy，同 `/core/` 一个站点）：

| 文件 | 说明 | 缓存 |
|---|---|---|
| `latest.json` | 官方 updater 清单 | `no-cache` |
| `manifest.json` | 自建清单 | `no-cache` |
| 安装包（dmg/msi/exe/apk/app.tar.gz）与 `.sig` | 带版本/内容不可变 | 1 年 immutable |

同步：`script/kernel-mirror/exvlan-kernel-sync.sh`（systemd timer，15 min）。
它先落安装包与签名，最后落清单——客户端不会看到指向缺失文件的清单。
环境变量 `UPGRADE_DEST` 可改目录（默认 `/srv/exvlan/upgrade`）。

## Android 自动安装

`app_install_update`（android）→ `tauri-plugin-vpnservice` 的 `install_apk`：

1. APK 下载到**应用 cache 目录**（`app_cache_dir()/upgrade`），
   否则 FileProvider 无法共享给系统安装器；
2. Kotlin 侧用 `FileProvider.getUriForFile(<pkg>.fileprovider)` 生成 `content://` URI；
3. `Intent.ACTION_VIEW` + `application/vnd.android.package-archive` 启动系统安装器。

前提：AndroidManifest 声明 `REQUEST_INSTALL_PACKAGES`（插件清单已加），
`file_paths.xml` 覆盖 `cache-path`。首次安装系统会要求用户授予「安装未知应用」权限。

## 文件名约定（踩过的坑）

- **GitHub 会把资产名里的空格换成点**：`EXVlan Studio_x64.dmg` → `EXVlan.Studio_x64.dmg`。
  清单必须引用上传后的名字，因此生成脚本会先把空格规范化为点。
- **macOS 的 `.app.tar.gz` 默认不含版本**（`EXVlan Studio.app.tar.gz`）。镜像对 `.gz`
  用 `immutable` 缓存，同名覆盖会让客户端拿到旧包 → 脚本重命名为 `…_<version>_<arch>.app.tar.gz`。
- **Windows 安装包里的版本号是 Tauri 自己的一套**（如 `26.104.207-5`），与 `tauri.conf.json`
  的 `26.410.20706` 不同。这是既有行为（2026-10 起就这么命名），只影响文件名与「应用和功能」
  里的显示，不影响 updater 版本比较（比较用的是运行时版本）。
- `release.yml` 的 `Package release assets` 会跳过 `mobile-kernel-*`，避免把裸 `.so`
  混进 Release 资产（内核已经打进 APK）。

## 发版流程要点

1. `release.yml` → `script/gen_upgrade_manifests.py` 生成两个清单（任一为空则失败）；
2. 缺少 `.sig` 时脚本直接失败，避免发出无法验证的更新；
3. 镜像脚本把安装包/签名/清单同步到 `/upgrade/`；
4. `tauri.conf.json` 的 `version` 必须等于 tag（`v` 前缀去掉后），否则 updater 判定异常。
