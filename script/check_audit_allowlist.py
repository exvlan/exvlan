#!/usr/bin/env python3
"""校验 cargo-audit 允许清单的元数据（.cargo/audit-allowlist.toml）。

规则：
1. 每项必须有 id / review_by / reason；
2. review_by 不得过期（过期即失败，强制复核）；
3. 本文件的 id 集合必须与 .cargo/audit.toml 的 advisories.ignore 完全一致。
"""
from __future__ import annotations

import datetime
import pathlib
import sys
import tomllib

ALLOW_PATH = pathlib.Path(".cargo/audit-allowlist.toml")
AUDIT_PATH = pathlib.Path(".cargo/audit.toml")


def main() -> int:
    entries = tomllib.loads(ALLOW_PATH.read_text(encoding="utf-8")).get("allow", [])
    ignore = (
        tomllib.loads(AUDIT_PATH.read_text(encoding="utf-8"))
        .get("advisories", {})
        .get("ignore", [])
    )

    today = datetime.date.today()
    errors: list[str] = []
    ids: list[str] = []

    for entry in entries:
        advisory_id = entry.get("id")
        review_by = entry.get("review_by")
        reason = entry.get("reason")
        if not advisory_id or not review_by or not reason:
            errors.append(f"条目不完整（需 id/review_by/reason）: {entry}")
            continue

        ids.append(advisory_id)
        try:
            deadline = datetime.date.fromisoformat(str(review_by))
        except ValueError:
            errors.append(f"{advisory_id}: review_by 不是合法日期: {review_by!r}")
            continue

        remaining = (deadline - today).days
        if remaining < 0:
            errors.append(
                f"{advisory_id}: 已过期（review_by={review_by}，逾期 {-remaining} 天）"
                "——请复核：修复依赖，或延期并在 reason 中补充说明"
            )
        else:
            print(f"OK   {advisory_id}  review_by={review_by}（剩余 {remaining} 天）")

    if sorted(ids) != sorted(ignore):
        errors.append(
            "allowlist 与 .cargo/audit.toml 的 ignore 不一致：\n"
            f"  allowlist={sorted(ids)}\n  ignore={sorted(ignore)}"
        )

    if errors:
        print("\n".join(f"ERROR: {e}" for e in errors), file=sys.stderr)
        return 1

    print(f"allowlist 校验通过（{len(ids)} 项）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
