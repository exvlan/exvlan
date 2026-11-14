/** 单元测试：清单合并/平台裁剪/URL 通道化/版本比较/Python 兼容 JSON/状态双写/通道选择。 */

import { describe, expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { channelizeFile, channelizeUrl } from '../src/channelize';
import { pyJsonDumps } from '../src/pyjson';
import {
  channelLatest,
  channelVersions,
  filterPlatforms,
  manifestFiles,
  mergeManifests,
  versionKey,
} from '../src/manifest';
import { versionCompare, versionGe } from '../src/version';
import { classifyTag, selectTags } from '../src/tags';
import { buildStatusInput, readChannels, writeCompatStatus } from '../src/status';
import type { Config } from '../src/config';

function tmpdir(): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'exvlan-units-'));
}

function writeJson(file: string, value: unknown): void {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

function baseConfig(dest: string): Config {
  return {
    repo: 'exvlan/exvlan',
    dest: path.join(dest, 'core'),
    upgradeDest: path.join(dest, 'upgrade'),
    platforms: ['linux-x86_64', 'macos-aarch64'],
    force: false,
    keepCoreStable: 5,
    keepAppStable: 5,
    keepRc: 3,
    keepBeta: 3,
    keepDev: 3,
    sinceTag: '',
    maxBytes: 8589934592,
    mirrorStatus: path.join(dest, 'core', 'mirror-status.json'),
    mirrorStatusCompat: path.join(dest, 'mirror-status.json'),
    dryRun: false,
    tagsFile: null,
    verifySha256: false,
    syncLock: null,
  };
}

describe('版本比较（替代 sort -V）', () => {
  test('stable 内部数值比较', () => {
    expect(versionCompare('v26.410.20711', 'v26.410.20710')).toBeGreaterThan(0);
    expect(versionCompare('v26.410.2079', 'v26.410.20711')).toBeLessThan(0);
  });

  test('sort -V 语义：同 base 时无后缀者更小（通道内后缀同质，不影响排序）', () => {
    expect(versionCompare('v26.410.20713', 'v26.410.20713-rc.1')).toBeLessThan(0);
    expect(versionCompare('v26.410.20713-nightly.1115', 'v26.410.20713')).toBeGreaterThan(0);
    // stage 语义（stable > rc > beta > dev > nightly）由 merge_manifests 的 versionKey 负责
    expect(versionKey('26.410.20713')).toEqual([26, 410, 20713, 5, 0]);
  });

  test('同一 stage 序号递增', () => {
    expect(versionCompare('v26.410.20713-dev.2', 'v26.410.20713-dev.1')).toBeGreaterThan(0);
    expect(versionCompare('v26.410.20713-rc.10', 'v26.410.20713-rc.9')).toBeGreaterThan(0);
  });

  test('基础版本优先于后缀（20714-nightly > 20713）', () => {
    expect(versionCompare('v26.410.20714-nightly.1115', 'v26.410.20713')).toBeGreaterThan(0);
  });

  test('versionGe 与 SINCE_TAG 判定', () => {
    expect(versionGe('v26.410.20711', 'v26.410.20706')).toBe(true);
    expect(versionGe('v26.410.20705', 'v26.410.20706')).toBe(false);
    expect(versionGe('v26.410.20706', 'v26.410.20706')).toBe(true);
  });
});

describe('tag → 通道', () => {
  test('归类表', () => {
    expect(classifyTag('v26.410.20713')).toBe('stable');
    expect(classifyTag('v26.410.20713-rc.1')).toBe('rc');
    expect(classifyTag('v26.410.20713-beta.2')).toBe('beta');
    expect(classifyTag('v26.410.20713-dev.1')).toBe('dev');
    expect(classifyTag('v26.410.20714-nightly.1115')).toBe('nightly');
    expect(classifyTag('nightly')).toBe('nightly');
  });

  test('legacy / 未知 tag 不镜像', () => {
    expect(classifyTag('v26.410.20713-pre.9')).toBe('skip');
    expect(classifyTag('preview-26')).toBe('skip');
    expect(classifyTag('release-2026')).toBe('skip');
  });
});

describe('Python 兼容 JSON 序列化', () => {
  test('空容器与嵌套缩进', () => {
    expect(pyJsonDumps({ a: [], b: {} })).toBe('{\n  "a": [],\n  "b": {}\n}');
    expect(pyJsonDumps([1, 'x'])).toBe('[\n  1,\n  "x"\n]');
  });

  test('非 ASCII 原样、控制字符小写 \\u00xx', () => {
    expect(pyJsonDumps('中文')).toBe('"中文"');
    expect(pyJsonDumps('\u0001')).toBe('"\\u0001"');
    expect(pyJsonDumps('a\nb\tc')).toBe('"a\\nb\\tc"');
  });

  test('整数不带小数点', () => {
    expect(pyJsonDumps(42)).toBe('42');
    expect(pyJsonDumps(null)).toBe('null');
  });
});

describe('清单合并（merge_manifests）', () => {
  test('同一版本先出现者为准 + 版本倒序 + channel 保留', () => {
    const dir = tmpdir();
    const a = path.join(dir, 'a.json');
    const b = path.join(dir, 'b.json');
    writeJson(a, {
      channel: 'stable',
      versions: [
        { version: '26.410.20711', assets: { 'linux-x86_64': { file: 'new.zip' } } },
        { version: '26.410.20712', assets: {} },
      ],
    });
    writeJson(b, {
      versions: [
        { version: '26.410.20711', assets: { 'linux-x86_64': { file: 'old.zip' } } },
        { version: '26.410.20710', assets: {} },
      ],
    });
    const out = path.join(dir, 'out.json');
    expect(mergeManifests([a, b], out, { keep: 5, since: '' })).toBe(true);
    const merged = JSON.parse(fs.readFileSync(out, 'utf8'));
    expect(merged.schema).toBe(1);
    expect(merged.channel).toBe('stable');
    expect(merged.latest).toBe('26.410.20712');
    expect(merged.versions.map((v: { version: string }) => v.version)).toEqual([
      '26.410.20712',
      '26.410.20711',
      '26.410.20710',
    ]);
    expect(merged.versions[1].assets['linux-x86_64'].file).toBe('new.zip');
    // Python 的 json.dumps 不带结尾换行
    expect(fs.readFileSync(out, 'utf8').endsWith('}')).toBe(true);
  });

  test('--keep 与 --since 裁剪', () => {
    const dir = tmpdir();
    const a = path.join(dir, 'a.json');
    writeJson(a, {
      versions: [
        { version: '26.410.20709', assets: {} },
        { version: '26.410.20710', assets: {} },
        { version: '26.410.20711', assets: {} },
      ],
    });
    const out = path.join(dir, 'out.json');
    expect(mergeManifests([a], out, { keep: 2, since: 'v26.410.20710' })).toBe(true);
    const merged = JSON.parse(fs.readFileSync(out, 'utf8'));
    expect(merged.versions.map((v: { version: string }) => v.version)).toEqual([
      '26.410.20711',
      '26.410.20710',
    ]);
  });

  test('没有任何版本 → 失败（退出码 1 语义）', () => {
    const dir = tmpdir();
    const a = path.join(dir, 'a.json');
    writeJson(a, { versions: [] });
    expect(mergeManifests([a], path.join(dir, 'out.json'), { keep: 5, since: '' })).toBe(false);
  });

  test('损坏的清单被跳过，其它照常合并', () => {
    const dir = tmpdir();
    const broken = path.join(dir, 'broken.json');
    const good = path.join(dir, 'good.json');
    fs.writeFileSync(broken, '{not json');
    writeJson(good, { versions: [{ version: '26.410.20711', assets: {} }] });
    const out = path.join(dir, 'out.json');
    expect(mergeManifests([broken, good], out, { keep: 5, since: '' })).toBe(true);
    expect(JSON.parse(fs.readFileSync(out, 'utf8')).latest).toBe('26.410.20711');
  });

  test('版本键按 (base, stage, seq) 解析', () => {
    expect(versionKey('26.410.20712-rc.1')).toEqual([26, 410, 20712, 4, 1]);
    expect(versionKey('v26.410.20712')).toEqual([26, 410, 20712, 5, 0]);
    expect(versionKey('nonsense')).toEqual([0, 0, 0, 0, 0]);
  });
});

describe('平台裁剪（filter_manifest_platforms）', () => {
  test('只保留托管的平台，无匹配平台的版本被剔除', () => {
    const dir = tmpdir();
    const file = path.join(dir, 'manifest.json');
    writeJson(file, {
      schema: 1,
      latest: 'v2',
      versions: [
        { version: 'v2', assets: { 'linux-x86_64': { file: 'a.zip' }, riscv64: { file: 'b.zip' } } },
        { version: 'v1', assets: { riscv64: { file: 'c.zip' } } },
      ],
    });
    filterPlatforms(file, ['linux-x86_64']);
    const data = JSON.parse(fs.readFileSync(file, 'utf8'));
    expect(data.versions.length).toBe(1);
    expect(Object.keys(data.versions[0].assets)).toEqual(['linux-x86_64']);
    expect(fs.readFileSync(file, 'utf8').endsWith('\n')).toBe(true);
  });
});

describe('清单文本助手（复刻 grep/sed 语义）', () => {
  test('manifestFiles / 版本 / latest / tags', () => {
    const text = '{\n  "latest": "v2",\n  "versions": [\n    { "version": "v2", "assets": { "a": { "file": "a.zip" } } }\n  ]\n}\n';
    expect(manifestFiles(text)).toEqual(['a.zip']);
    expect(channelLatest(text)).toBe('v2');
    expect(channelVersions(text)).toBe('v2');
  });
});

describe('URL 通道化（channelize_urls）', () => {
  test('保留 origin、重建路径、丢弃 query/fragment', () => {
    expect(channelizeUrl('https://dl.example/upgrade/x/y.app.tar.gz?sig=1#frag', 'rc').url).toBe(
      'https://dl.example/upgrade/rc/y.app.tar.gz',
    );
  });

  test('相对 URL 不动', () => {
    expect(channelizeUrl('files/y.dmg', 'rc')).toEqual({ url: 'files/y.dmg', changed: false });
  });

  test('中文文件名按 UTF-8 百分号编码', () => {
    expect(channelizeUrl('https://dl.example/upgrade/%E8%BF%85%E9%9B%B7.dmg', 'stable').url).toBe(
      'https://dl.example/upgrade/stable/%E8%BF%85%E9%9B%B7.dmg',
    );
  });

  test('幂等：重复执行不叠加通道目录', () => {
    const once = channelizeUrl('https://h/upgrade/a.dmg', 'stable').url;
    expect(channelizeUrl(once, 'stable').url).toBe(once);
  });

  test('文件改写：platforms.*.url 与 zipUrl 都改，返回条数', () => {
    const dir = tmpdir();
    const file = path.join(dir, 'latest.json');
    writeJson(file, {
      version: '1',
      platforms: {
        'darwin-x86_64': { signature: 's', url: 'https://h/upgrade/x.app.tar.gz' },
        'windows-x86_64': { signature: 's', url: 'https://h/upgrade/x-setup.exe' },
      },
      zipUrl: 'https://h/upgrade/m.zip',
    });
    expect(channelizeFile(file, 'beta')).toBe(true);
    const data = JSON.parse(fs.readFileSync(file, 'utf8'));
    expect(data.platforms['darwin-x86_64'].url).toBe('https://h/upgrade/beta/x.app.tar.gz');
    expect(data.platforms['windows-x86_64'].url).toBe('https://h/upgrade/beta/x-setup.exe');
    expect(data.zipUrl).toBe('https://h/upgrade/beta/m.zip');
  });
});

describe('状态文件双 schema', () => {
  test('readChannels 容忍空行/坏行/非数字字节', () => {
    const dir = tmpdir();
    const file = path.join(dir, 'status.tsv');
    fs.writeFileSync(file, 'stable\tv1\t10\t20\tv1,v2\n\nbad\tv2\nrc\tv3\tx\t5\t\n');
    const channels = readChannels(file);
    expect(channels.stable).toEqual({
      latest: 'v1',
      tags: ['v1', 'v2'],
      bytes: 30,
      core_bytes: 10,
      upgrade_bytes: 20,
    });
    expect(channels.rc.core_bytes).toBe(0);
    expect(Object.keys(channels)).toEqual(['stable', 'bad', 'rc']);
  });

  test('兼容文件：无 channels、round 递增、字段齐全', () => {
    const dir = tmpdir();
    const compat = path.join(dir, 'mirror-status.json');
    const channels = {
      stable: { latest: 'v2', tags: ['v2', 'v1'], bytes: 3, core_bytes: 1, upgrade_bytes: 2 },
    };
    writeCompatStatus(compat, channels, { result: 'ok', detail: '', sinceTag: '', maxBytes: 100 });
    writeCompatStatus(compat, channels, { result: 'ok', detail: '', sinceTag: '', maxBytes: 100 });
    const doc = JSON.parse(fs.readFileSync(compat, 'utf8'));
    expect(doc.round).toBe(2);
    expect(doc.result).toBe('ok');
    expect(doc.detail).toBe('同步完成: v2,v1');
    expect(doc.bytes).toEqual({ core: 1, app: 2, total: 3, max: 100 });
    expect(doc.window).toEqual({ core: 2, app: 2 });
    expect('channels' in doc).toBe(false);
    expect(doc.first_ok_at).toBe(doc.checked_at);
  });

  test('buildStatusInput：只有 app 目录的通道也算存在，空通道不汇报', () => {
    const dir = tmpdir();
    const cfg = baseConfig(dir);
    fs.mkdirSync(path.join(cfg.upgradeDest, 'rc'), { recursive: true });
    writeJson(path.join(cfg.upgradeDest, 'rc', 'manifest.json'), {
      schema: 1,
      latest: '26.410.20713',
      versions: [{ version: '26.410.20713' }],
    });
    const channels = buildStatusInput(cfg);
    expect(Object.keys(channels)).toEqual(['rc']);
    expect(channels.rc.latest).toBe('26.410.20713');
  });
});

describe('通道选择（KEEP / SINCE_TAG / MAX_BYTES）', () => {
  function makeWork(assets: Record<string, string>): string {
    const work = tmpdir();
    for (const [tag, body] of Object.entries(assets)) {
      fs.writeFileSync(path.join(work, `assets.${tag}`), body);
    }
    return work;
  }

  test('SINCE_TAG 只过滤 stable，其它通道保留', () => {
    const dir = tmpdir();
    const cfg = { ...baseConfig(dir), sinceTag: 'v26.410.20711', keepRc: 3 };
    const work = makeWork({});
    const stable = selectTags(cfg, 'stable', { tags: ['v26.410.20710', 'v26.410.20709'], core: [], app: [], dl: [] }, work);
    expect(stable.core).toEqual([]);
    const rc = selectTags(cfg, 'rc', { tags: ['v26.410.20710-rc.1'], core: [], app: [], dl: [] }, work);
    expect(rc.core).toEqual(['v26.410.20710-rc.1']);
  });

  test('nightly 固定保留 1 个', () => {
    const dir = tmpdir();
    const cfg = baseConfig(dir);
    const work = makeWork({});
    const nightly = selectTags(
      cfg,
      'nightly',
      { tags: ['nightly'], core: [], app: [], dl: [] },
      work,
    );
    expect(nightly.core.length).toBe(1);
    expect(nightly.app.length).toBe(1);
  });

  test('MAX_BYTES 超限：从最旧版本收缩 core/app 窗口', () => {
    const dir = tmpdir();
    const cfg = { ...baseConfig(dir), maxBytes: 150, keepCoreStable: 3, keepAppStable: 3 };
    const work = makeWork({
      'v26.410.20712': 'linux-x86_64-v26.410.20712.zip\t100\n',
      'v26.410.20711': 'linux-x86_64-v26.410.20711.zip\t100\n',
      'v26.410.20710': 'linux-x86_64-v26.410.20710.zip\t100\n',
    });
    const selected = selectTags(
      cfg,
      'stable',
      { tags: ['v26.410.20712', 'v26.410.20711', 'v26.410.20710'], core: [], app: [], dl: [] },
      work,
    );
    expect(selected.core.length).toBe(1);
    expect(selected.core[0]).toBe('v26.410.20712');
  });

  test('MAX_BYTES=0 不裁剪', () => {
    const dir = tmpdir();
    const cfg = { ...baseConfig(dir), maxBytes: 0, keepCoreStable: 2 };
    const work = makeWork({ 'v26.410.20712': 'linux-x86_64-v26.410.20712.zip\t100\n' });
    const selected = selectTags(
      cfg,
      'stable',
      { tags: ['v26.410.20712', 'v26.410.20711'], core: [], app: [], dl: [] },
      work,
    );
    expect(selected.core.length).toBe(2);
  });

  test('无资产元数据（--tags-file）时放弃预算裁剪', () => {
    const dir = tmpdir();
    const cfg = { ...baseConfig(dir), maxBytes: 1 };
    const work = makeWork({});
    const selected = selectTags(
      cfg,
      'stable',
      { tags: ['v26.410.20712', 'v26.410.20711'], core: [], app: [], dl: [] },
      work,
    );
    expect(selected.core.length).toBe(2);
  });
});
