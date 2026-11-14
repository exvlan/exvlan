/**
 * 命令行与环境变量解析。
 *
 * **入口兼容**是硬约束：发布机/unit 里写的还是旧脚本的参数与变量名
 * （`SINCE_TAG` / `KEEP_*` / `MAX_BYTES` / `MIRROR_REF` 等），换成 Bun 入口后
 * 必须一字不改地继续生效，否则容器换镜像时会静默跑错（同步到别的目录、丢掉保留策略）。
 */

import path from 'node:path';
import { die } from './log';

export interface Config {
  repo: string;
  dest: string;
  upgradeDest: string;
  platforms: string[];
  force: boolean;
  keepCoreStable: number;
  keepAppStable: number;
  keepRc: number;
  keepBeta: number;
  keepDev: number;
  sinceTag: string;
  maxBytes: number;
  mirrorStatus: string;
  mirrorStatusCompat: string;
  dryRun: boolean;
  tagsFile: string | null;
  verifySha256: boolean;
  /** 可选：跨进程互斥锁文件（trigger 服务与定时器共存时用；不设置 = 不锁，行为同旧脚本）。 */
  syncLock: string | null;
}

export const CHANNELS = ['stable', 'rc', 'beta', 'dev', 'nightly'] as const;
export type Channel = (typeof CHANNELS)[number];

export const DEFAULT_PLATFORMS =
  'macos-x86_64 macos-aarch64 windows-x86_64 windows-arm64 linux-x86_64 linux-aarch64';

export const USAGE = `按通道同步 EXVlan 的 Release 资产到镜像目录（docs/decisions-7.3.md §14.13）。

用法：
  bun script/kernel-mirror/index.ts [--dry-run] [--tags-file <path>]

选项：
  --dry-run            只打印「每个通道会同步哪些 tag、会产出哪些路径」，不下载、不写盘
  --tags-file <path>   从文件读 tag 列表（每行一个，可附带 <tab>published_at），跳过 gh api（测试用）
  -h, --help           显示本帮助

环境变量：
  REPO=exvlan/exvlan                  Release 所在仓库
  DEST=/srv/exvlan/core               内核镜像根目录（其下按通道建子目录）
  UPGRADE_DEST=/srv/exvlan/upgrade    应用更新镜像根目录（其下按通道建子目录）
  MIRROR_STATUS=$DEST/mirror-status.json  状态文件（新 schema，按通道）
  MIRROR_STATUS_COMPAT=\${DEST%/core}/mirror-status.json
                                      旧 schema 兼容状态（发布机看门狗读它）
  MAX_BYTES=8589934592                core+app 合计预算；超限从最旧版本开始收缩窗口（0 = 不限）
  KEEP_CORE_STABLE=5  KEEP_APP_STABLE=5   stable 通道保留的版本数
  KEEP_RC=3  KEEP_BETA=3  KEEP_DEV=3      预览通道保留的版本数（nightly 固定 1）
  SINCE_TAG=                          只同步 >= 该 tag 的 stable 版本（其余通道不受影响）
  PLATFORMS="..."                     镜像的平台（默认 6 个 GUI 平台）
  FORCE=1                             忽略增量指纹，强制重新下载
  VERIFY_SHA256=1                     对清单里声明的 sha256 做校验（不一致只告警，不改变发布结果）
  MIRROR_SYNC_LOCK=/path/to.lock      跨进程互斥（与 trigger 服务/定时器共存时设置）
`;

export interface ParsedArgv {
  dryRun: boolean;
  tagsFile: string | null;
  help: boolean;
}

export function parseArgv(argv: readonly string[]): ParsedArgv {
  let dryRun = false;
  let tagsFile: string | null = null;
  let i = 0;
  while (i < argv.length) {
    const arg = argv[i];
    if (arg === '--dry-run') {
      dryRun = true;
      i += 1;
    } else if (arg === '--tags-file') {
      if (i + 1 >= argv.length) die('--tags-file 需要路径');
      tagsFile = argv[i + 1];
      i += 2;
    } else if (arg.startsWith('--tags-file=')) {
      tagsFile = arg.slice('--tags-file='.length);
      i += 1;
    } else if (arg === '-h' || arg === '--help') {
      return { dryRun, tagsFile, help: true };
    } else {
      process.stderr.write(`${USAGE}\n`);
      die(`未知参数：${arg}`);
    }
  }
  return { dryRun, tagsFile, help: false };
}

