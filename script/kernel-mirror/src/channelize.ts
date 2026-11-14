/**
 * 把清单里的绝对 URL 改写到**通道目录**（原 `channelize_urls.py`）。
 *
 * `upgrade-latest.json`（镜像为 `<channel>/latest.json`）与 `magisk_update.json` 里的
 * 下载地址是绝对 URL（Tauri updater 只接受 URL）。镜像按通道存放后必须指向自己的
 * 通道目录，否则老客户端拿到 `/upgrade/latest.json`（stable 的符号链接）也会去根目录
 * 下载 —— 那里已经没有文件了。
 *
 * 规则：保留 origin，路径重建成 `<root>/<channel>/<文件名>`；只动 URL 字段，幂等。
 */

import fs from 'node:fs';
import path from 'node:path';
import { info } from './log';
import { writeFileAtomic } from './fsx';
import { pyJsonDumps, type Json } from './pyjson';

const URL_FIELDS = ['url', 'zipUrl'] as const;

/** Python `urllib.parse.quote(name)`（safe='/'，其余按 UTF-8 百分号大写编码）。 */
function quotePathSegment(value: string): string {
  let out = '';
  for (const byte of Buffer.from(value, 'utf8')) {
    const isUnreserved =
      (byte >= 0x41 && byte <= 0x5a) ||
      (byte >= 0x61 && byte <= 0x7a) ||
      (byte >= 0x30 && byte <= 0x39) ||
      byte === 0x5f || // _
      byte === 0x2e || // .
      byte === 0x2d || // -
      byte === 0x7e; // ~
    const isSafe = byte === 0x2f; // /
    out += isUnreserved || isSafe ? String.fromCharCode(byte) : `%${byte.toString(16).toUpperCase().padStart(2, '0')}`;
  }
  return out;
}

/** `https://host/upgrade/<任意>/<file>` → `https://host/upgrade/<channel>/<file>`。 */
export function channelizeUrl(rawUrl: string, channel: string, root = '/upgrade'): { url: string; changed: boolean } {
  let parsed: URL;
  try {
    parsed = new URL(rawUrl);
  } catch {
    return { url: rawUrl, changed: false };
  }
  if (!parsed.protocol || !parsed.hostname) return { url: rawUrl, changed: false };

  let decoded: string;
  try {
    decoded = decodeURIComponent(parsed.pathname);
  } catch {
    decoded = parsed.pathname;
  }
  const name = decoded.slice(decoded.lastIndexOf('/') + 1);
  if (!name) return { url: rawUrl, changed: false };

  const base = root.replace(/\/+$/, '');
  const rewritten = `${parsed.protocol}//${parsed.host}${base}/${channel}/${quotePathSegment(name)}`;
  return { url: rewritten, changed: true };
}

function rewriteNode(node: Json, channel: string, root: string): number {
  if (node === null || typeof node !== 'object' || Array.isArray(node)) return 0;
  const obj = node as { [key: string]: Json };
  let changed = 0;

  const platforms = obj.platforms;
  if (platforms !== null && typeof platforms === 'object' && !Array.isArray(platforms)) {
    for (const entry of Object.values(platforms)) {
      if (entry === null || typeof entry !== 'object' || Array.isArray(entry)) continue;
      const platform = entry as { [key: string]: Json };
      for (const field of URL_FIELDS) {
        const value = platform[field];
        if (typeof value === 'string' && value.length > 0) {
          const result = channelizeUrl(value, channel, root);
          platform[field] = result.url;
          changed += result.changed ? 1 : 0;
        }
      }
    }
  }
  for (const field of URL_FIELDS) {
    const value = obj[field];
    if (typeof value === 'string' && value.length > 0) {
      const result = channelizeUrl(value, channel, root);
      obj[field] = result.url;
      changed += result.changed ? 1 : 0;
    }
  }
  return changed;
}

/** 原地改写 `file` 里的 URL。返回是否成功（解析失败 = false）。 */
export function channelizeFile(file: string, channel: string, root = '/upgrade'): boolean {
  let data: Json;
  try {
    data = JSON.parse(fs.readFileSync(file, 'utf8')) as Json;
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    process.stderr.write(`[sync] 解析 ${file} 失败: ${detail}\n`);
    return false;
  }

  const changed = rewriteNode(data, channel, root);
  writeFileAtomic(file, `${pyJsonDumps(data)}\n`);

  if (changed === 0) {
    // 没有 URL 字段（例如只有自建清单）属正常，不当作失败。
    info(`URL 通道化：${path.basename(file)} 没有绝对 URL，保持原样`);
    return true;
  }
  info(`URL 通道化：${path.basename(file)} 的 ${changed} 条地址 → ${channel}/`);
  return true;
}
