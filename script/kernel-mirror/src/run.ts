/**
 * 主流程（等价旧脚本的 `main`）：取元数据 → 分通道 → 选窗口 → 下载 → 发布 → 状态。
 *
 * 顺序与旧脚本一致，别随手调整：状态文件（含看门狗按 mtime 判活）必须在**所有**
 * 早退路径上写；别名必须在发布之后、旧布局兼容链接之前；sync-state 只记录
 * 「core 真正发布成功」的通道。
 */

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import type { Channel, Config } from './config';
import { CHANNELS } from './config';
import { info, warn } from './log';
import { assetTablePath, collectReleaseMetadata, downloadRelease } from './gh';
import {
  ensureDir,
  exists,
  fingerprintOfAssetTable,
  isDir,
  isFile,
  readTextOr,
  rmrf,
  symlinkAtomic,
  writeFileAtomic,
} from './fsx';
import { buildChannelLists, releasePublishedAt, selectTags, type ChannelLists } from './tags';
import { manifestFiles } from './manifest';
import { publishApp, publishCore, type PublishOutcome } from './publish';
import { writeStatusFiles } from './status';

const PLATFORM_ZIP_RE_CACHE = new Map<string, RegExp>();

function platformZipRe(platforms: readonly string[]): RegExp {
  const key = platforms.join(' ');
  const cached = PLATFORM_ZIP_RE_CACHE.get(key);
  if (cached) return cached;
  const escaped = platforms.map((p) => p.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
  const re = new RegExp(`^(${escaped.join('|')})-.*[.]zip$`);
  PLATFORM_ZIP_RE_CACHE.set(key, re);
  return re;
}

function coreAssetNames(workDir: string, tag: string, platforms: readonly string[]): string[] {
  const file = assetTablePath(workDir, tag);
  let text: string;
  try {
    text = fs.readFileSync(file, 'utf8');
  } catch {
    return [];
  }
  if (text.length === 0) return [];
  const re = platformZipRe(platforms);
  const names: string[] = [];
  for (const line of text.split('\n')) {
    if (line === '') continue;
    const name = line.split('\t')[0] ?? '';
    if (re.test(name)) names.push(name);
  }
  return names;
}

/** `.sync-state` / `.published-at` 里的单值读取（awk 的「首个匹配」语义）。 */
function readStateField(file: string, tag: string): string {
  for (const line of readTextOr(file, '').split('\n')) {
    if (line === '') continue;
    const [key, value] = line.split('\t');
    if (key === tag) return value ?? '';
  }
  return '';
}

export interface RunContext {
  cfg: Config;
  workDir: string;
  releasesFile: string;
  lists: ChannelLists;
}

function needDownload(ctx: RunContext, channel: Channel, tag: string): boolean {
  const { cfg, workDir, releasesFile } = ctx;
  if (cfg.force) return true;

  const channelDir = path.join(cfg.dest, channel);
  const selection = ctx.lists.selections[channel];

  if (channel === 'nightly') {
    // 固定 tag：published_at 是主要信号；再叠一层资产指纹，
    // 防止「同一个 tag 覆盖上传」时 published_at 不变而漏掉新构建。
    const publishedAt = releasePublishedAt(releasesFile, tag);
    const lastPublished = readTextOr(path.join(channelDir, '.published-at'), '').replace(/\n+$/, '');
    if (publishedAt && publishedAt === lastPublished) {
      const fingerprint = fingerprintOfAssetTable(assetTablePath(workDir, tag));
      if (fingerprint === null) return true;
      const lastFingerprint = readStateField(path.join(channelDir, '.sync-state'), tag);
      return fingerprint !== lastFingerprint;
    }
    return true;
  }

  const fingerprint = fingerprintOfAssetTable(assetTablePath(workDir, tag));
  if (fingerprint === null) return true;
  const recorded = readStateField(path.join(channelDir, '.sync-state'), tag);
  if (recorded !== fingerprint) return true;

  if (selection.core.includes(tag)) {
    for (const name of coreAssetNames(workDir, tag, cfg.platforms)) {
      if (!isFile(path.join(channelDir, name))) return true;
    }
  }
  if (selection.app.includes(tag)) {
    const version = tag.replace(/^v/, '');
    const manifest = path.join(cfg.upgradeDest, channel, 'manifest.json');
    if (!isFile(manifest)) return true;
    if (!readTextOr(manifest, '').includes(`"version": "${version}"`)) return true;
  }
  return false;
}

function downloadChannels(ctx: RunContext): number {
  const { cfg } = ctx;
  let downloaded = 0;
  for (const channel of CHANNELS) {
    const selection = ctx.lists.selections[channel];
    if (selection.dl.length === 0) continue;
    for (const tag of selection.dl) {
      if (!needDownload(ctx, channel, tag)) {
        info(`${channel}/${tag}：指纹/发布时间未变，跳过下载`);
        continue;
      }
      const rel = path.join(ctx.workDir, 'rel', channel, tag);
      rmrf(rel);
      ensureDir(rel);
      if (!downloadRelease(cfg, tag, rel)) {
        warn(`${channel}/${tag} 下载失败，本轮跳过该 tag`);
        rmrf(rel);
        continue;
      }
      downloaded += 1;
      info(`${channel}/${tag}：下载 ${countFiles(rel)} 个文件`);
    }
  }
  info(`下载完成：${downloaded} 个 tag 有新内容`);
  return downloaded;
}

function countFiles(dir: string): number {
  let total = 0;
  const walk = (current: string): void => {
    let entries: fs.Dirent[];
    try {
      entries = fs.readdirSync(current, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) walk(full);
      else total += 1;
    }
  };
  walk(dir);
  return total;
}

function writeSyncState(ctx: RunContext, channel: Channel): void {
  const { cfg } = ctx;
  const channelDir = path.join(cfg.dest, channel);
  if (!isDir(channelDir)) return;

  const rows: string[] = [];
  for (const tag of ctx.lists.selections[channel].dl) {
    if (isDir(path.join(ctx.workDir, 'rel', channel, tag))) {
      // 本轮重新下载过：记录新指纹
      const fingerprint = fingerprintOfAssetTable(assetTablePath(ctx.workDir, tag));
      if (fingerprint === null) continue;
      rows.push(`${tag}\t${fingerprint}`);
    } else {
      // 本轮因指纹命中而跳过下载：沿用上次记录的指纹，避免下次被迫重下
      const fingerprint = readStateField(path.join(channelDir, '.sync-state'), tag);
      if (!fingerprint) continue;
      rows.push(`${tag}\t${fingerprint}`);
    }
  }
  const target = path.join(channelDir, '.sync-state');
  writeFileAtomic(target, rows.length === 0 ? '' : `${rows.join('\n')}\n`, `${target}.tmp`);
}

function writePublishedAt(ctx: RunContext, channel: Channel, tag: string): void {
  const publishedAt = releasePublishedAt(ctx.releasesFile, tag);
  if (!publishedAt) return;
  const channelDir = path.join(ctx.cfg.dest, channel);
  if (!isDir(channelDir)) return;
  const target = path.join(channelDir, '.published-at');
  writeFileAtomic(target, `${publishedAt}\n`, `${target}.tmp`);
}

/** 别名：符号链接指向 stable（不复制文件），用 rename 原子替换。 */
function linkAlias(cfg: Config, target: string, dest: string): void {
  const parent = path.dirname(dest);
  if (!isFile(path.join(parent, target))) {
    info(`别名跳过：${parent}/${target} 不存在`);
    return;
  }
  if (cfg.dryRun) {
    info(`别名（dry-run）：${dest} → ${target}`);
    return;
  }
  symlinkAtomic(target, dest);
  info(`别名：${dest} → ${target}`);
}

/**
 * 老布局兼容：老客户端缓存过的旧清单、以及发布机看门狗（按 `core/<file>` 校验
 * 清单引用的 zip 是否存在）都按 `DEST/<zip>` 找内核包，而文件现在在 `DEST/stable/`。
 * 为缺失的名字补**相对符号链接**：不复制、不占空间，也不动根上已有的真实旧文件。
 */
function linkLegacyCoreFiles(cfg: Config, manifestFile: string, dest: string): void {
  if (!isFile(manifestFile)) return;
  if (cfg.dryRun) {
    info(`旧布局兼容（dry-run）：${dest} 根上缺失的内核 zip 会补符号链接 → stable/`);
    return;
  }
  let count = 0;
  for (const name of manifestFiles(readTextOr(manifestFile, ''))) {
    if (!name) continue;
    if (name.includes('/')) continue;
    if (!isFile(path.join(dest, 'stable', name))) continue;
    if (exists(path.join(dest, name))) continue;
    fs.symlinkSync(`stable/${name}`, path.join(dest, name));
    count += 1;
  }
  if (count > 0) {
    info(`旧布局兼容：${dest} 根上补了 ${count} 个指向 stable/ 的符号链接`);
  }
}

function humanSize(bytes: number): string {
  const units = ['B', 'K', 'M', 'G', 'T', 'P'];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  if (unit === 0) return `${value}B`;
  // `du -h` 向上取整到 1 位小数
  const rounded = Math.ceil(value * 10) / 10;
  return `${rounded.toFixed(1)}${units[unit]}`;
}

function dirSize(dir: string): number {
  // 统计用 `du -sh` 语义（表观大小），仅用于日志
  const stack = [dir];
  let total = 0;
  while (stack.length > 0) {
    const current = stack.pop() as string;
    let entries: fs.Dirent[];
    try {
      entries = fs.readdirSync(current, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      const full = path.join(current, entry.name);
      const stat = fs.lstatSync(full);
      total += stat.size;
      if (entry.isDirectory()) stack.push(full);
    }
  }
  return total;
}

function countLines(list: readonly string[]): number {
  return list.length;
}

function printPlan(ctx: RunContext): void {
  const { cfg, lists } = ctx;
  info('dry-run：不下载、不写盘');
  const counts = CHANNELS.map((ch) => `${ch}=${countLines(lists.selections[ch].tags)} `).join('');
  info(`分类结果：${counts}`);
  const legacy = lists.skipped.join(' ');
  if (legacy) info(`跳过（legacy / 非通道 tag）：${legacy}`);
  info(
    `保留策略：stable core=${cfg.keepCoreStable} app=${cfg.keepAppStable}；rc=${cfg.keepRc} beta=${cfg.keepBeta} dev=${cfg.keepDev}；nightly=1`,
  );
  info(`MAX_BYTES=${cfg.maxBytes}（core+app 合计预算，超限从最旧版本收缩窗口；0 = 不限）`);
  if (cfg.sinceTag) info(`SINCE_TAG=${cfg.sinceTag}（只约束 stable）`);

  let total = 0;
  for (const channel of CHANNELS) {
    const selection = lists.selections[channel];
    const coreN = selection.core.length;
    const appN = selection.app.length;
    if (coreN === 0 && appN === 0) {
      info(`[${channel}] 无 release，不创建目录`);
      continue;
    }
    info(`[${channel}] core 同步 ${coreN} 个 tag：${selection.core.join(' ')}`);
    if (appN > 0 && selection.core.join('\n') !== selection.app.join('\n')) {
      info(`[${channel}] app  同步 ${appN} 个 tag：${selection.app.join(' ')}`);
    }
    for (const tag of selection.core) {
      info(`[${channel}] core → ${cfg.dest}/${channel}/manifest.json`);
      for (const platform of cfg.platforms) {
        info(`[${channel}] core → ${cfg.dest}/${channel}/${platform}-${tag}.zip（预计名，实际以 Release 资产名为准）`);
      }
    }
    if (appN > 0) {
      info(`[${channel}] app  → ${cfg.upgradeDest}/${channel}/{manifest.json,latest.json,magisk_update.json} + 安装包/签名`);
    }
    total += coreN + appN;
  }

  info(`别名：${cfg.dest}/manifest.json → stable/manifest.json`);
  info(`别名：${cfg.upgradeDest}/manifest.json → stable/manifest.json`);
  info(`别名：${cfg.upgradeDest}/latest.json → stable/latest.json`);
  info(`别名：${cfg.upgradeDest}/magisk_update.json → stable/magisk_update.json`);
  info(`状态文件：${cfg.mirrorStatus}（channels.{stable,rc,beta,dev,nightly}）`);
  info(`状态文件：${cfg.mirrorStatusCompat}（旧 schema 兼容，发布机看门狗读它）`);
  info(`旧布局兼容：缺失的 ${cfg.dest}/<zip> 会补符号链接 → stable/`);
  info(`dry-run 结束：共 ${total} 个通道-版本待同步`);
}

interface LockHandle {
  release(): void;
}

/** 可选跨进程互斥（`MIRROR_SYNC_LOCK`）；不设置时完全不加锁，与旧脚本一致。 */
function acquireLock(lockFile: string): LockHandle | null {
  const staleMs = 6 * 60 * 60 * 1000;
  const tryCreate = (): number | null => {
    try {
      return fs.openSync(lockFile, 'wx');
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (code === 'EEXIST') return null;
      throw error;
    }
  };

  let fd = tryCreate();
  if (fd === null) {
    let stale = false;
    try {
      stale = Date.now() - fs.statSync(lockFile).mtimeMs > staleMs;
    } catch {
      stale = true;
    }
    if (!stale) return null;
    warn(`发现陈旧的同步锁 ${lockFile}（>6h），按崩溃遗留处理并接管`);
    rmrf(lockFile);
    fd = tryCreate();
    if (fd === null) return null;
  }
  fs.writeSync(fd, `${process.pid}\n`);
  fs.closeSync(fd);
  return {
    release: () => {
      rmfs(lockFile);
    },
  };
}

function rmfs(target: string): void {
  try {
    fs.rmSync(target, { force: true });
  } catch (error) {
    warn(`删除 ${target} 失败（忽略）：${error instanceof Error ? error.message : String(error)}`);
  }
}

/** 入口：跑完一轮同步，返回进程退出码。 */
export function runOnce(cfg: Config): number {
  const workDir = fs.mkdtempSync(path.join(os.tmpdir(), 'exvlan-mirror-'));
  let lock: LockHandle | null = null;

  const cleanup = (): void => {
    lock?.release();
    rmrf(workDir);
  };

  try {
    if (cfg.syncLock) {
      lock = acquireLock(cfg.syncLock);
      if (!lock) {
        warn(`已有一轮同步在进行（锁 ${cfg.syncLock}），本轮跳过`);
        return 75;
      }
    }

    const metadata = collectReleaseMetadata(cfg, workDir);
    const lists = buildChannelLists(metadata.releasesFile);
    for (const channel of CHANNELS) {
      lists.selections[channel] = selectTags(cfg, channel, lists.selections[channel], workDir);
    }

    const ctx: RunContext = { cfg, workDir, releasesFile: metadata.releasesFile, lists };

    if (cfg.dryRun) {
      printPlan(ctx);
      return 0;
    }

    let totalTags = 0;
    for (const channel of CHANNELS) totalTags += lists.selections[channel].tags.length;
    if (totalTags === 0) {
      info('没有可同步的 release，退出');
      // 仍然刷新状态文件：否则看门狗按 mtime 判「同步停滞」
      writeStatusFiles(cfg, {
        result: 'waiting',
        detail: `没有满足 SINCE_TAG=${cfg.sinceTag || '（不限）'} 的 release，等待下一轮`,
        sinceTag: cfg.sinceTag,
        maxBytes: cfg.maxBytes,
      });
      return 0;
    }

    ensureDir(cfg.upgradeDest);
    downloadChannels(ctx);

    const coreOk = new Set<Channel>();
    const failed: string[] = [];
    const handle = (label: string, outcome: PublishOutcome): void => {
      if (outcome === 'published') return;
      if (outcome === 'noop') {
        info(`${label}：无新内容，跳过`);
        return;
      }
      warn(`${label} 发布未完成（保留旧内容）`);
      failed.push(label);
    };

    for (const channel of CHANNELS) {
      const publishCtx = { cfg, workDir, selection: lists.selections[channel] };
      const coreOutcome = publishCore(publishCtx, channel);
      if (coreOutcome === 'published') coreOk.add(channel);
      handle(`core/${channel}`, coreOutcome);
      handle(`app/${channel}`, publishApp(publishCtx, channel));
    }

    // 增量状态：只记录真正发布成功的通道
    for (const channel of CHANNELS) {
      if (!coreOk.has(channel)) continue;
      writeSyncState(ctx, channel);
      if (channel === 'nightly') {
        const head = lists.selections[channel].core[0];
        if (head) writePublishedAt(ctx, channel, head);
      }
    }
    // 最后才是 stable 别名（原子替换链接）
    linkAlias(cfg, 'stable/manifest.json', path.join(cfg.dest, 'manifest.json'));
    linkAlias(cfg, 'stable/manifest.json', path.join(cfg.upgradeDest, 'manifest.json'));
    linkAlias(cfg, 'stable/latest.json', path.join(cfg.upgradeDest, 'latest.json'));
    linkAlias(cfg, 'stable/magisk_update.json', path.join(cfg.upgradeDest, 'magisk_update.json'));

    // 老布局 / 看门狗兼容：$DEST/<zip> 必须能解析到实际文件（现放在 stable/ 下）
    linkLegacyCoreFiles(
      cfg,
      path.join(cfg.dest, 'stable', 'manifest.json'),
      cfg.dest,
    );

    if (failed.length > 0) {
      // 通道半发布/下载失败只影响该通道（旧内容保持），按「等待」上报：
      // 看门狗只在等待超过 24h 时才升级告警，不会把发布窗口刷成红灯。
      writeStatusFiles(cfg, {
        result: 'waiting',
        detail: `通道发布未完成（已保持旧内容）：${failed.join(' ')}`,
        sinceTag: cfg.sinceTag,
        maxBytes: cfg.maxBytes,
      });
    } else {
      writeStatusFiles(cfg, { result: 'ok', detail: '', sinceTag: cfg.sinceTag, maxBytes: cfg.maxBytes });
    }

    // 旧布局遗留在 DEST 根上的 zip 不删：老客户端缓存过的旧清单还指向它们。
    const legacyRoot = fs
      .readdirSync(cfg.dest, { withFileTypes: true })
      .filter((entry) => entry.isFile() && entry.name.endsWith('.zip')).length;
    if (legacyRoot > 0) {
      info(`注意：${cfg.dest} 根上仍有 ${legacyRoot} 个旧布局 zip（不自动清理，确认无老客户端后再手动删）`);
    }

    for (const channel of CHANNELS) {
      if (!isDir(path.join(cfg.dest, channel))) continue;
      const coreFiles = countTopLevelFiles(path.join(cfg.dest, channel));
      const appFiles = countTopLevelFiles(path.join(cfg.upgradeDest, channel));
      info(`${channel}：core ${coreFiles} 个文件 / app ${appFiles} 个文件`);
    }

    info(`完成：${cfg.dest} ${humanSize(dirSize(cfg.dest))}，${cfg.upgradeDest} ${humanSize(dirSize(cfg.upgradeDest))}`);
    return 0;
  } finally {
    cleanup();
  }
}

function countTopLevelFiles(dir: string): number {
  try {
    return fs
      .readdirSync(dir, { withFileTypes: true })
      .filter((entry) => entry.isFile() && !entry.name.startsWith('.')).length;
  } catch {
    return 0;
  }
}
