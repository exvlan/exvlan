/**
 * 发布（原脚本 `publish_core` / `publish_app`）。
 *
 * 不变式（改这里之前先读 docs/decisions-7.3.md §14.13）：
 * - **先资产、后清单**：清单落盘时它引用的文件一定已经在目录里；
 * - **通道独立**：某通道失败只留旧内容，不动其它通道；
 * - **未版本化资产只由最新 tag 发布**（同名跨版本重复，旧 tag 后写赢会让 sha256 对不上）；
 * - **别名是符号链接**，指向 `stable/`，原子替换，不复制文件。
 */

import fs from 'node:fs';
import path from 'node:path';
import type { Channel, Config } from './config';
import { keepCountApp, keepCountCore } from './config';
import { info, warn } from './log';
import { channelizeFile } from './channelize';
import { copyFile, ensureDir, isDir, isFile, listEntries, readTextOr, rmrf, sha256Hex, wipeDirFiles } from './fsx';
import {
  filterPlatforms,
  jsonUrlNames,
  jsonVersion,
  manifestFiles,
  mergeManifests,
  type JsonObject,
} from './manifest';
import type { ChannelSelection } from './tags';
import { versionGe } from './version';

export type PublishOutcome = 'published' | 'noop' | 'failed';

export interface PublishContext {
  cfg: Config;
  workDir: string;
  selection: ChannelSelection;
}

/** 保留增量状态的点文件（nightly 整目录替换时不能删）。 */
const STATE_FILES = ['.sync-state', '.published-at'];

function relDir(ctx: PublishContext, channel: Channel, tag: string): string {
  return path.join(ctx.workDir, 'rel', channel, tag);
}

/** `app_refs`：清单引用 + latest/magisk 引用，各自再带上成对的 `.sig`。 */
function appRefs(dir: string, manifestFile: string): Set<string> {
  const refs = new Set<string>();
  const push = (name: string): void => {
    if (!name) return;
    refs.add(name);
    refs.add(`${name}.sig`);
  };
  for (const name of manifestFiles(readTextOr(manifestFile, ''))) push(name);
  const latestFile = path.join(dir, 'latest.json');
  if (isFile(latestFile)) for (const name of jsonUrlNames(readTextOr(latestFile, ''), 'url')) push(name);
  const magiskFile = path.join(dir, 'magisk_update.json');
  if (isFile(magiskFile)) for (const name of jsonUrlNames(readTextOr(magiskFile, ''), 'zipUrl')) push(name);
  return refs;
}

/** 按清单引用清理（`*.json` 与点文件不动）；引用为空时跳过（避免误删）。 */
function pruneDir(dir: string, manifestFile: string, kind: 'core' | 'app'): void {
  const refs = kind === 'core' ? new Set(manifestFiles(readTextOr(manifestFile, ''))) : appRefs(dir, manifestFile);
  if (refs.size === 0) {
    warn(`prune_dir：${manifestFile} 里没有任何引用，跳过清理（避免误删）`);
    return;
  }
  for (const name of listEntries(dir)) {
    if (name.startsWith('.') || name.endsWith('.json')) continue;
    if (refs.has(name)) continue;
    const target = path.join(dir, name);
    info(`清理未引用文件：${target}`);
    fs.rmSync(target, { force: true });
  }
}

/** 可选校验：清单声明的 sha256 与实际文件是否一致（`VERIFY_SHA256=1`）。 */
function verifyDeclaredSha256(cfg: Config, channel: Channel, manifestFile: string, dirs: string[]): void {
  if (!cfg.verifySha256) return;
  let data: JsonObject;
  try {
    data = JSON.parse(readTextOr(manifestFile, '{}')) as JsonObject;
  } catch (error) {
    warn(
      `${channel}：清单无法解析，跳过 sha256 校验（${error instanceof Error ? error.message : String(error)}）`,
    );
    return;
  }
  const versions = Array.isArray(data.versions) ? data.versions : [];
  for (const entry of versions) {
    if (entry === null || typeof entry !== 'object' || Array.isArray(entry)) continue;
    const assets = (entry as JsonObject).assets;
    if (assets === null || typeof assets !== 'object' || Array.isArray(assets)) continue;
    for (const meta of Object.values(assets as JsonObject)) {
      if (meta === null || typeof meta !== 'object' || Array.isArray(meta)) continue;
      const file = (meta as JsonObject).file;
      const expected = (meta as JsonObject).sha256;
      if (typeof file !== 'string' || typeof expected !== 'string') continue;
      const source = dirs.map((dir) => path.join(dir, file)).find((candidate) => isFile(candidate));
      if (!source) continue;
      const actual = sha256Hex(fs.readFileSync(source));
      if (actual !== expected) {
        warn(`${channel}：${file} 的 sha256 与清单不符（清单 ${expected} / 实际 ${actual}）`);
      }
    }
  }
}

