/**
 * 入口兼容与边界行为：CLI 参数、env 变量、空通道、nightly 整目录替换、
 * `--dry-run` 不落盘、`MIRROR_STATUS*` 覆盖、参数错误退出码。
 *
 * 这些是「发布机换镜像后不会静默跑错」的护栏：旧脚本读的变量名/参数必须继续生效。
 */

import { afterAll, beforeAll, describe, expect, test } from 'bun:test';
import fs from 'node:fs';
import path from 'node:path';
import { Sandbox, readJson, readText, runNew } from './harness';

let sandbox: Sandbox;

beforeAll(() => {
  sandbox = new Sandbox();
  sandbox.buildRelease('26.410.20711');
  sandbox.buildRelease('26.410.20710');
});

afterAll(() => {
  sandbox.cleanup();
});

describe('CLI 兼容', () => {
  test('--help 退出 0 且不落盘', async () => {
    const dist = path.join(sandbox.root, 'cli-help');
    const result = await runNew(sandbox, dist, {}, ['--help']);
    expect(result.code).toBe(0);
    expect(result.log).toContain('--dry-run');
    expect(fs.existsSync(dist)).toBe(false);
  });

  test('未知参数 → 退出 1 并打印用法', async () => {
    const dist = path.join(sandbox.root, 'cli-bad');
    const result = await runNew(sandbox, dist, {}, ['--nope']);
    expect(result.code).toBe(1);
    expect(result.log).toContain('未知参数：--nope');
    expect(result.log).toContain('用法：');
  });

  test('--tags-file 不存在 → 退出 1', async () => {
    const dist = path.join(sandbox.root, 'cli-missing-tags');
    const result = await runNew(sandbox, dist, {}, ['--tags-file', path.join(sandbox.root, 'no-such-file')]);
    expect(result.code).toBe(1);
    expect(result.log).toContain('读不到 --tags-file');
  });

  test('--dry-run 只打印计划，不改写任何文件', async () => {
    const dist = path.join(sandbox.root, 'cli-dry');
    const result = await runNew(sandbox, dist, {}, ['--dry-run']);
    expect(result.code).toBe(0);
    expect(result.log).toContain('dry-run 结束');
    expect(fs.existsSync(dist)).toBe(false);
  });

  test('--dry-run 的报告里含通道目录与别名计划', async () => {
    const result = await runNew(sandbox, path.join(sandbox.root, 'cli-dry2'), {}, ['--dry-run']);
    expect(result.log).toContain('别名：');
    expect(result.log).toContain('core 同步 2 个 tag');
    expect(result.log).toContain('状态文件：');
  });

  test('MAX_BYTES 非数字 → 退出 1 且提示变量名', async () => {
    const result = await runNew(sandbox, path.join(sandbox.root, 'cli-maxbytes'), { MAX_BYTES: 'abc' });
    expect(result.code).toBe(1);
    expect(result.log).toContain('MAX_BYTES');
  });
});

describe('环境变量兼容', () => {
  test('MIRROR_STATUS / MIRROR_STATUS_COMPAT 覆盖生效', async () => {
    const dist = path.join(sandbox.root, 'env-status');
    const status = path.join(sandbox.root, 'env-status-custom', 'new.json');
    const compat = path.join(sandbox.root, 'env-status-custom', 'compat.json');
    const result = await runNew(sandbox, dist, { MIRROR_STATUS: status, MIRROR_STATUS_COMPAT: compat });
    expect(result.code).toBe(0);
    expect(fs.existsSync(status)).toBe(true);
    expect(fs.existsSync(compat)).toBe(true);
    // 默认位置不再被写
    expect(fs.existsSync(path.join(dist, 'core', 'mirror-status.json'))).toBe(false);
  });

  test('KEEP 旧变量仍管 stable 保留数（未设 KEEP_CORE_STABLE 时）', async () => {
    const dist = path.join(sandbox.root, 'env-keep');
    // 旧 shell 语义：KEEP_CORE_STABLE 为空时回落到 KEEP
    const result = await runNew(sandbox, dist, { KEEP: '1', KEEP_CORE_STABLE: '', KEEP_APP_STABLE: '' });
    expect(result.code).toBe(0);
    const data = readJson(path.join(dist, 'core/stable/manifest.json'));
    expect((data.versions as unknown[]).length).toBe(1);
  });

  test('rc 通道下载失败时不创建半套目录（通道独立）', async () => {
    const dist = path.join(sandbox.root, 'env-keep-rc');
    const tagsFile = path.join(sandbox.root, 'rc-tags');
    fs.writeFileSync(tagsFile, 'v26.410.20711-rc.1\nv26.410.20710-rc.1\n');
    const result = await runNew(sandbox, dist, { KEEP_RC: '1' }, ['--tags-file', tagsFile]);
    expect(result.code).toBe(0);
    // fake gh 里没有 rc 资产 → 下载失败；rc 目录不落盘（旧内容保持），stable 也不受影响
    expect(result.log).toContain('下载失败');
    expect(fs.existsSync(path.join(dist, 'core', 'rc'))).toBe(false);
  });
});

