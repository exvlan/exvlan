/**
 * e2e 夹具：假 `gh` + 假 Release（离线跑，不需要 token、不联网）。
 *
 * 与旧 `e2e-sync-test.sh` 用同一套资产命名，便于逐条对照两者的断言：
 * - 内核 zip：`<platform>-v<ver>.zip`（名字带版本 → 各版本不冲突）
 * - 未版本化资产：`app-*-release.apk` / `*.msi` / `*-setup.exe`（跨版本同名）
 * - 清单三件套：`manifest.json` / `upgrade-manifest.json` / `upgrade-latest.json` / `magisk_update.json`
 */

import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

export const PLATFORMS = [
  'macos-x86_64',
  'macos-aarch64',
  'windows-x86_64',
  'windows-arm64',
  'linux-x86_64',
  'linux-aarch64',
];

export const APP_PLATFORMS = [
  'macos-x86_64',
  'windows-x86_64',
  'android-arm64-v8a',
  'android-armeabi-v7a',
  'android-x86_64',
  'android-x86',
];

export const APKS = [
  'app-arm64-release.apk',
  'app-arm-release.apk',
  'app-x86_64-release.apk',
  'app-x86-release.apk',
];

export const MSI_NAME = 'EXVlan.Studio_26.104.207-5_x64_en-US.msi';
export const SETUP_NAME = 'EXVlan.Studio_26.104.207-5_x64-setup.exe';

const FAKE_GH = `#!/usr/bin/env bash
# 离线 e2e 用的假 gh（不联网、不需要凭据）
set -euo pipefail
releases="\${FAKE_GH_RELEASES:?}"
assets="\${FAKE_GH_ASSETS:?}"
case "\${1:-}" in
  api)
    # 同步脚本只用它取 release 列表（R/A 的 TSV 行）；--jq 表达式在这里等价展开
    cat "$releases"
    ;;
  release)
    [ "\${2:-}" = download ] || { echo "fake gh: 只支持 release download" >&2; exit 1; }
    tag="\${3:?fake gh: 缺 tag}"
    shift 3
    dir=""; patterns=()
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --repo) shift 2 ;;
        -p) patterns+=("$2"); shift 2 ;;
        --dir) dir="$2"; shift 2 ;;
        --clobber) shift ;;
        *) echo "fake gh: 未知参数 $1" >&2; exit 1 ;;
      esac
    done
    [ -n "$dir" ] && [ -d "$assets/$tag" ] || { echo "fake gh: 没有 release $tag" >&2; exit 1; }
    shopt -s nullglob
    n=0
    for p in "\${patterns[@]}"; do
      for f in "$assets/$tag"/$p; do
        [ -f "$f" ] || continue
        cp -f "$f" "$dir/"
        n=$((n + 1))
      done
    done
    [ "$n" -gt 0 ] || { echo "fake gh: $tag 没有匹配的资产" >&2; exit 1; }
    ;;
  *)
    echo "fake gh: 不支持的子命令 \${1:-（空）}" >&2
    exit 1
    ;;
esac
`;

