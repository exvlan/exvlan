/**
 * e2e 场景回归（Bun 版），与旧 `e2e-sync-test.sh` 的 62 条断言**逐条对应**。
 *
 * 断言名保持旧脚本的原文（便于对照 docs/mirror-toolchain.md 的对照表）：
 *   A  首轮发布两版（20711 最新 / 20710 旧）          15 条
 *   B  只重下旧 tag（模拟最新版指纹命中）              12 条
 *   B2 再跑一轮无变化：不降级、不粘住、不重下          10 条
 *   C  FORCE=1 全量重下                               11 条
 *   D  MAX_BYTES 预算收缩                             14 条
 *   ------------------------------------------------------------
 *   合计                                              62 条
 *
 * 每个 `test()` 就是一条断言；同一 describe 内按声明顺序串行执行（共享上一步的同步结果）。
 */

import { afterAll, beforeAll, describe, expect, setDefaultTimeout, test } from 'bun:test';
import fs from 'node:fs';
import path from 'node:path';
import {
  APKS,
  APP_PLATFORMS,
  MSI_NAME,
  SETUP_NAME,
  Sandbox,
  readJson,
  readText,
  runNew,
  sha256File,
} from './harness';

// 并行跑其它测试文件时单轮同步可能超过默认 5s
setDefaultTimeout(60_000);

let sandbox: Sandbox;
let dist: string;
let logA = '';
let logB = '';
let logB2 = '';
let logC = '';
let logD = '';

const LATEST = '26.410.20711';
const OLDER = '26.410.20710';
const NEWEST = '26.410.20712';

function latestJson(file: string): Record<string, unknown> {
  return readJson(file);
}

function appVersionOf(file: string): unknown {
  return latestJson(file).version;
}

function checkAppManifest(want: string): string[] {
  const manifest = readJson(path.join(dist, 'upgrade/stable/manifest.json'));
  const latest = manifest.latest as string;
  const versions = (manifest.versions as Array<Record<string, unknown>>) ?? [];
  const entry = versions.find((v) => v.version === latest);
  const problems: string[] = [];
  if (!entry) problems.push(`manifest.latest=${latest} 没有对应版本条目`);
  if (latest !== want) problems.push(`manifest.latest=${latest} != 期望 ${want}`);
  const assets = (entry?.assets as Record<string, { file?: string; sha256?: string }>) ?? {};
  for (const [platform, meta] of Object.entries(assets)) {
    const name = meta.file as string;
    const file = path.join(dist, 'upgrade/stable', name);
    try {
      const digest = sha256File(file);
      if (digest !== meta.sha256) problems.push(`${platform}: ${name} sha256 不符`);
    } catch {
      problems.push(`${platform}: ${name} 读不到`);
    }
  }
  if (Object.keys(assets).length !== 6) problems.push(`平台数 ${Object.keys(assets).length} != 6`);
  return problems;
}

function compatProblems(wantResult: string, maxBytes: string): string[] {
  const file = path.join(dist, 'mirror-status.json');
  const data = readJson(file);
  const need = [
    'checked_at',
    'round',
    'result',
    'detail',
    'since_tag',
    'latest_release',
    'tags',
    'first_ok_at',
    'last_change_at',
    'wait_since',
    'bytes',
  ];
  const problems: string[] = [];
  const missing = need.filter((key) => !(key in data));
  if (missing.length > 0) problems.push(`缺字段 ${missing.join(',')}`);
  if (data.result !== wantResult) problems.push(`result=${String(data.result)} != ${wantResult}`);
  if (Number(data.round ?? 0) < 1) problems.push('round 未递增');
  const bytes = (data.bytes ?? {}) as Record<string, unknown>;
  if (Number(bytes.max ?? 0) !== Number(maxBytes)) {
    problems.push(`bytes.max=${String(bytes.max)} != ${maxBytes}`);
  }
  if ('channels' in data) problems.push('兼容文件混入了新 schema 的 channels 字段');
  return problems;
}

function coreManifestLatest(): unknown {
  return readJson(path.join(dist, 'core/stable/manifest.json')).latest;
}

beforeAll(() => {
  sandbox = new Sandbox();
  dist = path.join(sandbox.root, 'dist');
  sandbox.buildRelease(LATEST);
  sandbox.buildRelease(OLDER);
});

afterAll(() => {
  sandbox.cleanup();
});