/** 发布内核通道。返回 published/noop/failed。 */
export function publishCore(ctx: PublishContext, channel: Channel): PublishOutcome {
  const { cfg } = ctx;
  const dir = path.join(cfg.dest, channel);
  const stage = path.join(cfg.dest, `.staging-core.${channel}`);
  const keep = keepCountCore(cfg, channel);
  const since = channel === 'stable' && cfg.sinceTag ? cfg.sinceTag : '';

  rmrf(stage);
  ensureDir(stage);

  let manifests: string[] = [];
  let count = 0;

  for (const tag of ctx.selection.core) {
    const rel = relDir(ctx, channel, tag);
    // 没有 rel 目录 = 本轮因指纹命中跳过下载（资产已在通道目录里），静默跳过
    if (!isDir(rel)) continue;
    if (!isFile(path.join(rel, 'manifest.json'))) {
      warn(`core/${channel}：${tag} 缺 manifest.json（半发布？），本轮不发布该版本`);
      continue;
    }
    let found = false;
    for (const name of listEntries(rel)) {
      if (!name.endsWith('.zip')) continue;
      if (name.startsWith('Exvlan-Magisk-')) continue; // Magisk 包属于 upgrade
      copyFile(path.join(rel, name), path.join(stage, name));
      found = true;
    }
    if (!found) {
      warn(`core/${channel}：${tag} 没有可用内核 zip，本轮不发布该版本`);
      continue;
    }
    manifests.push(path.join(rel, 'manifest.json'));
    count += 1;
  }

  const publishedManifest = path.join(dir, 'manifest.json');
  if (count === 0) {
    // 没有新版本时一般不重写清单（省事且安全）；但若 stable 设了 SINCE_TAG，
    // 需要把现有清单里的更早版本也剔掉，所以仍要重写一次。
    if (!since || !isFile(publishedManifest)) {
      info(`core/${channel}：本轮没有可发布的内核包（保持现状）`);
      rmrf(stage);
      return 'noop';
    }
    info(`core/${channel}：没有新版本，但 SINCE_TAG 要求重写清单`);
    manifests = [publishedManifest];
  }

  // 保留策略靠清单裁剪实现：本轮重新下载的版本在前，通道现有清单补足其余版本，
  // 这样「增量跳过」的旧版本不会从清单里消失。
  if (count > 0 && channel !== 'nightly' && isFile(publishedManifest)) {
    manifests = [...manifests, publishedManifest];
  }

  const stageManifest = path.join(stage, 'manifest.json');
  if (!mergeManifests(manifests, stageManifest, { keep, since })) {
    rmrf(stage);
    return 'failed';
  }
  // Release 自带 manifest 含全部 14 个平台，镜像只托管 PLATFORMS：
  // 不裁剪会让 GUI 看到「有更新」但下载 404。
  filterPlatforms(stageManifest, cfg.platforms);

  let missing = false;
  for (const name of manifestFiles(readTextOr(stageManifest, ''))) {
    if (!name) continue;
    if (!isFile(path.join(stage, name)) && !isFile(path.join(dir, name))) {
      warn(`core/${channel}：清单引用的 ${name} 不存在（半发布防御：保持现状）`);
      missing = true;
    }
  }
  if (missing) {
    rmrf(stage);
    return 'failed';
  }

  verifyDeclaredSha256(cfg, channel, stageManifest, [stage, dir]);

  ensureDir(dir);
  if (channel === 'nightly') wipeDirFiles(dir, STATE_FILES);

  for (const name of listEntries(stage)) {
    if (!name.endsWith('.zip')) continue;
    fs.renameSync(path.join(stage, name), path.join(dir, name));
  }
  fs.renameSync(stageManifest, publishedManifest);
  rmrf(stage);
  info(`core/${channel}：已发布清单（新版本 ${count} 个）→ ${dir}`);
  pruneDir(dir, publishedManifest, 'core');
  return 'published';
}

/** 未版本化资产名里是否带该 tag 的版本号（带 = 各版本名字天然不同，谁都能发布）。 */
function nameScopedToTag(name: string, tag: string): boolean {
  const version = tag.replace(/^v/, '');
  if (!version) return false;
  return name.includes(version);
}

