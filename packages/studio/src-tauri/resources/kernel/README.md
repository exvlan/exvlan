# 内核资源目录

构建时由 CI 把 **core 构建产物**（`exvlan` / `exvlan.exe`）复制到本目录，
再由 `tauri.conf.json` 的 `bundle.resources` 打进安装包。

GUI 首次运行时会把这里的内核复制到系统位置（需要一次管理员授权）：

- macOS:   `/Library/Application Support/exvlan/core/exvlan`
- Windows: `C:\ProgramData\exvlan\core\exvlan.exe`
- Linux:   `/opt/exvlan/core/exvlan`

本目录**不要**手工放置二进制（会被 .gitignore 忽略）。