describe('A. 首轮发布 v26.410.20711 + v26.410.20710', () => {
  test('两版都下载了', async () => {
    const result = await runNew(sandbox, dist);
    logA = result.log;
    expect(result.code).toBe(0);
    expect(logA).toContain('v26.410.20710：下载');
  });

  test('最新版也下载了', () => {
    expect(logA).toContain('v26.410.20711：下载');
  });

  test('latest.json 版本', () => {
    expect(appVersionOf(path.join(dist, 'upgrade/stable/latest.json'))).toBe(LATEST);
  });

  test('magisk_update.json 版本', () => {
    expect(appVersionOf(path.join(dist, 'upgrade/stable/magisk_update.json'))).toBe(LATEST);
  });

  for (const apk of APKS) {
    test(`未版本化资产 ${apk} 仍是 ${LATEST}`, () => {
      expect(readText(path.join(dist, 'upgrade/stable', apk)).trim()).toBe(`apk ${LATEST} ${apk}`);
    });
  }

  test(`未版本化资产 msi 仍是 ${LATEST}`, () => {
    expect(readText(path.join(dist, 'upgrade/stable', MSI_NAME)).trim()).toBe(`msi ${LATEST}`);
  });

  test(`未版本化资产 setup.exe 仍是 ${LATEST}`, () => {
    expect(readText(path.join(dist, 'upgrade/stable', SETUP_NAME)).trim()).toBe(`setup ${LATEST}`);
  });

  test('upgrade/manifest.json 最新版 sha256：6 个平台全部 MATCH', () => {
    expect(checkAppManifest(LATEST)).toEqual([]);
  });

  test('core/manifest.json 的 latest', () => {
    expect(coreManifestLatest()).toBe(`v${LATEST}`);
  });

  test('旧 schema 兼容状态 OK', () => {
    expect(compatProblems('ok', '8589934592')).toEqual([]);
  });

  test('旧布局符号链接：core/<zip> → stable/<zip>', () => {
    for (const name of [`linux-x86_64-v${LATEST}.zip`, `macos-aarch64-v${OLDER}.zip`]) {
      const link = path.join(dist, 'core', name);
      expect(fs.lstatSync(link).isSymbolicLink()).toBe(true);
      expect(fs.readlinkSync(link)).toBe(`stable/${name}`);
      expect(fs.statSync(link).isFile()).toBe(true);
    }
  });

  test('core/stable/manifest.json 别名', () => {
    expect(fs.readlinkSync(path.join(dist, 'core/manifest.json'))).toBe('stable/manifest.json');
  });
});

describe('B. 只重下旧 tag v26.410.20710（模拟最新版指纹命中）', () => {
  test('旧 tag 被重下', async () => {
    fs.writeFileSync(path.join(sandbox.assetDir(OLDER), 'app-x86-release.apk'), 'apk re-uploaded 26.410.20710\n');
    sandbox.rebuildReleases();
    const result = await runNew(sandbox, dist);
    logB = result.log;
    expect(result.code).toBe(0);
    expect(logB).toContain('v26.410.20710：下载');
  });

  test('最新版没有再下载（指纹命中）', () => {
    expect(logB).not.toContain('v26.410.20711：下载');
  });

  test('latest.json 版本', () => {
    expect(appVersionOf(path.join(dist, 'upgrade/stable/latest.json'))).toBe(LATEST);
  });

  test('magisk_update.json 版本', () => {
    expect(appVersionOf(path.join(dist, 'upgrade/stable/magisk_update.json'))).toBe(LATEST);
  });

  for (const apk of APKS) {
    test(`未版本化资产 ${apk} 仍是 ${LATEST}`, () => {
      expect(readText(path.join(dist, 'upgrade/stable', apk)).trim()).toBe(`apk ${LATEST} ${apk}`);
    });
  }

  test(`未版本化资产 msi 仍是 ${LATEST}`, () => {
    expect(readText(path.join(dist, 'upgrade/stable', MSI_NAME)).trim()).toBe(`msi ${LATEST}`);
  });

  test(`未版本化资产 setup.exe 仍是 ${LATEST}`, () => {
    expect(readText(path.join(dist, 'upgrade/stable', SETUP_NAME)).trim()).toBe(`setup ${LATEST}`);
  });

  test('upgrade/manifest.json 最新版 sha256：6 个平台全部 MATCH', () => {
    expect(checkAppManifest(LATEST)).toEqual([]);
  });

  test('core/manifest.json 的 latest 未回退', () => {
    expect(coreManifestLatest()).toBe(`v${LATEST}`);
  });
});

describe('B2. 无变化再跑一轮（不降级、不重下）', () => {
  test('没有重下任何 tag', async () => {
    const result = await runNew(sandbox, dist);
    logB2 = result.log;
    expect(result.code).toBe(0);
    expect(logB2).not.toContain('：下载');
  });

  test('latest.json 版本', () => {
    expect(appVersionOf(path.join(dist, 'upgrade/stable/latest.json'))).toBe(LATEST);
  });

  test('magisk_update.json 版本', () => {
    expect(appVersionOf(path.join(dist, 'upgrade/stable/magisk_update.json'))).toBe(LATEST);
  });

  for (const apk of APKS) {
    test(`未版本化资产 ${apk} 仍是 ${LATEST}`, () => {
      expect(readText(path.join(dist, 'upgrade/stable', apk)).trim()).toBe(`apk ${LATEST} ${apk}`);
    });
  }

  test(`未版本化资产 msi 仍是 ${LATEST}`, () => {
    expect(readText(path.join(dist, 'upgrade/stable', MSI_NAME)).trim()).toBe(`msi ${LATEST}`);
  });

  test(`未版本化资产 setup.exe 仍是 ${LATEST}`, () => {
    expect(readText(path.join(dist, 'upgrade/stable', SETUP_NAME)).trim()).toBe(`setup ${LATEST}`);
  });

  test('upgrade/manifest.json 最新版 sha256：6 个平台全部 MATCH', () => {
    expect(checkAppManifest(LATEST)).toEqual([]);
  });
});

