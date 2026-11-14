/** 文件系统助手：替代旧脚本里的 `install -d` / `mv -fT` / `ln -sfn` / `du -sb` / `sort` / `sha256sum`。 */

import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

export function ensureDir(dir: string): void {
  fs.mkdirSync(dir, { recursive: true });
}

export function rmrf(target: string): void {
  fs.rmSync(target, { recursive: true, force: true });
}

export function readTextOr(file: string, fallback: string): string {
  try {
    return fs.readFileSync(file, 'utf8');
  } catch {
    return fallback;
  }
}

/** `[ -f x ]`（跟随符号链接）。 */
export function isFile(file: string): boolean {
  try {
    return fs.statSync(file).isFile();
  } catch {
    return false;
  }
}

/** `[ -d x ]`（跟随符号链接）。 */
export function isDir(dir: string): boolean {
  try {
    return fs.statSync(dir).isDirectory();
  } catch {
    return false;
  }
}

/** `[ -e x ]`（跟随符号链接；悬空链接视为不存在，与 shell 一致）。 */
export function exists(file: string): boolean {
  try {
    fs.statSync(file);
    return true;
  } catch {
    return false;
  }
}

export function lstatOrNull(target: string): fs.Stats | null {
  try {
    return fs.lstatSync(target);
  } catch {
    return null;
  }
}

/** 目录项（默认跳过点文件，等价 shell 的 `dir/*` 展开）。 */
export function listEntries(dir: string, includeHidden = false): string[] {
  let names: string[];
  try {
    names = fs.readdirSync(dir);
  } catch {
    return [];
  }
  const filtered = includeHidden ? names : names.filter((name) => !name.startsWith('.'));
  return filtered.sort();
}

/** 原子写：先写 `<tmp>` 再 rename（同目录，跨不了设备，保证读者看不到半个文件）。 */
export function writeFileAtomic(file: string, content: string, tmpFile?: string): void {
  const tmp = tmpFile ?? `${file}.tmp`;
  ensureDir(path.dirname(tmp));
  fs.writeFileSync(tmp, content);
  fs.renameSync(tmp, file);
}

export function copyFile(src: string, dest: string): void {
  fs.copyFileSync(src, dest);
}

/**
 * 原子替换符号链接：`ln -sfn target tmp && mv -fT tmp dest`。
 * `target` 按**相对**写法保存（镜像里是 `stable/manifest.json`）。
 */
export function symlinkAtomic(target: string, dest: string): void {
  const parent = path.dirname(dest);
  const tmp = path.join(parent, `.${path.basename(dest)}.tmp.${process.pid}`);
  fs.rmSync(tmp, { force: true });
  fs.symlinkSync(target, tmp);
  fs.renameSync(tmp, dest);
}

/** `ln -sfn`（仅在目标不存在时调用；已存在则跳过由调用方判断）。 */
export function symlinkRelative(target: string, dest: string): void {
  fs.symlinkSync(target, dest);
}

/** nightly 整目录替换：保留增量状态点文件，其余（文件与目录）全删。 */
export function wipeDirFiles(dir: string, keep: readonly string[]): void {
  if (!isDir(dir)) return;
  const keepSet = new Set(keep);
  for (const name of fs.readdirSync(dir)) {
    if (keepSet.has(name)) continue;
    fs.rmSync(path.join(dir, name), { recursive: true, force: true });
  }
}

/**
 * `du -sb DIR`：**表观大小**合计 —— 普通文件算 st_size、符号链接算目标字符串长度，
 * **不算目录自身**（GNU du 的实测语义，用 `du -sb` 对照验证过）。
 * 状态文件里的 `bytes.*` 由它算出，客户端/看门狗会跨版本比对，不能换成块大小。
 */
export function apparentSize(target: string): number {
  const stat = lstatOrNull(target);
  if (!stat) return 0;
  if (!stat.isDirectory()) return stat.size;
  let total = 0;
  {
    let names: string[];
    try {
      names = fs.readdirSync(target);
    } catch {
      return total;
    }
    for (const name of names) {
      total += apparentSize(path.join(target, name));
    }
  }
  return total;
}

export function sha256Hex(data: string | Uint8Array): string {
  return createHash('sha256').update(data).digest('hex');
}

/** C locale 的 `sort`（逐字节比较整行）。 */
export function sortLinesC(lines: readonly string[]): string[] {
  return [...lines].sort((a, b) => Buffer.compare(Buffer.from(a, 'utf8'), Buffer.from(b, 'utf8')));
}

/** 供测试使用：不落盘的等价函数，返回 null 表示「无资产元数据」（旧脚本返回 1）。 */
export function fingerprintOfAssetTable(file: string): string | null {
  let text: string;
  try {
    text = fs.readFileSync(file, 'utf8');
  } catch {
    return null;
  }
  if (text.length === 0) return null;
  const lines = text.split('\n');
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop();
  const sorted = sortLinesC(lines);
  const body = sorted.length === 0 ? '' : `${sorted.join('\n')}\n`;
  return sha256Hex(body);
}
