/**
 * Python 兼容的 JSON 序列化（`json.dumps(obj, indent=2, ensure_ascii=False)`）。
 *
 * 为什么不用 `JSON.stringify(obj, null, 2)`：镜像目录里的清单/状态文件由旧 Python
 * 脚本产出，客户端与看门狗都按字节比对 sha256、`diff` 也会被用来做等价性验收。
 * 两边的差异点有：Python 的 `": "` 分隔符、空容器写成 `[]`/`{}`、控制字符用
 * `\u00XX`（小写 hex）、非 ASCII 原样输出（ensure_ascii=False）。
 */

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

function escapeString(value: string): string {
  let out = '';
  for (const ch of value) {
    switch (ch) {
      case '"':
        out += '\\"';
        break;
      case '\\':
        out += '\\\\';
        break;
      case '\b':
        out += '\\b';
        break;
      case '\f':
        out += '\\f';
        break;
      case '\n':
        out += '\\n';
        break;
      case '\r':
        out += '\\r';
        break;
      case '\t':
        out += '\\t';
        break;
      default: {
        const code = ch.codePointAt(0) ?? 0;
        // Python 只把 < 0x20 的控制字符转义成 \u00xx（小写），其余（含非 ASCII）原样输出。
        out += code < 0x20 ? `\\u${code.toString(16).padStart(4, '0')}` : ch;
      }
    }
  }
  return out;
}

function formatNumber(value: number): string {
  if (Number.isNaN(value)) return 'NaN';
  if (value === Infinity) return 'Infinity';
  if (value === -Infinity) return '-Infinity';
  if (Number.isInteger(value)) return String(value);
  return String(value);
}

function dump(value: Json, level: number): string {
  const inner = '  '.repeat(level + 1);
  const outer = '  '.repeat(level);

  if (value === null) return 'null';
  if (typeof value === 'boolean') return value ? 'true' : 'false';
  if (typeof value === 'number') return formatNumber(value);
  if (typeof value === 'string') return `"${escapeString(value)}"`;

  if (Array.isArray(value)) {
    if (value.length === 0) return '[]';
    const items = value.map((item) => inner + dump(item, level + 1));
    return `[\n${items.join(',\n')}\n${outer}]`;
  }

  const keys = Object.keys(value);
  if (keys.length === 0) return '{}';
  const items = keys.map((key) => `${inner}"${escapeString(key)}": ${dump(value[key], level + 1)}`);
  return `{\n${items.join(',\n')}\n${outer}}`;
}

/** 等价于 `json.dumps(obj, indent=2, ensure_ascii=False)`（**不带**结尾换行）。 */
export function pyJsonDumps(value: Json): string {
  return dump(value, 0);
}
