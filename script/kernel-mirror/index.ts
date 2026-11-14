#!/usr/bin/env bun
/**
 * EXVlan 镜像同步入口（Bun）。
 *
 * 取代 `exvlan-kernel-sync.sh` + `merge_manifests.py` + `filter_manifest_platforms.py`
 * + `channelize_urls.py` + `mirror_status.py`（那五个文件保留一个周期并标注 legacy）。
 *
 * **入口兼容**：CLI 参数（`--dry-run` / `--tags-file`）与环境变量（`REPO` / `DEST` /
 * `UPGRADE_DEST` / `SINCE_TAG` / `KEEP_*` / `MAX_BYTES` / `PLATFORMS` / `FORCE` /
 * `MIRROR_STATUS*` / `MIRROR_REF`）与旧脚本完全一致 —— 发布机换镜像时不能静默跑错。
 *
 * 用法：
 *   GH_TOKEN=xxx bun script/kernel-mirror/index.ts
 *   bun script/kernel-mirror/index.ts --dry-run --tags-file /tmp/tags
 */

import { resolveConfig } from './src/config';
import { SyncError, warn } from './src/log';
import { runOnce } from './src/run';

function main(argv: readonly string[]): number {
  try {
    const cfg = resolveConfig(argv);
    if (!cfg) return 0;
    return runOnce(cfg);
  } catch (error) {
    if (error instanceof SyncError) return 1;
    const detail = error instanceof Error ? (error.stack ?? error.message) : String(error);
    warn(`未捕获错误：${detail}`);
    return 1;
  }
}

process.exit(main(process.argv.slice(2)));
