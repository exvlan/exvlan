/**
 * tag → 通道的归类、通道内 tag 选择（KEEP_<CHANNEL> / SINCE_TAG / MAX_BYTES 预算）。
 *
 * 三条硬规则的落点（改代码前先读 docs/decisions-7.3.md §14.13）：
 * - `SINCE_TAG` **只约束 stable**，其余通道按 `KEEP_<CHANNEL>` 从最新往前取；
 * - `MAX_BYTES` 是 core+app 的**窗口预算**：超限从最旧版本收缩，清单随之收缩；
 * - 空通道不建目录（`tags` 为空 → core/app/dl 都为空 → 发布阶段直接返回）。
 */

import fs from 'node:fs';
import type { Channel, Config } from './config';
import { CHANNELS, keepCountApp, keepCountCore } from './config';
import { info, warn } from './log';
import { assetTablePath } from './gh';
import { readTextOr } from './fsx';
import { versionCompare, versionGe } from './version';

export type ChannelOrSkip = Channel | 'skip';

export interface ChannelSelection {
  /** 通道内全部 tag（新 → 旧）。 */
  tags: string[];
  /** core 保留窗口。 */
  core: string[];
  /** app 保留窗口。 */
  app: string[];
  /** 下载集合 = core ∪ app（同一 tag 只下载一次）。 */
  dl: string[];
}

export interface ChannelLists {
  selections: Record<Channel, ChannelSelection>;
  /** 非通道 tag（legacy / 其它），dry-run 打印用。 */
  skipped: string[];
}

export function emptySelection(): ChannelSelection {
  return { tags: [], core: [], app: [], dl: [] };
}

/** tag → 通道（`nightly` 固定 tag / 后缀 / 纯 `vX.Y.Z`）。 */
export function classifyTag(tag: string): ChannelOrSkip {
  if (tag === 'nightly') return 'nightly';
  if (tag.includes('-pre.') || tag.startsWith('preview-')) return 'skip'; // legacy，不镜像
  if (tag.includes('-rc.')) return 'rc';
  if (tag.includes('-beta.')) return 'beta';
  if (tag.includes('-dev.')) return 'dev';
  if (tag.includes('-nightly.')) return 'nightly';
  if (/^v\d+\.\d+\.\d+$/.test(tag)) return 'stable';
  return 'skip';
}

interface ReleaseRow {
  tag: string;
  publishedAt: string;
  prerelease: string;
}

function readReleases(releasesFile: string): ReleaseRow[] {
  const rows: ReleaseRow[] = [];
  for (const line of readTextOr(releasesFile, '').split('\n')) {
    if (line === '') continue;
    const fields = line.split('\t');
    const tag = fields[0] ?? '';
    if (tag === '') continue;
    rows.push({ tag, publishedAt: fields[1] ?? '', prerelease: fields[2] ?? '' });
  }
  return rows;
}

/** `release_published_at`：releases.tsv 里该 tag 的第一行。 */
export function releasePublishedAt(releasesFile: string, tag: string): string {
  for (const row of readReleases(releasesFile)) {
    if (row.tag === tag) return row.publishedAt;
  }
  return '';
}

/** 按通道分桶 + 排序（stable/rc/beta/dev 按版本倒序；nightly 按发布时间倒序）。 */
export function buildChannelLists(releasesFile: string): ChannelLists {
  const buckets: Record<Channel, ReleaseRow[]> = {
    stable: [],
    rc: [],
    beta: [],
    dev: [],
    nightly: [],
  };
  const skipped: string[] = [];

  for (const row of readReleases(releasesFile)) {
    const channel = classifyTag(row.tag);
    if (channel === 'skip') {
      skipped.push(row.tag);
      continue;
    }
    // stable 沿用旧语义：标了 prerelease 的 vX.Y.Z 不算正式版
    if (channel === 'stable' && row.prerelease === 'true') {
      skipped.push(row.tag);
      continue;
    }
    buckets[channel].push(row);
  }

  const selections = {} as Record<Channel, ChannelSelection>;
  for (const channel of CHANNELS) {
    const rows = buckets[channel];
    let tags: string[] = [];
    if (rows.length > 0) {
      if (channel === 'nightly') {
        // 固定 tag：按发布时间倒序（同一 published_at 只留一行，等价 `sort -k2,2r -u`）
        const sorted = [...rows].sort((a, b) =>
          a.publishedAt === b.publishedAt ? 0 : a.publishedAt < b.publishedAt ? 1 : -1,
        );
        const seen = new Set<string>();
        for (const row of sorted) {
          if (seen.has(row.publishedAt)) continue;
          seen.add(row.publishedAt);
          tags.push(row.tag);
        }
      } else {
        const sorted = [...rows].sort((a, b) => versionCompare(b.tag, a.tag));
        const seen = new Set<string>();
        for (const row of sorted) {
          if (seen.has(row.tag)) continue;
          seen.add(row.tag);
          tags.push(row.tag);
        }
      }
    }
    tags = tags.filter((tag) => tag !== '');
    selections[channel] = { ...emptySelection(), tags };
  }

  return { selections, skipped };
}

