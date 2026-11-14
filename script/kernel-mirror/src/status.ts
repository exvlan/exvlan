/**
 * `mirror-status.json` 双写（原 `mirror_status.py`）。
 *
 * - 新 schema：`<DEST>/mirror-status.json`，按通道汇总 `channels.*`；顶层 `latest` /
 *   `versions` / `bytes` 仍是 **stable** 的汇总（旧监控直接读这几项）。
 * - 旧 schema 兼容文件：`<EXVLAN_DIST>/mirror-status.json`（发布机看门狗读它，并按
 *   **mtime** 判「同步停滞」——所以正常轮次与早退路径都必须写）。
 *
 * 文件已存在时先读出来再覆盖本模块负责的键：看门狗写的其它字段（`disk_free` 等）
 * 原样保留；键顺序也保持（Python `dict.update` 语义），避免字节级 diff 噪音。
 */

import fs from 'node:fs';
import path from 'node:path';
import type { Channel, Config } from './config';
import { CHANNELS } from './config';
import { pyJsonDumps, type Json } from './pyjson';
import { apparentSize, ensureDir, isDir, isFile, readTextOr, writeFileAtomic } from './fsx';
import { channelLatest, channelVersions, type JsonObject } from './manifest';

const SPLIT_RE = /[,\s]+/;

export interface ChannelStatus {
  latest: string;
  tags: string[];
  bytes: number;
  core_bytes: number;
  upgrade_bytes: number;
}

export type ChannelStatusMap = Record<string, ChannelStatus>;

function pyRepr(value: string): string {
  return `'${value.replace(/\\/g, '\\\\').replace(/'/g, "\\'")}'`;
}

function parseByteField(raw: string | undefined): number | null {
  if (raw === undefined || raw === '') return 0;
  const trimmed = raw.trim();
  // Python `int()`：允许首尾空白，不接受小数/其它字符
  if (!/^[+-]?\d+$/.test(trimmed)) return null;
  return Number.parseInt(trimmed, 10);
}

/** 解析 channels 输入（`channel \t latest \t core_bytes \t upgrade_bytes \t tags`）。 */
export function readChannels(file: string): ChannelStatusMap {
  const channels: ChannelStatusMap = {};
  if (!isFile(file)) {
    process.stderr.write(`[sync] 状态输入不存在，按空处理：${file}\n`);
    return channels;
  }
  const lines = readTextOr(file, '').split('\n');
  lines.forEach((line, index) => {
    if (line.trim() === '') return;
    const parts = line.split('\t');
    if (parts.length < 1 || !parts[0]) {
      process.stderr.write(`[sync] 状态输入第 ${index + 1} 行不规范，跳过：${pyRepr(line)}\n`);
      return;
    }
    const padded = [...parts, '', '', '', '', ''].slice(0, 5);
    const [name, latest, coreBytesRaw, upgradeBytesRaw, tags] = padded;
    let core = 0;
    let upgrade = 0;
    const parsedCore = parseByteField(coreBytesRaw);
    const parsedUpgrade = parseByteField(upgradeBytesRaw);
    if (parsedCore === null || parsedUpgrade === null) {
      process.stderr.write(`[sync] 状态输入第 ${index + 1} 行字节数非数字，按 0 处理：${pyRepr(line)}\n`);
    } else {
      core = parsedCore;
      upgrade = parsedUpgrade;
    }
    channels[name] = {
      latest,
      tags: tags.trim() === '' ? [] : tags.trim().split(SPLIT_RE).filter((t) => t !== ''),
      bytes: core + upgrade,
      core_bytes: core,
      upgrade_bytes: upgrade,
    };
  });
  return channels;
}

function freeGbOf(file: string): number {
  const candidates = [path.dirname(file), path.dirname(path.dirname(file))];
  for (const candidate of candidates) {
    try {
      const stat = fs.statfsSync(candidate);
      return Math.trunc((stat.bavail * stat.bsize) / 2 ** 30);
    } catch {
      continue;
    }
  }
  return 0;
}

function timestamp(): string {
  return new Date().toISOString().replace(/\.\d{3}Z$/, 'Z');
}

/** 等价 Python `dict.update`：已有键保持原位置，新键追加在末尾。 */
function updatePreservingOrder(existing: JsonObject, updates: JsonObject): JsonObject {
  const out: JsonObject = {};
  for (const [key, value] of Object.entries(existing)) {
    out[key] = key in updates ? updates[key] : value;
  }
  for (const [key, value] of Object.entries(updates)) {
    if (!(key in out)) out[key] = value;
  }
  return out;
}

function loadJsonObject(file: string): JsonObject | null {
  if (!isFile(file)) return null;
  try {
    const data = JSON.parse(readTextOr(file, '')) as Json;
    if (data === null || typeof data !== 'object' || Array.isArray(data)) {
      process.stderr.write(`[sync] 旧状态文件不是对象，覆盖：${file}\n`);
      return null;
    }
    return data as JsonObject;
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    process.stderr.write(`[sync] 旧状态文件无法解析（覆盖）：${detail}\n`);
    return null;
  }
}

export interface StatusWriteOptions {
  result: 'ok' | 'waiting' | 'error';
  detail: string;
  sinceTag: string;
  maxBytes: number;
}

