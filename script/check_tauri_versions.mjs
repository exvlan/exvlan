#!/usr/bin/env node
// 校验 Tauri 的 npm 包与 Rust crate 的 major.minor 是否一致。
// Tauri CLI 在 build 时会因不一致直接失败（且发生在耗时构建的末端），
// 这里提前检查，快速给出可读原因。
//
// 运行：node script/check_tauri_versions.mjs   （需先 pnpm install）
import fs from 'node:fs'
import path from 'node:path'

const root = process.cwd()
const lockPath = path.join(root, 'Cargo.lock')
if (!fs.existsSync(lockPath)) {
  console.error(`[tauri-check] 找不到 ${lockPath}`)
  process.exit(2)
}

const crateVersions = {}
const lock = fs.readFileSync(lockPath, 'utf8')
for (const match of lock.matchAll(/\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"/g)) {
  crateVersions[match[1]] = match[2]
}

/** crate 名 → npm 包名 */
const pairs = [
  ['tauri', '@tauri-apps/api'],
  ['tauri-plugin-shell', '@tauri-apps/plugin-shell'],
  ['tauri-plugin-clipboard-manager', '@tauri-apps/plugin-clipboard-manager'],
  ['tauri-plugin-os', '@tauri-apps/plugin-os'],
  ['tauri-plugin-process', '@tauri-apps/plugin-process'],
]

const minor = version => version.split('.').slice(0, 2).join('.')

let mismatched = 0
let checked = 0
for (const [crate, pkg] of pairs) {
  const crateVersion = crateVersions[crate]
  if (!crateVersion) continue

  const pkgJsonPath = path.join(root, 'packages/studio/node_modules', pkg, 'package.json')
  if (!fs.existsSync(pkgJsonPath)) {
    console.log(`[tauri-check] 跳过 ${pkg}（未安装）`)
    continue
  }

  const npmVersion = JSON.parse(fs.readFileSync(pkgJsonPath, 'utf8')).version
  checked++
  if (minor(crateVersion) !== minor(npmVersion)) {
    console.error(
      `[tauri-check] 不匹配：${crate} ${crateVersion} ↔ ${pkg} ${npmVersion}` +
      `（要求同为 ${minor(crateVersion)}.x）`,
    )
    mismatched++
  }
  else {
    console.log(`[tauri-check] OK ${crate} ${crateVersion} ↔ ${pkg} ${npmVersion}`)
  }
}

if (mismatched > 0) {
  console.error(
    `\n[tauri-check] ${mismatched} 处不匹配。Tauri 要求 npm 包与 Rust crate 的 major.minor 一致：\n` +
    `  - 升 Rust：cargo update -p <crate>，并把对应 npm 包改到同 minor；\n` +
    `  - 或把 npm 包钉到与 Cargo.lock 相同的 minor（本仓使用精确版本 + taze -x 排除）。`,
  )
  process.exit(1)
}

console.log(`[tauri-check] 通过（${checked} 对）`)