export function sha256File(file: string): string {
  return createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

export interface SandboxOptions {
  /** 是否把宿主的真实 PATH 附在假 gh 之后（legacy 实现需要 bash/python3/du 等系统工具）。 */
  keepHostPath?: boolean;
}

export class Sandbox {
  readonly root: string;
  readonly binDir: string;
  readonly assetsDir: string;
  readonly releasesFile: string;
  readonly tmpDir: string;
  readonly versions: string[] = [];
  /** 固定 tag `nightly` 的假 Release（发布机 nightly 链路用）。 */
  nightly: { publishedAt: string } | null = null;

  constructor(options: SandboxOptions = {}) {
    this.root = fs.mkdtempSync(path.join(os.tmpdir(), 'exvlan-mirror-e2e-'));
    this.binDir = path.join(this.root, 'bin');
    this.assetsDir = path.join(this.root, 'releases');
    this.releasesFile = path.join(this.root, 'releases.tsv');
    this.tmpDir = path.join(this.root, 'tmp');
    fs.mkdirSync(this.binDir, { recursive: true });
    fs.mkdirSync(this.assetsDir, { recursive: true });
    fs.mkdirSync(this.tmpDir, { recursive: true });
    const ghPath = path.join(this.binDir, 'gh');
    fs.writeFileSync(ghPath, FAKE_GH);
    fs.chmodSync(ghPath, 0o755);
    this.keepHostPath = options.keepHostPath ?? true;
  }

  private readonly keepHostPath: boolean;

  cleanup(): void {
    fs.rmSync(this.root, { recursive: true, force: true });
  }

  /** 生成一个假 Release（内核 zip + 应用资产 + 四份清单）。 */
  buildRelease(version: string): void {
    const ver = version;
    const tag = `v${ver}`;
    const dir = path.join(this.assetsDir, tag);
    fs.mkdirSync(dir, { recursive: true });

    for (const platform of PLATFORMS) {
      fs.writeFileSync(path.join(dir, `${platform}-${tag}.zip`), `core ${platform} ${tag}\n`);
    }

    // 应用：版本化资产
    fs.writeFileSync(path.join(dir, `EXVlan.Studio_${ver}_x64.dmg`), `dmg ${ver}\n`);
    fs.writeFileSync(path.join(dir, `EXVlan.Studio_${ver}_x86_64.app.tar.gz`), `updater ${ver}\n`);
    fs.writeFileSync(path.join(dir, `EXVlan.Studio_${ver}_x86_64.app.tar.gz.sig`), `sig app ${ver}\n`);
    fs.writeFileSync(path.join(dir, `Exvlan-Magisk-${tag}.zip`), `magisk ${tag}\n`);

    // 应用：未版本化资产（同名跨版本重复，字节按版本不同）
    for (const name of APKS) fs.writeFileSync(path.join(dir, name), `apk ${ver} ${name}\n`);
    fs.writeFileSync(path.join(dir, MSI_NAME), `msi ${ver}\n`);
    fs.writeFileSync(path.join(dir, `${MSI_NAME}.sig`), `sig msi ${ver}\n`);
    fs.writeFileSync(path.join(dir, SETUP_NAME), `setup ${ver}\n`);
    fs.writeFileSync(path.join(dir, `${SETUP_NAME}.sig`), `sig setup ${ver}\n`);

    // 内核 manifest（只含镜像托管的 6 个平台）
    const coreAssets: Record<string, { file: string; sha256: string; entry: string }> = {};
    for (const platform of PLATFORMS) {
      const file = `${platform}-${tag}.zip`;
      coreAssets[platform] = { file, sha256: sha256File(path.join(dir, file)), entry: 'exvlan' };
    }
    this.writeJson(path.join(dir, 'manifest.json'), {
      schema: 1,
      latest: tag,
      versions: [{ version: tag, assets: coreAssets }],
    });

    // 自建升级清单（镜像里是 manifest.json）
    const appAssets: Record<string, { file: string; sha256: string }> = {};
    for (const platform of APP_PLATFORMS) {
      const name = this.appAssetName(platform, ver);
      appAssets[platform] = { file: name, sha256: sha256File(path.join(dir, name)) };
    }
    this.writeJson(path.join(dir, 'upgrade-manifest.json'), {
      schema: 1,
      latest: ver,
      versions: [{ version: ver, assets: appAssets }],
    });

    // 官方 updater 清单（镜像里是 latest.json；URL 是绝对地址，由 channelize 改写）
    this.writeJson(path.join(dir, 'upgrade-latest.json'), {
      version: ver,
      notes: `Exvlan ${ver}`,
      pub_date: '2026-11-04T00:00:00Z',
      platforms: {
        'darwin-x86_64': {
          signature: `sig-${ver}`,
          url: `https://dl.example.invalid/upgrade/EXVlan.Studio_${ver}_x86_64.app.tar.gz`,
        },
        'windows-x86_64': {
          signature: `sig-${ver}`,
          url: `https://dl.example.invalid/upgrade/${SETUP_NAME}`,
        },
      },
    });

    // Magisk 在线更新清单（镜像里是 magisk_update.json）
    this.writeJson(path.join(dir, 'magisk_update.json'), {
      version: ver,
      versionCode: 1,
      zipUrl: `https://dl.example.invalid/upgrade/Exvlan-Magisk-${tag}.zip`,
      changelog: `Exvlan ${ver}`,
    });

    this.versions.push(ver);
    this.rebuildReleases();
  }

  appAssetName(platform: string, ver: string): string {
    switch (platform) {
      case 'macos-x86_64':
        return `EXVlan.Studio_${ver}_x64.dmg`;
      case 'windows-x86_64':
        return MSI_NAME;
      case 'android-arm64-v8a':
        return 'app-arm64-release.apk';
      case 'android-armeabi-v7a':
        return 'app-arm-release.apk';
      case 'android-x86_64':
        return 'app-x86_64-release.apk';
      case 'android-x86':
        return 'app-x86-release.apk';
      default:
        throw new Error(`未知平台 ${platform}`);
    }
  }

  private writeJson(file: string, value: unknown): void {
    fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
  }

  assetDir(version: string): string {
    return path.join(this.assetsDir, `v${version}`);
  }

  nightlyAssetDir(): string {
    return path.join(this.assetsDir, 'nightly');
  }

  /**
   * 固定 tag `nightly` 的假 Release：内核 zip 名带 `-nightly.`（走
   * `<platform>-*nightly*.zip` pattern），清单 latest 是完整版本串。
   */
  buildNightlyRelease(version: string, stamp = '1115'): void {
    const tag = 'nightly';
    const declared = `v${version}-nightly.${stamp}`;
    const dir = this.nightlyAssetDir();
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, 'manifest.json'), JSON.stringify({ schema: 1, latest: declared, versions: [] }));

    const coreAssets: Record<string, { file: string; sha256: string; entry: string }> = {};
    for (const platform of PLATFORMS) {
      const file = `${platform}-${version}-nightly.${stamp}.zip`;
      fs.writeFileSync(path.join(dir, file), `core ${platform} ${tag} ${stamp}\n`);
      coreAssets[platform] = { file, sha256: sha256File(path.join(dir, file)), entry: 'exvlan' };
    }
    this.writeJson(path.join(dir, 'manifest.json'), {
      schema: 1,
      latest: declared,
      versions: [{ version: declared, assets: coreAssets }],
    });

    const appAssets: Record<string, { file: string; sha256: string }> = {};
    for (const platform of APP_PLATFORMS) {
      const name =
        platform.startsWith('android-')
          ? `app-${platform.replace('android-', '').replace('-v8', '').replace('armeabi-v7a', 'arm')}-release.apk`
          : platform === 'macos-x86_64'
            ? `EXVlan.Studio_${version}_x64.dmg`
            : MSI_NAME;
      if (!fs.existsSync(path.join(dir, name))) {
        fs.writeFileSync(path.join(dir, name), `${tag} ${stamp} ${name}\n`);
      }
      appAssets[platform] = { file: name, sha256: sha256File(path.join(dir, name)) };
    }
    this.writeJson(path.join(dir, 'upgrade-manifest.json'), {
      schema: 1,
      latest: version,
      versions: [{ version, assets: appAssets }],
    });
    this.writeJson(path.join(dir, 'upgrade-latest.json'), {
      version,
      notes: `nightly ${stamp}`,
      pub_date: '2026-11-05T00:00:00Z',
      platforms: {
        'darwin-x86_64': { signature: `sig-${stamp}`, url: `https://dl.example.invalid/upgrade/EXVlan.Studio_${version}_x64.dmg` },
      },
    });
    this.writeJson(path.join(dir, 'magisk_update.json'), {
      version,
      versionCode: 1,
      zipUrl: `https://dl.example.invalid/upgrade/Exvlan-Magisk-${tag}.zip`,
    });
    fs.writeFileSync(path.join(dir, `Exvlan-Magisk-${tag}.zip`), `magisk ${tag} ${stamp}\n`);

    this.nightly = { publishedAt: `2026-11-05T0${stamp === '1115' ? '1' : '2'}:00:00Z` };
    this.rebuildReleases();
  }

  /** 从资产目录重建「gh api」输出（R/A 行，与真实 --jq 一致）。 */
  rebuildReleases(): void {
    const rows: string[] = [];
    const tags: Array<{ tag: string; dir: string; publishedAt: string }> = [];
    if (this.nightly) {
      tags.push({ tag: 'nightly', dir: this.nightlyAssetDir(), publishedAt: this.nightly.publishedAt });
    }
    for (const ver of this.versions) {
      tags.push({ tag: `v${ver}`, dir: this.assetDir(ver), publishedAt: '2026-11-04T00:00:00Z' });
    }
    for (const entry of tags) {
      rows.push(`R\t${entry.tag}\t${entry.publishedAt}\tfalse`);
    }
    for (const entry of tags) {
      const names = fs
        .readdirSync(entry.dir)
        .filter((name) => fs.statSync(path.join(entry.dir, name)).isFile())
        .sort((a, b) => Buffer.compare(Buffer.from(a, 'utf8'), Buffer.from(b, 'utf8')));
      for (const name of names) {
        rows.push(`A\t${entry.tag}\t${name}\t${fs.statSync(path.join(entry.dir, name)).size}`);
      }
    }
    fs.writeFileSync(this.releasesFile, rows.length === 0 ? '' : `${rows.join('\n')}\n`);
  }

  env(overrides: Record<string, string> = {}): Record<string, string> {
    const base: Record<string, string> = {
      HOME: this.root,
      TMPDIR: this.tmpDir,
      FAKE_GH_RELEASES: this.releasesFile,
      FAKE_GH_ASSETS: this.assetsDir,
      REPO: 'exvlan/exvlan',
      MAX_BYTES: '8589934592',
      KEEP_CORE_STABLE: '5',
      KEEP_APP_STABLE: '5',
    };
    const hostPath = this.keepHostPath ? (process.env.PATH ?? '/usr/local/bin:/usr/bin:/bin') : '';
    base.PATH = [this.binDir, hostPath].filter((p) => p !== '').join(':');
    return { ...base, ...overrides };
  }
}

