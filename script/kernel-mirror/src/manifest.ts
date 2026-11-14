/**
 * 清单处理：合并（原 `merge_manifests.py`）、平台裁剪（原 `filter_manifest_platforms.py`）、
 * 以及旧脚本里用 grep/sed 从 JSON 文本抽字段的那几个helper（行为保持逐字一致）。
 */

import fs from 'node:fs';
import path from 'node:path';
import { info, SyncError } from './log';
import { pyJsonDumps, type Json } from './pyjson';
import { ensureDir, writeFileAtomic } from './fsx';

export type JsonObject = { [key: string]: Json };

const VERSION_RE = /^v?(\d+)\.(\d+)\.(\d+)(?:-([A-Za-z]+)\.(\d+))?(?:\+.*)?$/;
// 与 packages/studio/src-tauri/src/upgrade.rs 的 Channel::compare_stage 对齐
const STAGE_RANK: Record<string, number> = { stable: 5, rc: 4, beta: 3, dev: 2, nightly: 1 };

export type VersionKey = [number, number, number, number, number];

/**
 * `26.410.20712-rc.1` → `[26, 410, 20712, 4, 1]`。
 * 旧实现按 `.` 切分后丢掉非纯数字段，预发布版本之间无法比较先后 —— 这里按契约解析。
 */
export function versionKey(version: string): VersionKey {
  const match = VERSION_RE.exec(version.trim());
  if (!match) return [0, 0, 0, 0, 0];
  const [, major, minor, patch, stage, seq] = match;
  return [
    Number(major),
    Number(minor),
    Number(patch),
    STAGE_RANK[(stage ?? 'stable').toLowerCase()] ?? 0,
    Number(seq ?? 0),
  ];
}

function compareKeys(a: VersionKey, b: VersionKey): number {
  for (let i = 0; i < a.length; i += 1) {
    if (a[i] !== b[i]) return a[i] < b[i] ? -1 : 1;
  }
  return 0;
}

export function readJsonObject(file: string): JsonObject {
  const text = fs.readFileSync(file, 'utf8');
  const data = JSON.parse(text) as Json;
  if (data === null || typeof data !== 'object' || Array.isArray(data)) {
    throw new SyncError(`${file} 不是 JSON 对象`);
  }
  return data;
}

export function parseJsonText(text: string): Json {
  return JSON.parse(text) as Json;
}

/** 清单里 `"file": "xxx"` 的值（相对文件名），保持出现顺序与重复。 */
export function manifestFiles(text: string): string[] {
  const out: string[] = [];
  const re = /"file"[ \t]*:[ \t]*"([^"]+)"/g;
  for (;;) {
    const match = re.exec(text);
    if (!match) break;
    out.push(match[1]);
  }
  return out;
}

/** 从 JSON 文本抽某字段 URL 的文件名（`"url"` / `"zipUrl"`），语义同旧 sed 管道。 */
export function jsonUrlNames(text: string, field: string): string[] {
  const out: string[] = [];
  const re = new RegExp(`"${field}"[ \\t]*:[ \\t]*"([^"]+)"`, 'g');
  for (;;) {
    const match = re.exec(text);
    if (!match) break;
    const url = match[1];
    const slash = url.lastIndexOf('/');
    // 旧 sed 规则：`s#.*/([^/"]+)"$#\1#`，没有斜杠时整段匹配落回原样（会触发半发布防御）。
    out.push(slash >= 0 ? url.slice(slash + 1) : `"${field}": "${url}"`);
  }
  return out;
}

/** 清单顶层（或版本条目里）第一个 `"version"` 字段。 */
export function jsonVersion(text: string): string {
  for (const line of text.split('\n')) {
    const match = /^[ \t]*"version"[ \t]*:[ \t]*"([^"]+)"/.exec(line);
    if (match) return match[1];
  }
  return '';
}

/** 清单里 `"latest"` 的值（每行取最后一次出现，再取首行 —— 复刻 `sed | head -1`）。 */
export function channelLatest(text: string): string {
  for (const line of text.split('\n')) {
    const matches = [...line.matchAll(/"latest"[ \t]*:[ \t]*"([^"]+)"/g)];
    if (matches.length > 0) return matches[matches.length - 1][1];
  }
  return '';
}

/** 清单里所有 `"version"` 值（每行最后一次出现），逗号连接（复刻 `sed | tr '\n' ','`）。 */
export function channelVersions(text: string): string {
  const versions: string[] = [];
  for (const line of text.split('\n')) {
    const matches = [...line.matchAll(/"version"[ \t]*:[ \t]*"([^"]+)"/g)];
    if (matches.length > 0) versions.push(matches[matches.length - 1][1]);
  }
  return versions.join(',');
}