describe('C. FORCE=1 全量重下', () => {
  test('旧 tag 也重下了', async () => {
    const result = await runNew(sandbox, dist, { FORCE: '1' });
    logC = result.log;
    expect(result.code).toBe(0);
    expect(logC).toContain('v26.410.20710：下载');
  });

  test('最新版也重下了', () => {
    expect(logC).toContain('v26.410.20711：下载');
  });

  test('latest.json 版本', () => {
    expect(appVersionOf(path.join(dist, 'upgrade/stable/latest.json'))).toBe(LATEST);
  });

  test('magisk_update.json 版本', () => {
    expect(appVersionOf(path.join(dist, 'upgrade/stable/magisk_update.json'))).toBe(LATEST);
  });

  for (const apk of APKS) {
    test(`未版本化资产 ${apk} 仍是 ${LATEST}`, () => {
      expect(readText(path.join(dist, 'upgrade/stable', apk)).trim()).toBe(`apk ${LATEST} ${apk}`);
    });
  }

  test(`未版本化资产 msi 仍是 ${LATEST}`, () => {
    expect(readText(path.join(dist, 'upgrade/stable', MSI_NAME)).trim()).toBe(`msi ${LATEST}`);
  });

  test(`未版本化资产 setup.exe 仍是 ${LATEST}`, () => {
    expect(readText(path.join(dist, 'upgrade/stable', SETUP_NAME)).trim()).toBe(`setup ${LATEST}`);
  });

  test('upgrade/manifest.json 最新版 sha256：6 个平台全部 MATCH', () => {
    expect(checkAppManifest(LATEST)).toEqual([]);
  });
});

describe('D. MAX_BYTES 预算裁剪（超限从最旧版本收缩）', () => {
  let maxBytes = '0';

  test('打印预算裁剪日志', async () => {
    sandbox.buildRelease(NEWEST);
    // 单 tag 的资产总大小（不含清单 .json）
    const perTag = readText(sandbox.releasesFile)
      .split('\n')
      .filter((line) => line.startsWith(`A\tv${NEWEST}\t`))
      .filter((line) => !line.split('\t')[2].endsWith('.json'))
      .reduce((sum, line) => sum + Number(line.split('\t')[3] ?? 0), 0);
    maxBytes = String(Math.floor((perTag * 3) / 2));
    fs.rmSync(dist, { recursive: true, force: true });
    const result = await runNew(sandbox, dist, { MAX_BYTES: maxBytes });
    logD = result.log;
    expect(result.code).toBe(0);
    expect(logD).toContain('超出 MAX_BYTES');
  });

  test('预算裁剪后只保留 1 个版本', () => {
    const data = readJson(path.join(dist, 'core/stable/manifest.json'));
    expect((data.versions as unknown[]).length).toBe(1);
  });

  test('裁剪后 core latest 仍是最新 stable', () => {
    expect(coreManifestLatest()).toBe(`v${NEWEST}`);
  });

  test('latest.json 版本', () => {
    expect(appVersionOf(path.join(dist, 'upgrade/stable/latest.json'))).toBe(NEWEST);
  });

  test('magisk_update.json 版本', () => {
    expect(appVersionOf(path.join(dist, 'upgrade/stable/magisk_update.json'))).toBe(NEWEST);
  });

  for (const apk of APKS) {
    test(`未版本化资产 ${apk} 仍是 ${NEWEST}`, () => {
      expect(readText(path.join(dist, 'upgrade/stable', apk)).trim()).toBe(`apk ${NEWEST} ${apk}`);
    });
  }

  test(`未版本化资产 msi 仍是 ${NEWEST}`, () => {
    expect(readText(path.join(dist, 'upgrade/stable', MSI_NAME)).trim()).toBe(`msi ${NEWEST}`);
  });

  test(`未版本化资产 setup.exe 仍是 ${NEWEST}`, () => {
    expect(readText(path.join(dist, 'upgrade/stable', SETUP_NAME)).trim()).toBe(`setup ${NEWEST}`);
  });

  test('upgrade/manifest.json 最新版 sha256：6 个平台全部 MATCH', () => {
    expect(checkAppManifest(NEWEST)).toEqual([]);
  });

  test('旧 schema 兼容状态 OK', () => {
    expect(compatProblems('ok', maxBytes)).toEqual([]);
  });

  test('单版本下载量等于单 tag 预算以内', () => {
    expect(logD.split('：下载').length - 1).toBe(1);
  });
});