export interface RunResult {
  code: number;
  log: string;
}

/** 跑一轮**新**实现（Bun 入口）。 */
export async function runNew(
  sandbox: Sandbox,
  distRoot: string,
  overrides: Record<string, string> = {},
  args: readonly string[] = [],
): Promise<RunResult> {
  const env = sandbox.env({
    DEST: path.join(distRoot, 'core'),
    UPGRADE_DEST: path.join(distRoot, 'upgrade'),
    ...overrides,
  });
  const entry = path.join(import.meta.dir, '..', 'index.ts');
  // 用当前 Bun 的可执行文件绝对路径：测试把 HOME 指向沙箱（隔离 gh 配置），
  // 走 PATH 会命中 volta/asdf 之类需要 HOME 的 shim。
  return spawnCollect([process.execPath, entry, ...args], env);
}

/** 跑一轮**旧**实现（shell + python，仅用于等价性对照）。 */
export async function runLegacy(
  sandbox: Sandbox,
  distRoot: string,
  overrides: Record<string, string> = {},
): Promise<RunResult> {
  const env = sandbox.env({
    DEST: path.join(distRoot, 'core'),
    UPGRADE_DEST: path.join(distRoot, 'upgrade'),
    ...overrides,
  });
  const entry = path.join(import.meta.dir, '..', 'exvlan-kernel-sync.sh');
  return spawnCollect(['bash', entry], env);
}