function tagMatchesVersion(tag: string, version: string): boolean {
  if (!version) return false;
  if (tag === 'nightly') return true;
  return tag.replace(/^v/, '') === version;
}

function tagNotOlderThan(tag: string, version: string): boolean {
  if (!version) return true;
  if (tag === 'nightly') return true;
  return versionGe(tag.replace(/^v/, ''), version);
}

interface ManifestPick {
  tag: string;
  src: 'rel' | 'existing';
}

/**
 * 选「该通道最新的、且清单可用的 tag」——增量同步下不能只从「本轮下载过的 tag」里挑，
 * 否则只补下旧 tag 时 latest/magisk 会被降级并一直粘住。
 */
function pickManifestTag(
  channel: Channel,
  tags: readonly string[],
  name: string,
  publishedFile: string,
  workDir: string,
): ManifestPick | null {
  if (tags.length === 0) return null;
  let publishedVersion = '';
  if (isFile(publishedFile)) publishedVersion = jsonVersion(readTextOr(publishedFile, ''));

  for (const tag of tags) {
    if (!tag) continue;
    const rel = path.join(workDir, 'rel', channel, tag);
    if (isFile(path.join(rel, name))) {
      if (tagNotOlderThan(tag, publishedVersion)) return { tag, src: 'rel' };
      warn(`app/${channel}：${tag} 的 ${name} 比已发布的 ${publishedVersion} 旧，忽略（不降级）`);
      continue;
    }
    if (tagMatchesVersion(tag, publishedVersion)) {
      // 最新版这轮因指纹命中没重下：目标目录里已有的清单就是它的内容
      return { tag, src: 'existing' };
    }
  }
  return null;
}

/** upgrade 目录收的资产类型（与下载 pattern 保持同步）。 */
function isUpgradeAsset(name: string): boolean {
  return (
    name.endsWith('.dmg') ||
    name.endsWith('.msi') ||
    name.endsWith('-setup.exe') ||
    /^app-.*-release.*\.apk$/.test(name) ||
    name.endsWith('.app.tar.gz') ||
    name.endsWith('.sig') ||
    /^Exvlan-Magisk-.*\.zip$/.test(name)
  );
}

