/**
 * 版本比较：替代旧脚本里 `sort -V` 的用法（`version_ge` 与通道列表排序）。
 *
 * 只覆盖镜像实际会遇到的字符串形状：`v26.410.20711`、`26.410.20711-rc.1`、
 * `26.410.20713-dev.2`、`26.410.20714-nightly.1115`。规则与 GNU `sort -V` 一致到
 * 能区分这些形状：数字段按数值、字母段按 ASCII、`~` 最小、前缀更短者更小。
 */

const TILDE = 0x7e;

function isDigit(code: number): boolean {
  return code >= 0x30 && code <= 0x39;
}

function isAlpha(code: number): boolean {
  return (code >= 0x41 && code <= 0x5a) || (code >= 0x61 && code <= 0x7a);
}

/** GNU filevercmp 的字符权重：`~` < 结束 < 字母 < 其它。 */
function charOrder(code: number | undefined): number {
  if (code === undefined) return 0;
  if (code === TILDE) return -1;
  if (isAlpha(code)) return code;
  return code + 256;
}

function readDigits(text: string, start: number): { end: number; value: string } {
  let end = start;
  while (end < text.length && isDigit(text.charCodeAt(end))) end += 1;
  return { end, value: text.slice(start, end) };
}

function compareDigitRuns(a: string, b: string): number {
  const ta = a.replace(/^0+(?=\d)/, '');
  const tb = b.replace(/^0+(?=\d)/, '');
  if (ta.length !== tb.length) return ta.length < tb.length ? -1 : 1;
  if (ta === tb) return 0;
  return ta < tb ? -1 : 1;
}

/** <0 / 0 / >0，语义同 `strcmp`（a 小返回负数）。 */
export function versionCompare(a: string, b: string): number {
  let i = 0;
  let j = 0;
  while (i < a.length || j < b.length) {
    const ca = i < a.length ? a.charCodeAt(i) : undefined;
    const cb = j < b.length ? b.charCodeAt(j) : undefined;

    if (ca !== undefined && cb !== undefined && isDigit(ca) && isDigit(cb)) {
      const ra = readDigits(a, i);
      const rb = readDigits(b, j);
      const cmp = compareDigitRuns(ra.value, rb.value);
      if (cmp !== 0) return cmp;
      i = ra.end;
      j = rb.end;
      continue;
    }

    const oa = charOrder(ca);
    const ob = charOrder(cb);
    if (oa !== ob) return oa < ob ? -1 : 1;
    i += 1;
    j += 1;
  }
  return 0;
}

/** `$1 >= $2`（旧脚本用 `sort -V | tail -n 1` 判定）。 */
export function versionGe(a: string, b: string): boolean {
  return versionCompare(a, b) >= 0;
}
