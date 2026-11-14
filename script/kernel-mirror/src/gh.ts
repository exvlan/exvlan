/**
 * GitHub Release 元数据与资产下载。
 *
 * 仍然走 `gh` CLI（**不**改成 HTTP API）：凭据语义不同 —— 容器/unit 里用的是
 * `GH_TOKEN` + `~/.config/gh`，改成裸 HTTP 会把 token 读取、企业域名、代理这些
 * 交给脚本自己实现，风险大于收益。离线 e2e 用假 `gh` 注入（同旧脚本）。
 */

import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import type { Config } from './config';
import { die, warn } from './log';
import { ensureDir, writeFileAtomic } from './fsx';

/** gh api 的 jq 表达式：R = release 行，A = 资产行。与旧脚本逐字一致。 */
const JQ_EXPR = `
    .[] | select(.draft == false) | . as $r
    | (["R", $r.tag_name, ($r.published_at // ""), ($r.prerelease | tostring)] | @tsv),
      (($r.assets // [])[] | ["A", $r.tag_name, .name, (.size | tostring)] | @tsv)
  `;

export interface MetadataFiles {
  releasesFile: string;
  workDir: string;
}

export function assetTablePath(workDir: string, tag: string): string {
  return path.join(workDir, `assets.${tag}`);
}

function splitTab(line: string): string[] {
  return line.split('\t');
}

/**
 * 取 release 元数据：`<work>/releases.tsv`（tag \t published_at \t prerelease）
 * 与 `<work>/assets.<tag>`（资产名 \t size）。`--tags-file` 模式只有前者。
 */
export function collectReleaseMetadata(cfg: Config, workDir: string): MetadataFiles {
  const releasesFile = path.join(workDir, 'releases.tsv');
  ensureDir(workDir);

  if (cfg.tagsFile) {
    let text: string;
    try {
      text = fs.readFileSync(cfg.tagsFile, 'utf8');
    } catch {
      die(`读不到 --tags-file：${cfg.tagsFile}`);
    }
    const rows: string[] = [];
    for (const rawLine of text.split('\n')) {
      // 旧 awk：先整行去 `#` 注释，再取前两列并 trim
      const line = rawLine.replace(/#.*/, '');
      const fields = splitTab(line);
      const tag = (fields[0] ?? '').trim();
      if (tag === '') continue;
      const publishedAt = fields.length > 1 ? (fields[1] ?? '').trim() : '';
      rows.push(`${tag}\t${publishedAt}\tfalse`);
    }
    fs.writeFileSync(releasesFile, rows.length === 0 ? '' : `${rows.join('\n')}\n`);
    return { releasesFile, workDir };
  }

  const result = spawnSync(
    'gh',
    ['api', `repos/${cfg.repo}/releases?per_page=100`, '--paginate', '--jq', JQ_EXPR],
    { encoding: 'utf8', maxBuffer: 256 * 1024 * 1024, stdio: ['ignore', 'pipe', 'inherit'] },
  );
  if (result.error) {
    die(`调用 gh 失败：${result.error.message}`);
  }
  if (result.status !== 0) {
    die(`gh api 取 release 列表失败（退出码 ${result.status ?? '?'}）`);
  }

  const releaseRows: string[] = [];
  const assetRows = new Map<string, string[]>();
  for (const line of (result.stdout ?? '').split('\n')) {
    if (line === '') continue;
    const fields = splitTab(line);
    if (fields[0] === 'R') {
      releaseRows.push(`${fields[1] ?? ''}\t${fields[2] ?? ''}\t${fields[3] ?? ''}`);
    } else if (fields[0] === 'A') {
      const tag = fields[1] ?? '';
      const row = `${fields[2] ?? ''}\t${fields[3] ?? ''}`;
      const bucket = assetRows.get(tag);
      if (bucket) bucket.push(row);
      else assetRows.set(tag, [row]);
    }
  }

  fs.writeFileSync(releasesFile, releaseRows.length === 0 ? '' : `${releaseRows.join('\n')}\n`);
  for (const [tag, rows] of assetRows) {
    writeFileAtomic(assetTablePath(workDir, tag), `${rows.join('\n')}\n`);
  }
  return { releasesFile, workDir };
}

/** `gh release download` 的 `-p` 参数（顺序与旧脚本一致）。 */
export function downloadPatterns(cfg: Config): string[] {
  const patterns: string[] = [];
  for (const platform of cfg.platforms) {
    patterns.push(`${platform}-v*.zip`);
    // nightly 用固定 tag 构建时，产物名可能是 `<platform>-<ver>-nightly.N.zip`（无 v 前缀）
    patterns.push(`${platform}-*nightly*.zip`);
  }
  patterns.push('manifest.json');
  // 应用更新：安装包 + 官方 updater 产物（.app.tar.gz / setup.exe / .sig）+ 清单。
  // APK 文件名允许带版本后缀（nightly 会写 `app-arm64-release-<ver>.apk`：镜像是
  // immutable 缓存，同名覆盖会让客户端永远拿到旧包）。
  patterns.push('*.dmg', '*.msi', '*-setup.exe', 'app-*-release*.apk');
  patterns.push('*.app.tar.gz', '*.sig');
  patterns.push('upgrade-manifest.json', 'upgrade-latest.json');
  // Magisk 在线更新：模块包 + 清单（module.prop 的 updateJson 指向镜像）
  patterns.push('Exvlan-Magisk-*.zip', 'magisk_update.json');
  return patterns;
}

/** 下载一个 tag 的全部匹配资产到 `dir`；失败返回 false（调用方跳过该 tag）。 */
export function downloadRelease(cfg: Config, tag: string, dir: string): boolean {
  const args = ['release', 'download', tag, '--repo', cfg.repo];
  for (const pattern of downloadPatterns(cfg)) args.push('-p', pattern);
  args.push('--dir', dir, '--clobber');

  const result = spawnSync('gh', args, { stdio: 'inherit' });
  if (result.error) {
    warn(`gh release download ${tag} 启动失败：${result.error.message}`);
    return false;
  }
  return result.status === 0;
}