function platformZipPattern(platforms: readonly string[]): RegExp {
  const escaped = platforms.map((p) => p.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
  return new RegExp(`^(${escaped.join('|')})-.*[.]zip$`);
}

// upgrade 目录收的资产（与 publish_app 的 case 保持同步；预算估算用）
const UPGRADE_ASSET_RE =
  /([.]dmg$|[.]msi$|-setup[.]exe$|app-.*-release.*[.]apk$|[.]app[.]tar[.]gz$|[.]sig$|^Exvlan-Magisk-.*[.]zip$)/;

/** 单 tag 的预计占用（内核 zip + upgrade 资产）；没有资产元数据时返回 null。 */
export function tagBytes(workDir: string, tag: string, platforms: readonly string[]): number | null {
  const file = assetTablePath(workDir, tag);
  let text: string;
  try {
    text = fs.readFileSync(file, 'utf8');
  } catch {
    return null;
  }
  if (text.length === 0) return null;
  const zipRe = platformZipPattern(platforms);
  let total = 0;
  for (const line of text.split('\n')) {
    if (line === '') continue;
    const fields = line.split('\t');
    const name = fields[0] ?? '';
    if (!(zipRe.test(name) || UPGRADE_ASSET_RE.test(name))) continue;
    const size = Number(fields[1] ?? '0');
    total += Number.isFinite(size) ? size : 0;
  }
  return total;
}

/**
 * MAX_BYTES 窗口预算：core+app 合计超限时从**最旧**版本开始收缩，
 * 合并出的清单同步收缩（不留「清单引用但文件已删」的不一致状态）。
 * 只裁到每边剩 1 个版本为止。
 */
function budgetTrim(cfg: Config, channel: Channel, selection: ChannelSelection, workDir: string): void {
  if (cfg.maxBytes <= 0) return;
  let dropped = 0;
  for (;;) {
    const union = [...new Set([...selection.core, ...selection.app])].sort((a, b) =>
      Buffer.compare(Buffer.from(a, 'utf8'), Buffer.from(b, 'utf8')),
    );
    if (union.length === 0) return;

    let total = 0;
    for (const tag of union) {
      const bytes = tagBytes(workDir, tag, cfg.platforms);
      // 没有资产元数据（--tags-file 模式）→ 放弃预算裁剪
      if (bytes === null) return;
      total += bytes;
    }
    if (total <= cfg.maxBytes) {
      if (dropped > 0) {
        info(
          `${channel}：MAX_BYTES=${cfg.maxBytes} 预算裁剪后保留 core ${selection.core.length} / app ${selection.app.length} 个版本（约 ${total} 字节）`,
        );
      }
      return;
    }

    const coreN = selection.core.length;
    const appN = selection.app.length;
    if (coreN <= 1 && appN <= 1) {
      warn(`${channel}：单个版本就超过 MAX_BYTES=${cfg.maxBytes}（约 ${total} 字节），不再裁剪`);
      return;
    }
    if (coreN >= appN && coreN > 1) {
      info(`${channel}：超出 MAX_BYTES=${cfg.maxBytes}，从最旧版本收缩 core 窗口：${selection.core[coreN - 1]}`);
      selection.core = selection.core.slice(0, coreN - 1);
    } else {
      info(`${channel}：超出 MAX_BYTES=${cfg.maxBytes}，从最旧版本收缩 app 窗口：${selection.app[appN - 1]}`);
      selection.app = selection.app.slice(0, appN - 1);
    }
    dropped += 1;
  }
}

/** 每个通道算 core / app 保留窗口（含 SINCE_TAG 与预算裁剪）。 */
export function selectTags(cfg: Config, channel: Channel, selection: ChannelSelection, workDir: string): ChannelSelection {
  const result: ChannelSelection = { ...selection, core: [], app: [], dl: [] };
  if (selection.tags.length === 0) return result;

  let tags = selection.tags;
  if (channel === 'stable' && cfg.sinceTag) {
    tags = tags.filter((tag) => versionGe(tag, cfg.sinceTag));
    if (tags.length === 0) {
      warn(`stable：>= SINCE_TAG=${cfg.sinceTag} 没有 release，跳过该通道`);
      return result;
    }
  }

  result.core = tags.slice(0, keepCountCore(cfg, channel));
  result.app = tags.slice(0, keepCountApp(cfg, channel));

  budgetTrim(cfg, channel, result, workDir);

  // 下载集合 = core 保留 ∪ app 保留（同一 tag 只下载一次）
  const dl = [...result.core];
  for (const tag of result.app) {
    if (!dl.includes(tag)) dl.push(tag);
  }
  result.dl = dl;
  return result;
}
