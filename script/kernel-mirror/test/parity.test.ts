/**
 * 等价性验收：**同一份假 Release 输入**分别喂给旧脚本（shell + python）与新入口（Bun），
 * 产出的 dist 树必须逐字节一致 —— 这是「行为完全等价」最强的证明。
 *
 * 唯一被归一化的是时间戳（`updated` / `checked_at` / …）：两边跑的时刻天然不同。
 * 其余一切（文件集合、文件内容、符号链接目标、清单/状态的 JSON 排版、`.sync-state` 指纹、
 * `bytes.*` 统计）都必须完全一致，否则这里会给出具体差异。
 */

import { afterAll, beforeAll, describe, expect, setDefaultTimeout, test } from 'bun:test';
import fs from 'node:fs';
import path from 'node:path';
import { Sandbox, listTree, readText, runLegacy, runNew } from './harness';

// 每个用例要跑 4~6 轮同步（含 legacy shell + python），默认 5s 不够
setDefaultTimeout(120_000);

const TS_FIELDS = ['updated', 'checked_at', 'first_ok_at', 'last_change_at', 'wait_since'];
// 两棵树的绝对路径天然不同（parity-a-legacy vs parity-a-new），归一化后再比
const PATH_FIELDS = ['dest', 'upgrade_dest'];

function normalizeStatus(text: string): string {
  let out = text;
  for (const field of TS_FIELDS) {
    out = out.replace(new RegExp(`("${field}":\\s*")[^"]*(")`), '$1<ts>$2');
  }
  for (const field of PATH_FIELDS) {
    out = out.replace(new RegExp(`("${field}":\\s*")[^"]*(")`), '$1<root>$2');
  }
  return out;
}

interface Diff {
  missingInNew: string[];
  missingInLegacy: string[];
  changed: string[];
}

function diffTrees(legacyRoot: string, newRoot: string): Diff {
  const legacy = listTree(legacyRoot);
  const fresh = listTree(newRoot);
  const legacyMap = new Map(legacy.map((entry) => [entry.path, entry]));
  const freshMap = new Map(fresh.map((entry) => [entry.path, entry]));

  const missingInNew = [...legacyMap.keys()].filter((p) => !freshMap.has(p)).sort();
  const missingInLegacy = [...freshMap.keys()].filter((p) => !legacyMap.has(p)).sort();

  const changed: string[] = [];
  for (const [rel, entry] of legacyMap) {
    const other = freshMap.get(rel);
    if (!other) continue;
    if (entry.type !== other.type) {
      changed.push(`${rel}: 类型 ${entry.type} != ${other.type}`);
      continue;
    }
    if (entry.type !== 'file') {
      if (entry.value !== other.value) changed.push(`${rel}: ${entry.type} ${entry.value} != ${other.value}`);
      continue;
    }
    if (rel.endsWith('mirror-status.json')) {
      const a = normalizeStatus(readText(path.join(legacyRoot, rel)));
      const b = normalizeStatus(readText(path.join(newRoot, rel)));
      if (a !== b) changed.push(`${rel}: 内容不同（已归一化时间戳）\n--- legacy ---\n${a}\n--- new ---\n${b}`);
      continue;
    }
    if (entry.value !== other.value) changed.push(`${rel}: 文件内容不同`);
  }
  return { missingInNew, missingInLegacy, changed };
}

function expectSameTree(legacyRoot: string, newRoot: string): void {
  const diff = diffTrees(legacyRoot, newRoot);
  expect({ missingInNew: diff.missingInNew, missingInLegacy: diff.missingInLegacy, changed: diff.changed }).toEqual({
    missingInNew: [],
    missingInLegacy: [],
    changed: [],
  });
}

let sandbox: Sandbox;
const rootOf = (name: string): string => path.join(sandbox.root, name);

beforeAll(() => {
  sandbox = new Sandbox();
  sandbox.buildRelease('26.410.20711');
  sandbox.buildRelease('26.410.20710');
});

afterAll(() => {
  sandbox.cleanup();
});

describe('新旧实现 dist 树等价', () => {
  test('首轮同步（两版）产出完全一致', async () => {
    const legacyDist = rootOf('parity-a-legacy');
    const newDist = rootOf('parity-a-new');
    const legacy = await runLegacy(sandbox, legacyDist);
    const fresh = await runNew(sandbox, newDist);
    expect(legacy.code).toBe(0);
    expect(fresh.code).toBe(0);
    expectSameTree(legacyDist, newDist);
  });

  test('FORCE=1 全量重下后仍完全一致', async () => {
    const legacyDist = rootOf('parity-a-legacy');
    const newDist = rootOf('parity-a-new');
    const legacy = await runLegacy(sandbox, legacyDist, { FORCE: '1' });
    const fresh = await runNew(sandbox, newDist, { FORCE: '1' });
    expect(legacy.code).toBe(0);
    expect(fresh.code).toBe(0);
    expectSameTree(legacyDist, newDist);
  });

  test('增量轮（只重下旧 tag）后仍完全一致', async () => {
    const legacyDist = rootOf('parity-a-legacy');
    const newDist = rootOf('parity-a-new');
    fs.writeFileSync(
      path.join(sandbox.assetDir('26.410.20710'), 'app-x86-release.apk'),
      'apk re-uploaded 26.410.20710\n',
    );
    sandbox.rebuildReleases();
    const legacy = await runLegacy(sandbox, legacyDist);
    const fresh = await runNew(sandbox, newDist);
    expect(legacy.code).toBe(0);
    expect(fresh.code).toBe(0);
    expectSameTree(legacyDist, newDist);
  });

  test('MAX_BYTES 预算裁剪场景产出完全一致', async () => {
    sandbox.buildRelease('26.410.20712');
    const perTag = readText(sandbox.releasesFile)
      .split('\n')
      .filter((line) => line.startsWith('A\tv26.410.20712\t'))
      .filter((line) => !line.split('\t')[2].endsWith('.json'))
      .reduce((sum, line) => sum + Number(line.split('\t')[3] ?? 0), 0);
    const maxBytes = String(Math.floor((perTag * 3) / 2));
    const legacyDist = rootOf('parity-d-legacy');
    const newDist = rootOf('parity-d-new');
    const legacy = await runLegacy(sandbox, legacyDist, { MAX_BYTES: maxBytes });
    const fresh = await runNew(sandbox, newDist, { MAX_BYTES: maxBytes });
    expect(legacy.code).toBe(0);
    expect(fresh.code).toBe(0);
    expectSameTree(legacyDist, newDist);
  });

  test('SINCE_TAG 只约束 stable（新旧一致）', async () => {
    const legacyDist = rootOf('parity-s-legacy');
    const newDist = rootOf('parity-s-new');
    const legacy = await runLegacy(sandbox, legacyDist, { SINCE_TAG: 'v26.410.20711' });
    const fresh = await runNew(sandbox, newDist, { SINCE_TAG: 'v26.410.20711' });
    expect(legacy.code).toBe(0);
    expect(fresh.code).toBe(0);
    expectSameTree(legacyDist, newDist);
  });
});