/** 写新 schema 状态文件。 */
export function writeChannelsStatus(cfg: Config, channels: ChannelStatusMap, file: string): void {
  const existing = loadJsonObject(file) ?? {};
  const values = Object.values(channels);
  const coreBytes = values.reduce((sum, c) => sum + c.core_bytes, 0);
  const upgradeBytes = values.reduce((sum, c) => sum + c.upgrade_bytes, 0);
  const stable = channels.stable;

  const updates: JsonObject = {
    updated: timestamp(),
    repo: cfg.repo,
    dest: cfg.dest,
    upgrade_dest: cfg.upgradeDest,
    // 兼容旧监控：顶层仍是 stable 的汇总
    latest: stable?.latest ?? '',
    versions: (stable?.tags ?? []) as Json,
    bytes: coreBytes + upgradeBytes,
    core_bytes: coreBytes,
    upgrade_bytes: upgradeBytes,
    channels: channels as unknown as Json,
  };

  const doc = updatePreservingOrder(existing, updates);
  ensureDir(path.dirname(file));
  writeFileAtomic(file, `${pyJsonDumps(doc)}\n`);
  process.stdout.write(`[sync] 状态已写入 ${file}（${values.length} 个通道）\n`);
}

/** 写旧 schema 兼容文件（发布机看门狗读的那份）。 */
export function writeCompatStatus(file: string, channels: ChannelStatusMap, options: StatusWriteOptions): void {
  const existing = loadJsonObject(file) ?? {};
  const now = timestamp();

  const stable = channels.stable;
  const latest = stable?.latest ?? '';
  const tags = stable?.tags ?? [];
  const coreBytes = Object.values(channels).reduce((sum, c) => sum + c.core_bytes, 0);
  const appBytes = Object.values(channels).reduce((sum, c) => sum + c.upgrade_bytes, 0);

  let previousRound = 0;
  if (typeof existing.round === 'number' && Number.isFinite(existing.round)) previousRound = Math.trunc(existing.round);
  else if (typeof existing.round === 'string') previousRound = parseByteField(existing.round) ?? 0;
  const round = previousRound + 1;

  let firstOk = typeof existing.first_ok_at === 'string' ? existing.first_ok_at : '';
  if (options.result === 'ok' && !firstOk) firstOk = now;

  // 只有 stable 最新版真的变了才算「有变化」，避免每轮都刷新时间
  let lastChange = typeof existing.last_change_at === 'string' ? existing.last_change_at : '';
  const previousLatest = typeof existing.latest_release === 'string' ? existing.latest_release : '';
  if (options.result === 'ok' && latest && latest !== previousLatest) lastChange = now;

  let waitSince = options.result === 'waiting' && typeof existing.wait_since === 'string' ? existing.wait_since : '';
  if (options.result === 'waiting' && !waitSince) waitSince = now;

  const detail = options.detail
    ? options.detail
    : channels && Object.keys(channels).length > 0
      ? `同步完成: ${tags.join(',')}`
      : '本轮没有可同步的 release';

  const doc: JsonObject = {
    checked_at: now,
    round,
    result: options.result,
    detail: detail ? detail : null,
    since_tag: options.sinceTag ? options.sinceTag : null,
    latest_release: latest ? latest : null,
    tags: tags as Json,
    first_ok_at: firstOk ? firstOk : null,
    last_change_at: lastChange ? lastChange : null,
    wait_since: waitSince ? waitSince : null,
    bytes: {
      core: coreBytes,
      app: appBytes,
      total: coreBytes + appBytes,
      max: Math.trunc(options.maxBytes),
    },
    free_gb: freeGbOf(file),
    window: { core: tags.length, app: tags.length },
  };

  ensureDir(path.dirname(file));
  writeFileAtomic(file, `${pyJsonDumps(doc)}\n`);
  process.stdout.write(`[sync] 兼容状态已写入 ${file}（旧 schema，round=${round}）\n`);
}

/** 按目录实测每通道占用，组装状态输入。空通道（core 与 app 都不存在）不汇报。 */
export function buildStatusInput(cfg: Config): ChannelStatusMap {
  const channels: ChannelStatusMap = {};
  for (const channel of CHANNELS as readonly Channel[]) {
    const coreDir = path.join(cfg.dest, channel);
    const appDir = path.join(cfg.upgradeDest, channel);
    if (!isDir(coreDir) && !isDir(appDir)) continue;

    let source = path.join(coreDir, 'manifest.json');
    if (!isFile(source)) source = path.join(appDir, 'manifest.json');
    const sourceText = isFile(source) ? readTextOr(source, '') : '';

    const coreBytes = apparentSize(coreDir);
    const appBytes = apparentSize(appDir);
    channels[channel] = {
      latest: channelLatest(sourceText),
      tags: channelVersions(sourceText)
        .split(',')
        .filter((t) => t !== ''),
      bytes: coreBytes + appBytes,
      core_bytes: coreBytes,
      upgrade_bytes: appBytes,
    };
  }
  return channels;
}

/** 写两份状态（新 + 旧 schema）。 */
export function writeStatusFiles(cfg: Config, options: StatusWriteOptions): void {
  const channels = buildStatusInput(cfg);
  writeChannelsStatus(cfg, channels, cfg.mirrorStatus);
  writeCompatStatus(cfg.mirrorStatusCompat, channels, options);
}
