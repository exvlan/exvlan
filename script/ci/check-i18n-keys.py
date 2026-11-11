#!/usr/bin/env python3
"""校验前端里字面量 i18n key 都存在于 `cn.yaml`。

背景（pre.14 事故）：`settings.privileged_service` 被误放进 `web.settings`
（层级错了一层），运行时直接把 key 原样渲染成 `settings.privileged_service`,
而 yaml 本身是合法的、`vue-tsc` 也不会报错。这类问题只能在界面上肉眼发现。

规则：
  · 只校验**字面量** `t("a.b.c")` / `t('a.b.c')`；
    模板字符串（`t(`mode.${x}`)`）与变量参数跳过；
  · 以 `cn.yaml` 为基准（英文缺键同样会让界面回退到 key，所以也校验 en.yaml）。

用法：python3 script/ci/check-i18n-keys.py
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

import yaml


class _StrictLoader(yaml.SafeLoader):
    """同一层出现重复 key 直接报错。

    PyYAML 默认静默取后者，而构建链里的 js-yaml 会直接失败 ——
    pre.14/15 就这么让 CI 红了一次（`mode.service` 与 `mode.privileged_service`
    撞名）。这里改成严格模式，本地就能拦住。
    """


def _no_duplicates(loader, node, deep=False):
    mapping = {}
    for key_node, value_node in node.value:
        key = loader.construct_object(key_node, deep=deep)
        if key in mapping:
            raise yaml.constructor.ConstructorError(
                None, None, f"重复的 key: {key!r}", key_node.start_mark
            )
        mapping[key] = loader.construct_object(value_node, deep=deep)
    return mapping


_StrictLoader.add_constructor(
    yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, _no_duplicates
)

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "packages" / "studio" / "src"
LOCALES = {
    "cn": ROOT / "packages" / "std" / "src" / "locales" / "cn.yaml",
    "en": ROOT / "packages" / "std" / "src" / "locales" / "en.yaml",
}

# t("a.b") / t('a.b')  —— 只取纯字面量
KEY_RE = re.compile(r"""\bt\(\s*["']([A-Za-z0-9_.-]+)["']""")


def flatten(node, prefix: str = "") -> set[str]:
    """把嵌套 dict 展开成点分 key 集合（只收叶子）。"""
    out: set[str] = set()
    if isinstance(node, dict):
        for key, value in node.items():
            path = f"{prefix}.{key}" if prefix else str(key)
            if isinstance(value, dict):
                out |= flatten(value, path)
            else:
                out.add(path)
    return out


def main() -> int:
    trees = {}
    for lang, path in LOCALES.items():
        try:
            doc = yaml.load(path.read_text(encoding="utf-8"), Loader=_StrictLoader)
        except yaml.constructor.ConstructorError as err:
            print(f"::error::{path.name} 有重复 key：{err}")
            return 1
        trees[lang] = flatten(doc)

    used: dict[str, set[str]] = {}
    for path in SRC.rglob("*"):
        if path.suffix not in {".vue", ".ts"} or ".d.ts" in path.name:
            continue
        text = path.read_text(encoding="utf-8", errors="ignore")
        for match in KEY_RE.finditer(text):
            used.setdefault(match.group(1), set()).add(
                str(path.relative_to(ROOT))
            )

    failed = False
    for lang, tree in trees.items():
        missing = sorted(key for key in used if key not in tree)
        if missing:
            failed = True
            print(f"::error::以下 i18n key 在 {LOCALES[lang].name} 里不存在：")
            for key in missing:
                where = ", ".join(sorted(used[key])[:2])
                print(f"  {key}    ({where})")
        else:
            print(f"[i18n:{lang}] {len(used)} 个字面量 key 全部存在")

    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
