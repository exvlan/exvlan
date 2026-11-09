#!/usr/bin/env python3
"""把标准版本推导成 Windows MSI 的 4 段 ProductVersion。

规则（见 docs/paths.md 与 README_CN.md）：

    MSI 版本 = <YY>.<MMQ>.<mmmm>.<第四段>

    YY      = 版本串首段（26）
    MMQ     = 月份(2 位) + 季度(1 位)，由 QMM 推导（410 → 10 月 Q4 → 104）
    mmmm    = 主版本 + 次版本(2 位)，由 Mmpp 推导（20712 → 207）
    第四段  = 预览：1000 + 预览序号（pre.9 → 1009）
              正式：60000 + patch（20712 → 60012）

这样任何新包都比旧包的 ProductVersion 大，MSI 会按升级处理，
不会因为“同版本”走修复式安装（pre.8 与 pre.9 曾经都是 26.104.207.12）。

用法：
    python3 script/ci/msi-version.py 26.410.20712-pre.9+abc123
    → 26.104.207.1009
"""

from __future__ import annotations

import re
import sys

VERSION_RE = re.compile(r"^(\d+)\.(\d+)\.(\d+)(?:-[A-Za-z]+\.(\d+))?(?:\+.*)?$")


def msi_version(version: str) -> str:
    m = VERSION_RE.match(version.strip())
    if not m:
        sys.exit(f"[msi] 无法解析版本: {version}")
    year, qmm, mmpp = int(m.group(1)), int(m.group(2)), int(m.group(3))
    preview = int(m.group(4) or 0)

    quarter, month = divmod(qmm, 100)
    if not (1 <= quarter <= 4 and 1 <= month <= 12):
        sys.exit(f"[msi] QMM 段不合法: {qmm}")
    major_minor, patch = divmod(mmpp, 100)
    if not (1 <= month <= 12):
        sys.exit(f"[msi] 月份不合法: {month}")

    mmq = int(f"{month:02d}{quarter}")
    fourth = 1000 + preview if preview else 60000 + patch
    if not (0 <= year <= 255 and 0 <= mmq <= 255 and 0 <= major_minor <= 65535 and fourth <= 65535):
        sys.exit(f"[msi] 推导结果超出 MSI 限制: {year}.{mmq}.{major_minor}.{fourth}")
    return f"{year}.{mmq}.{major_minor}.{fourth}"


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("用法: msi-version.py <EXVLAN_BUILD_VERSION>")
    print(msi_version(sys.argv[1]))