/** 发布应用/Magisk 通道。返回 published/noop/failed。 */
export function publishApp(ctx: PublishContext, channel: Channel): PublishOutcome {
  const { cfg } = ctx;
  const dir = path.join(cfg.upgradeDest, channel);
  const stage = path.join(cfg.upgradeDest, `.staging-app.${channel}`);
  const keep = keepCountApp(cfg, channel);
  const since = channel === 'stable' && cfg.sinceTag ? cfg.sinceTag : '';
  // 通道最新 tag：未版本化资产只由它提供（app 窗口已按版本倒序）
  const newestTag = ctx.selection.app[0] ?? '';

  rmrf(stage);
  ensureDir(stage);

  let manifests: string[] = [];
  let copied = 0;
  let skipped = 0;
  let missing = false;

  for (const tag of ctx.selection.app) {
    const rel = relDir(ctx, channel, tag);
    // 没有 rel 目录 = 本轮因指纹命中跳过下载，静默跳过
    if (!isDir(rel)) continue;
    if (!isFile(path.join(rel, 'upgrade-manifest.json'))) continue;
    manifests.push(path.join(rel, 'upgrade-manifest.json'));
    for (const asset of listEntries(rel)) {
      if (asset.endsWith('.json')) continue;
      if (!isUpgradeAsset(asset)) continue;
      // 未版本化资产跨版本同名：旧 tag 的副本会覆盖最新版字节（客户端 sha256 校验失败）
      if (tag !== newestTag && !nameScopedToTag(asset, tag)) {
        info(`app/${channel}：跳过 ${tag} 的未版本化资产 ${asset}（只从最新 tag ${newestTag || '?'} 发布）`);
        skipped += 1;
        continue;
      }
      copyFile(path.join(rel, asset), path.join(stage, asset));
      copied += 1;
    }
  }

  const publishedManifest = path.join(dir, 'manifest.json');
  if (manifests.length === 0) {
    // 没有新版本时一般不重写清单；但 stable 设了 SINCE_TAG 时要顺带剔除更早的版本
    if (!since || !isFile(publishedManifest)) {
      info(`app/${channel}：本轮没有应用更新清单（保持现状）`);
      rmrf(stage);
      return 'noop';
    }
    info(`app/${channel}：没有新版本，但 SINCE_TAG 要求重写清单`);
    manifests = [publishedManifest];
  } else if (channel !== 'nightly' && isFile(publishedManifest)) {
    // 与 core 同理：现有清单补足「本轮跳过下载」的旧版本，再按 KEEP 裁剪
    manifests = [...manifests, publishedManifest];
  }

  const stageManifest = path.join(stage, 'manifest.json');
  if (!mergeManifests(manifests, stageManifest, { keep, since })) {
    rmrf(stage);
    return 'failed';
  }

  // 官方 updater 清单与 Magisk 清单取「该通道最新的、且清单可用的 tag」：
  // 本轮下载到的直接落盘；本轮因指纹命中没重下的最新版，目标目录里的清单就是它，
  // 保持不动（只补下旧 tag 时不会把 latest 降级，也不会粘在旧版本上）。
  const latest = pickManifestTag(channel, ctx.selection.tags, 'upgrade-latest.json', path.join(dir, 'latest.json'), ctx.workDir);
  if (latest) {
    if (latest.src === 'rel') {
      copyFile(path.join(ctx.workDir, 'rel', channel, latest.tag, 'upgrade-latest.json'), path.join(stage, 'latest.json'));
      // URL 改写成**本通道**目录（绝对地址是 tauri updater 的硬要求）
      channelizeFile(path.join(stage, 'latest.json'), channel);
    } else {
      info(`app/${channel}：latest.json 已是 ${latest.tag} 的内容（本轮未重下最新版），保持不动`);
    }
  } else {
    warn(`app/${channel}：找不到可用的 upgrade-latest.json，官方 updater 清单保持旧内容`);
  }

  const magisk = pickManifestTag(
    channel,
    ctx.selection.tags,
    'magisk_update.json',
    path.join(dir, 'magisk_update.json'),
    ctx.workDir,
  );
  if (magisk) {
    if (magisk.src === 'rel') {
      copyFile(
        path.join(ctx.workDir, 'rel', channel, magisk.tag, 'magisk_update.json'),
        path.join(stage, 'magisk_update.json'),
      );
      channelizeFile(path.join(stage, 'magisk_update.json'), channel);
    } else {
      info(`app/${channel}：magisk_update.json 已是 ${magisk.tag} 的内容（本轮未重下最新版），保持不动`);
    }
  }

  const stageLatest = path.join(stage, 'latest.json');
  const stageMagisk = path.join(stage, 'magisk_update.json');
  for (const name of manifestFiles(readTextOr(stageManifest, ''))) {
    if (!name) continue;
    if (!isFile(path.join(stage, name)) && !isFile(path.join(dir, name))) {
      warn(`app/${channel}：清单引用的 ${name} 不存在（半发布防御：保持现状）`);
      missing = true;
    }
  }
  if (isFile(stageLatest)) {
    for (const name of jsonUrlNames(readTextOr(stageLatest, ''), 'url')) {
      if (!name) continue;
      if (!isFile(path.join(stage, name)) && !isFile(path.join(dir, name))) {
        warn(`app/${channel}：latest.json 引用的 ${name} 不存在（半发布防御：保持现状）`);
        missing = true;
      }
    }
  }
  if (isFile(stageMagisk)) {
    for (const name of jsonUrlNames(readTextOr(stageMagisk, ''), 'zipUrl')) {
      if (!name) continue;
      if (!isFile(path.join(stage, name)) && !isFile(path.join(dir, name))) {
        warn(`app/${channel}：magisk_update.json 引用的 ${name} 不存在（半发布防御：保持现状）`);
        missing = true;
      }
    }
  }
  if (missing) {
    rmrf(stage);
    return 'failed';
  }

  ensureDir(dir);
  if (channel === 'nightly') wipeDirFiles(dir, STATE_FILES);

  // 先资产、后清单
  for (const name of listEntries(stage)) {
    if (name.endsWith('.json')) continue;
    fs.renameSync(path.join(stage, name), path.join(dir, name));
  }
  for (const name of listEntries(stage)) {
    if (!name.endsWith('.json')) continue;
    fs.renameSync(path.join(stage, name), path.join(dir, name));
  }
  rmrf(stage);
  const skipNote = skipped > 0 ? `（跳过 ${skipped} 个旧 tag 同名未版本化资产）` : '';
  info(`app/${channel}：已发布 ${copied} 个资产 + 清单 → ${dir}${skipNote}`);
  pruneDir(dir, publishedManifest, 'app');
  return 'published';
}