async function spawnCollect(argv: string[], env: Record<string, string>): Promise<RunResult> {
  const proc = Bun.spawn(argv, {
    env: env as Record<string, string>,
    stdout: 'pipe',
    stderr: 'pipe',
  });
  const [stdout, stderr, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  return { code, log: `${stdout}${stderr}` };
}

export interface TreeEntry {
  path: string;
  type: 'file' | 'dir' | 'link';
  /** 文件内容 sha256 / 链接目标 / 目录（空）。 */
  value: string;
}

/** 递归列出目录树（含符号链接本体，不跟随）。 */
export function listTree(root: string): TreeEntry[] {
  const out: TreeEntry[] = [];
  const walk = (dir: string, prefix: string): void => {
    const names = fs.readdirSync(dir).sort();
    for (const name of names) {
      const full = path.join(dir, name);
      const rel = prefix === '' ? name : `${prefix}/${name}`;
      const stat = fs.lstatSync(full);
      if (stat.isSymbolicLink()) {
        out.push({ path: rel, type: 'link', value: fs.readlinkSync(full) });
      } else if (stat.isDirectory()) {
        out.push({ path: rel, type: 'dir', value: '' });
        walk(full, rel);
      } else {
        out.push({ path: rel, type: 'file', value: sha256File(full) });
      }
    }
  };
  walk(root, '');
  return out;
}

export function readJson(file: string): Record<string, unknown> {
  return JSON.parse(fs.readFileSync(file, 'utf8')) as Record<string, unknown>;
}

export function readText(file: string): string {
  try {
    return fs.readFileSync(file, 'utf8');
  } catch {
    return '';
  }
}
