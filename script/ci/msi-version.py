#!/usr/bin/env python3
"""把标准版本推导成 Windows MSI 的 4 段 ProductVersion。

规则（5 通道 + nightly）：

    MSI 版本 = <YY>.<MMQ>.<Mmpp>.<通道位>

    YY      = 版本串首段（26）
    MMQ     = 月份(2 位) + 季度(1 位)，由 QMM 推导（410 → 10 月 Q4 → 104）
    Mmpp    = 版本串第三段**整段**（20713）
    通道位  = stage*10000 + seq
              nightly = 1xxxx（seq = 日期 MMDD）
              dev     = 2xxxx（历史 `-pre.*` 也归这里）
              beta    = 3xxxx
              rc      = 4xxxx
              stable  = 5xxxx + patch（patch = Mmpp 末两位）

为什么第 3 段要放完整 Mmpp（以前放 `Mmpp/100`）：
    以前 20700–20799 的第 3 段全是 207，只能靠第 4 段区分。夜间版每天发，
    「新 base 的 nightly」必须大于「旧 base 的 stable」，所以让 base 独占第 3 段；
    第 4 段只表达「同 base 内的通道顺序」：nightly < dev < beta < rc < stable，
    与客户端比较器的 stage 顺序完全一致。

兼容性：老包的 field3 = Mmpp/100（如 207），新包 field3 = 20713 → 新包更大，
MSI 会按升级处理；UpgradeCode 未变，不会变成并存。

用法：
    python3 script/ci/msi-version.py 26.410.20713
    → 26.104.20713.50013
    python3 script/ci/msi-version.py 26.410.20713-rc.1
    → 26.104.20713.40001
"""

from __future__ import annotations

import re
import sys

VERSION_RE = re.compile(r"^(\d+)\.(\d+)\.(\d+)(?:-([A-Za-z]+)\.(\d+))?(?:\+.*)?$")


# 通道 → (stage 位基址)。注意与客户端 `compare_stage` 一致：
# stable 最高（正式版是 rc 的后一步），nightly 最低。
STAGE_BASE = {
    "nightly": 10000,
    "pre": 20000,
    "dev": 20000,
    "beta": 30000,
    "rc": 40000,
}


def msi_version(version: str) -> str:
    m = VERSION_RE.match(version.strip())
    if not m:
        sys.exit(f"[msi] 无法解析版本: {version}")
    year, qmm, mmpp = int(m.group(1)), int(m.group(2)), int(m.group(3))
    stage_name = (m.group(4) or "").lower()
    seq = int(m.group(5) or 0)

    quarter, month = divmod(qmm, 100)
    if not (1 <= quarter <= 4 and 1 <= month <= 12):
        sys.exit(f"[msi] QMM 段不合法: {qmm}")

    mmq = int(f"{month:02d}{quarter}")
    patch = mmpp % 100
    if stage_name:
        fourth = STAGE_BASE.get(stage_name)
        if fourth is None:
            sys.exit(f"[msi] 未知通道: {stage_name}（支持 nightly/dev/pre/beta/rc）")
        fourth += seq
    else:
        fourth = 50000 + patch

    if not (
        0 <= year <= 255
        and 0 <= mmq <= 255
        and 0 <= mmpp <= 65535
        and 0 <= fourth <= 65535
    ):
        sys.exit(
            f"[msi] 推导结果超出 MSI 限制: {year}.{mmq}.{mmpp}.{fourth}"
        )
    return f"{year}.{mmq}.{mmpp}.{fourth}"


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("用法: msi-version.py <EXVLAN_BUILD_VERSION>")
    print(msi_version(sys.argv[1]))