function envString(env: NodeJS.ProcessEnv, name: string, fallback: string): string {
  const value = env[name];
  return value === undefined || value === '' ? fallback : value;
}

function envNumber(env: NodeJS.ProcessEnv, name: string, fallback: number): number {
  const raw = env[name];
  if (raw === undefined || raw === '') return fallback;
  const parsed = Number(raw);
  if (!Number.isFinite(parsed) || !Number.isInteger(parsed) || parsed < 0) {
    die(`${name} 需要非负整数，收到 ${JSON.stringify(raw)}`);
  }
  return parsed;
}

export function resolveConfig(argv: readonly string[], env: NodeJS.ProcessEnv = process.env): Config | null {
  const parsed = parseArgv(argv);
  if (parsed.help) {
    process.stdout.write(`${USAGE}\n`);
    return null;
  }

  const dest = envString(env, 'DEST', '/srv/exvlan/core');
  const upgradeDest = envString(env, 'UPGRADE_DEST', '/srv/exvlan/upgrade');
  // 兼容旧的单个 KEEP（只管 stable），通道化后按通道分开。
  const legacyKeep = env.KEEP === undefined || env.KEEP === '' ? '5' : env.KEEP;
  const keepCoreStable = envNumber(
    { ...env, KEEP_CORE_STABLE: env.KEEP_CORE_STABLE || legacyKeep },
    'KEEP_CORE_STABLE',
    5,
  );
  const keepAppStable = envNumber(
    { ...env, KEEP_APP_STABLE: env.KEEP_APP_STABLE || legacyKeep },
    'KEEP_APP_STABLE',
    5,
  );

  const maxBytes = envNumber(env, 'MAX_BYTES', 8589934592);
  const mirrorStatus = envString(env, 'MIRROR_STATUS', path.join(dest, 'mirror-status.json'));

  return {
    repo: envString(env, 'REPO', 'exvlan/exvlan'),
    dest,
    upgradeDest,
    platforms: envString(env, 'PLATFORMS', DEFAULT_PLATFORMS)
      .split(/\s+/)
      .filter((p) => p.length > 0),
    force: env.FORCE === '1',
    keepCoreStable,
    keepAppStable,
    keepRc: envNumber(env, 'KEEP_RC', 3),
    keepBeta: envNumber(env, 'KEEP_BETA', 3),
    keepDev: envNumber(env, 'KEEP_DEV', 3),
    sinceTag: envString(env, 'SINCE_TAG', ''),
    maxBytes,
    mirrorStatus,
    mirrorStatusCompat: envString(
      env,
      'MIRROR_STATUS_COMPAT',
      path.join(path.dirname(dest), 'mirror-status.json'),
    ),
    dryRun: parsed.dryRun,
    tagsFile: parsed.tagsFile,
    verifySha256: env.VERIFY_SHA256 === '1',
    syncLock: envString(env, 'MIRROR_SYNC_LOCK', '') || null,
  };
}

/** 通道保留数（nightly 固定 1，发布时整目录替换）。 */
export function keepCountCore(cfg: Config, channel: string): number {
  switch (channel) {
    case 'stable':
      return cfg.keepCoreStable;
    case 'rc':
      return cfg.keepRc;
    case 'beta':
      return cfg.keepBeta;
    case 'dev':
      return cfg.keepDev;
    default:
      return 1;
  }
}

export function keepCountApp(cfg: Config, channel: string): number {
  switch (channel) {
    case 'stable':
      return cfg.keepAppStable;
    case 'rc':
      return cfg.keepRc;
    case 'beta':
      return cfg.keepBeta;
    case 'dev':
      return cfg.keepDev;
    default:
      return 1;
  }
}