export interface MergeOptions {
  keep: number;
  since: string;
}

/**
 * 合并多个清单的 `versions[]`：同一版本以**先出现**的为准（新下载的在前，
 * 这样重传过的资产用新的 sha256）；`keep` 只留最近 N 个版本，`since` 丢掉更早的。
 * 失败（没有任何版本）返回 false，与旧脚本的退出码 1 对应。
 */
export function mergeManifests(manifests: readonly string[], outFile: string, options: MergeOptions): boolean {
  const versions = new Map<string, Json>();
  let channel = '';

  for (const manifestPath of manifests) {
    let data: JsonObject;
    try {
      data = readJsonObject(manifestPath);
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      process.stderr.write(`[sync] 跳过损坏的 manifest ${manifestPath}: ${detail}\n`);
      continue;
    }
    if (!channel && typeof data.channel === 'string') {
      // 通道字段由发版流水线写入（排查用），合并时保留
      channel = data.channel;
    }
    const entries = Array.isArray(data.versions) ? data.versions : [];
    for (const entry of entries) {
      if (entry === null || typeof entry !== 'object' || Array.isArray(entry)) continue;
      const name = entry.version;
      if (typeof name !== 'string' || name.length === 0) continue;
      if (!versions.has(name)) versions.set(name, entry);
    }
  }

  if (versions.size === 0) {
    process.stderr.write('[sync] manifest 里没有任何版本\n');
    return false;
  }

  // Array.prototype.sort 是稳定排序：同 key 时保持插入顺序（与 Python sorted 一致）。
  let ordered = [...versions.keys()].sort((a, b) => compareKeys(versionKey(b), versionKey(a)));

  if (options.since) {
    const floor = versionKey(options.since);
    const dropped = ordered.filter((name) => compareKeys(versionKey(name), floor) < 0);
    ordered = ordered.filter((name) => compareKeys(versionKey(name), floor) >= 0);
    for (const name of dropped) {
      process.stderr.write(`[sync] 早于 SINCE_TAG=${options.since}，移出清单：${name}\n`);
    }
  }
  if (options.keep > 0) {
    const dropped = ordered.slice(options.keep);
    ordered = ordered.slice(0, options.keep);
    for (const name of dropped) {
      process.stderr.write(`[sync] 超出保留数（--keep ${options.keep}），移出清单：${name}\n`);
    }
  }

  const merged: JsonObject = {
    schema: 1,
    latest: ordered[0],
    versions: ordered.map((name) => versions.get(name) as Json),
  };
  if (channel) merged.channel = channel;

  ensureDir(path.dirname(outFile));
  // Python 的 `write_text` 不带结尾换行（下一个过滤器会补）
  writeFileAtomic(outFile, pyJsonDumps(merged));
  info(`manifest: ${ordered.length} 个版本, latest=${ordered[0]}`);
  return true;
}

function pyListRepr(values: readonly string[]): string {
  return `[${values.map((value) => `'${value.replace(/\\/g, '\\\\').replace(/'/g, "\\'")}'`).join(', ')}]`;
}

/**
 * 把合并后的内核清单裁剪到「镜像实际托管」的平台。
 * 只托管的平台之外的版本会被剔除（否则 GUI 看到更新却下载 404）。
 */
export function filterPlatforms(file: string, platforms: readonly string[]): void {
  const data = readJsonObject(file);
  const allowed = new Set(platforms);
  const kept: Json[] = [];

  const versions = Array.isArray(data.versions) ? data.versions : [];
  for (const entry of versions) {
    if (entry === null || typeof entry !== 'object' || Array.isArray(entry)) continue;
    const version = entry as JsonObject;
    const assets = (version.assets ?? {}) as JsonObject;
    const filtered: JsonObject = {};
    for (const [name, meta] of Object.entries(assets)) {
      if (allowed.has(name)) filtered[name] = meta;
    }
    if (Object.keys(filtered).length > 0) {
      version.assets = filtered;
      kept.push(version);
    } else if (Object.keys(assets).length > 0) {
      process.stderr.write(`[sync] ${String(version.version)} 只有未镜像平台，已从清单剔除\n`);
    }
  }

  data.versions = kept;
  writeFileAtomic(file, `${pyJsonDumps(data)}\n`);
  info(`清单平台过滤完成：保留 ${kept.length} 个版本，平台 ${pyListRepr([...allowed].sort())}`);
}
