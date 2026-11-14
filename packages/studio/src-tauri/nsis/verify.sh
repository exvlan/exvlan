#!/usr/bin/env bash
# 验证 NSIS 钩子（在 Linux 上就能跑，不需要 Windows）。
#
# 为什么要有这个脚本：service-lifecycle.nsh 里有大量 NSIS 转义（`$$` = PowerShell 的 `$`、
# `$\"` = 引号），手写极易出错，而真机验证要 Windows。这里做三层：
#
#   1. 用 makensis 真编译（`-WX`：warning 当 error，任何未转义的 `$变量` 都会失败）；
#   2. 从产物里把**实际发出的 PowerShell 命令行**抠出来（harness 用 SetCompress off，
#      Unicode 字符串原样躺在 exe 里），避免「我以为是这段」；
#   3. 把抠出来的载荷交给 PowerShell 做语法解析 + 桩控制流断言
#      （需要 pwsh；没有就跳过第 3 层并打印 SKIP）。
#
# 用法：bash packages/studio/src-tauri/nsis/verify.sh
# 依赖：makensis（Debian: apt install nsis）、python3、可选 pwsh 7+。
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
hooks="$here/service-lifecycle.nsh"
[ -f "$hooks" ] || { echo "找不到 $hooks" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"

if ! command -v makensis >/dev/null 2>&1; then
  echo "SKIP: 没有 makensis（apt install nsis），只做了存在性检查" >&2
  exit 0
fi

# ---------------------------------------------------------------- 1. 真编译
cat > harness.nsi <<'NSI'
Unicode true
SetCompress off
Name "exvlan-nsis-hook-harness"
OutFile "hook-harness.exe"
InstallDir "$TEMP\exvlan-hook-harness"
RequestExecutionLevel admin
!include "LogicLib.nsh"
NSI
printf '!include "%s"\n' "$hooks" >> harness.nsi
cat >> harness.nsi <<'NSI'

!ifmacrondef NSIS_HOOK_PREINSTALL
  !error "NSIS_HOOK_PREINSTALL 未定义"
!endif
!ifmacrondef NSIS_HOOK_POSTINSTALL
  !error "NSIS_HOOK_POSTINSTALL 未定义"
!endif

Section "install"
  !insertmacro NSIS_HOOK_PREINSTALL
  !insertmacro NSIS_HOOK_POSTINSTALL
SectionEnd
NSI

if ! makensis -V4 -WX harness.nsi > makensis.log 2>&1; then
  echo "FAIL: makensis 编译失败" >&2
  tail -20 makensis.log >&2
  exit 1
fi
echo "PASS  编译：makensis -WX 无 warning（$(sed -n 's/^MakeNSIS v/nsis /p' makensis.log | head -1)）"

# ---------------------------------------------------------------- 2 + 3. 抽载荷 + 断言
cat > check.py <<'PY'
import pathlib
import re
import shutil
import subprocess
import sys

blob = pathlib.Path("hook-harness.exe").read_bytes().decode("utf-16-le", errors="ignore")
pat = re.compile(
    r'\\System32\\WindowsPowerShell\\v1\.0\\powershell\.exe" '
    r'-NoProfile -NonInteractive -Command "([^"]*)"'
)
found = pat.findall(blob)
if len(found) != 2:
    sys.exit(f"FAIL: 期望 2 条 PowerShell 命令行，实际 {len(found)}")
for name, script in zip(("preinstall.ps1", "postinstall.ps1"), found):
    if "$$" in script or '$"' in script:
        sys.exit(f"FAIL: {name} 里还有未展开的 NSIS 转义")
    if "exit 0" not in script:
        sys.exit(f"FAIL: {name} 缺 exit 0（失败会阻塞安装）")
    pathlib.Path(name).write_text(script, encoding="utf-8")
    print(f"PASS  抽取：{name}（{len(script)} 字符，含 exit 0）")

pwsh = shutil.which("pwsh")
if not pwsh:
    print("SKIP  PowerShell 层：没有 pwsh（第 1、2 层已过）")
    sys.exit(0)

# 语法
for f in ("preinstall.ps1", "postinstall.ps1"):
    r = subprocess.run(
        [pwsh, "-NoProfile", "-Command",
         f'$e=$null; [void][System.Management.Automation.Language.Parser]::ParseInput('
         f'(Get-Content -Raw "{f}"), [ref]$null, [ref]$e); if ($e.Count) {{ $e[0].Message; exit 1 }}'],
        capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"FAIL: {f} 语法错误：{r.stdout}{r.stderr}")
    print(f"PASS  语法：{f} PARSE-OK")

SVC = "era.exvlan.studio.service"
ORPHANS = "Get-Process:exvlan+exvlan-gateway,Stop-Process:exvlan,Stop-Process:exvlan-gateway"
CASES = [
    # (载荷, 场景, 期望调用序列, 期望日志片段)
    ("preinstall.ps1", "svc-absent", f"Get-Service:{SVC},{ORPHANS}", "stopped=True"),
    ("preinstall.ps1", "svc-stopped", f"Get-Service:{SVC},{ORPHANS}", "stopped=True"),
    ("preinstall.ps1", "svc-running", f"Get-Service:{SVC},Stop-Service:{SVC},WaitForStatus:Stopped,{ORPHANS}", "stopped=True"),
    # 停不下来时**必须不清孤儿**，否则和 5s 看门狗打架
    ("preinstall.ps1", "svc-timeout", f"Get-Service:{SVC},Stop-Service:{SVC},WaitForStatus:Stopped", "stopped=False"),
    ("postinstall.ps1", "svc-absent", f"Get-Service:{SVC}", "before=absent started=False"),
    ("postinstall.ps1", "svc-running", f"Get-Service:{SVC}", "before=Running started=False"),
    ("postinstall.ps1", "svc-start-pending", f"Get-Service:{SVC},Start-Service:{SVC},WaitForStatus:Running", "before=Stopped started=True"),
]
for payload, scenario, want_calls, want_log in CASES:
    r = subprocess.run(
        [pwsh, "-NoProfile", "-File", "stub-run.ps1", "-Payload", payload, "-Scenario", scenario],
        capture_output=True, text=True)
    out = r.stdout.strip()
    got_calls = out.split("| LOG=")[0].removeprefix("CALLS=").strip()
    got_log = out.split("| LOG=")[-1].strip()
    if r.returncode != 0:
        sys.exit(f"FAIL {payload}/{scenario}: 退出码 {r.returncode}\n{r.stdout}{r.stderr}")
    if got_calls != want_calls:
        sys.exit(f"FAIL {payload}/{scenario}: 调用序列不符\n  期望 {want_calls}\n  实际 {got_calls}")
    if want_log not in got_log:
        sys.exit(f"FAIL {payload}/{scenario}: 日志不符（期望含 {want_log}）\n  实际 {got_log}")
    print(f"PASS  控制流：{payload}/{scenario} → {want_log}")
PY

cat > stub-run.ps1 <<'PS'
param([Parameter(Mandatory)][string]$Payload, [Parameter(Mandatory)][string]$Scenario)

$global:Calls = [System.Collections.Generic.List[string]]::new()
$global:SvcStatus = switch ($Scenario) {
  'svc-absent'        { $null }
  'svc-stopped'       { 'Stopped' }
  'svc-running'       { 'Running' }
  'svc-timeout'       { 'Running' }
  'svc-start-pending' { 'Stopped' }
  default             { throw "unknown scenario $Scenario" }
}
$global:WaitThrows = $Scenario -eq 'svc-timeout'

function Get-Service {
  [CmdletBinding()] param([string]$Name)
  $global:Calls.Add("Get-Service:$Name")
  if ($null -eq $global:SvcStatus) { return $null }
  $fake = [pscustomobject]@{ Name = $Name; Status = $global:SvcStatus }
  $fake | Add-Member -MemberType ScriptMethod -Name WaitForStatus -Value {
    param($target, $timeout)
    $global:Calls.Add("WaitForStatus:$target")
    if ($global:WaitThrows) { throw '等待超时（桩）' }
    $this.Status = $target
  }
  return $fake
}
function Stop-Service {
  [CmdletBinding()] param([string]$Name, [switch]$Force)
  $global:Calls.Add("Stop-Service:$Name")
}
function Start-Service {
  [CmdletBinding()] param([string]$Name)
  $global:Calls.Add("Start-Service:$Name")
}
function Get-Process {
  [CmdletBinding()] param([string[]]$Name)
  $global:Calls.Add("Get-Process:$($Name -join '+')")
  foreach ($n in $Name) { [pscustomobject]@{ Name = $n } }
}
function Stop-Process {
  [CmdletBinding()] param([Parameter(ValueFromPipeline)]$InputObject, [switch]$Force)
  process { $global:Calls.Add("Stop-Process:$($InputObject.Name)") }
}

# 钩子里的 $env:TEMP 在 Windows 上一定有；Linux 版 pwsh 没有，补一个。
if (-not $env:TEMP) { $env:TEMP = [System.IO.Path]::GetTempPath() }
$log = Join-Path $env:TEMP 'exvlan-nsis-hooks.log'
Remove-Item -LiteralPath $log -ErrorAction SilentlyContinue

try {
  Invoke-Expression (Get-Content -Raw -LiteralPath $Payload)
} finally {
  $line = if (Test-Path -LiteralPath $log) { (Get-Content -LiteralPath $log | Select-Object -Last 1) } else { '<无日志>' }
  Write-Output ("CALLS=" + ($global:Calls -join ',') + " | LOG=$line")
}
PS

python3 check.py
