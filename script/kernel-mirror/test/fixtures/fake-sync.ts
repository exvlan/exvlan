#!/usr/bin/env bun
/**
 * trigger 服务测试用的「假同步」：可配置耗时/结果，会把 token 打进输出
 * （验证 trigger 回传 detail 时是否脱敏），并按需写一份兼容状态文件。
 */

import fs from 'node:fs';

const delayMs = Number(process.env.FAKE_SYNC_DELAY_MS ?? '300');
const exitCode = Number(process.env.FAKE_SYNC_EXIT ?? '0');
const result = process.env.FAKE_SYNC_RESULT ?? 'ok';

process.stdout.write('[sync] 假同步开始\n');
process.stdout.write(`[sync] 凭据泄露测试：Authorization: Bearer ${process.env.MIRROR_TRIGGER_TOKEN ?? ''}\n`);
await Bun.sleep(delayMs);

const compat = process.env.MIRROR_STATUS_COMPAT;
if (compat) {
  const doc = {
    checked_at: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
    round: 1,
    result,
    detail: '假同步完成',
  };
  fs.writeFileSync(compat, `${JSON.stringify(doc, null, 2)}\n`);
}

process.stdout.write('[sync] 假同步结束\n');
process.exit(exitCode);