describe('空通道与 nightly', () => {
  test('只有 legacy tag 时不创建通道目录、state 仍写 waiting', async () => {
    const dist = path.join(sandbox.root, 'empty');
    const tagsFile = path.join(sandbox.root, 'legacy-tags');
    fs.writeFileSync(tagsFile, 'v26.410.20713-pre.9\npreview-1\n');
    const result = await runNew(sandbox, dist, {}, ['--tags-file', tagsFile]);
    expect(result.code).toBe(0);
    expect(result.log).toContain('没有可同步的 release，退出');
    expect(fs.existsSync(path.join(dist, 'core', 'stable'))).toBe(false);
    expect(fs.existsSync(path.join(dist, 'core', 'nightly'))).toBe(false);
    expect(fs.existsSync(path.join(dist, 'upgrade'))).toBe(false);
    const compat = readJson(path.join(dist, 'mirror-status.json'));
    expect(compat.result).toBe('waiting');
    expect(compat.round).toBe(1);
  });

  test('nightly：发布 + 写 .published-at，第二轮指纹命中跳过下载', async () => {
    const nightlySandbox = new Sandbox();
    try {
      nightlySandbox.buildNightlyRelease('26.410.20714');
      const dist = path.join(nightlySandbox.root, 'dist');
      const first = await runNew(nightlySandbox, dist);
      expect(first.code).toBe(0);
      expect(fs.existsSync(path.join(dist, 'core/nightly/manifest.json'))).toBe(true);
      expect(readText(path.join(dist, 'core/nightly/.published-at')).trim()).toBe('2026-11-05T01:00:00Z');

      const second = await runNew(nightlySandbox, dist);
      expect(second.code).toBe(0);
      expect(second.log).toContain('nightly/nightly：指纹/发布时间未变，跳过下载');
    } finally {
      nightlySandbox.cleanup();
    }
  });

  test('nightly：整目录替换（保留 .sync-state/.published-at，旧文件清掉）', async () => {
    const nightlySandbox = new Sandbox();
    try {
      nightlySandbox.buildNightlyRelease('26.410.20714');
      const dist = path.join(nightlySandbox.root, 'dist');
      expect((await runNew(nightlySandbox, dist)).code).toBe(0);
      fs.writeFileSync(path.join(dist, 'core/nightly/stale.zip'), 'stale\n');

      // 覆盖上传同一 tag：资产大小变化 → 指纹变化 → 重下并整目录替换
      const assetDir = nightlySandbox.nightlyAssetDir();
      const zip = fs.readdirSync(assetDir).find((name) => name.endsWith('-nightly.1115.zip'));
      expect(zip).toBeDefined();
      fs.writeFileSync(path.join(assetDir, zip as string), 'core rebuilt content\n');
      nightlySandbox.rebuildReleases();
      // manifest 里的 sha256 与新内容不一致，但默认不校验（VERIFY_SHA256 关闭）
      const result = await runNew(nightlySandbox, dist);
      expect(result.code).toBe(0);
      expect(fs.existsSync(path.join(dist, 'core/nightly/stale.zip'))).toBe(false);
      expect(fs.existsSync(path.join(dist, 'core/nightly/.sync-state'))).toBe(true);
      expect(fs.existsSync(path.join(dist, 'core/nightly/.published-at'))).toBe(true);
    } finally {
      nightlySandbox.cleanup();
    }
  });

  test('VERIFY_SHA256=1：sha256 不符只告警，不改变发布结果', async () => {
    const verifySandbox = new Sandbox();
    try {
      verifySandbox.buildRelease('26.410.20711');
      const dist = path.join(verifySandbox.root, 'dist');
      const manifestPath = path.join(verifySandbox.assetDir('26.410.20711'), 'manifest.json');
      const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
      manifest.versions[0].assets['linux-x86_64'].sha256 = 'deadbeef';
      fs.writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
      verifySandbox.rebuildReleases();
      const result = await runNew(verifySandbox, dist, { VERIFY_SHA256: '1' });
      expect(result.code).toBe(0);
      expect(result.log).toContain('sha256 与清单不符');
      expect(fs.existsSync(path.join(dist, 'core/stable/manifest.json'))).toBe(true);
    } finally {
      verifySandbox.cleanup();
    }
  });
});
