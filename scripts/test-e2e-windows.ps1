$ErrorActionPreference = 'Stop'

$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$dataDir = Join-Path $root '.apogee-e2e'
$appPath = Join-Path $root 'target\debug\apogee.exe'
$vite = $null
$app = $null

function Stop-ProcessTree([System.Diagnostics.Process]$process) {
  if ($null -eq $process -or $process.HasExited) { return }
  & taskkill /PID $process.Id /T /F | Out-Null
}

try {
  if (-not (Test-Path -LiteralPath $appPath)) {
    throw "缺少 debug 应用：$appPath。请先运行 cargo build -p apogee-desktop。"
  }
  if (Test-Path -LiteralPath $dataDir) {
    $resolved = (Resolve-Path -LiteralPath $dataDir).Path
    if ($resolved -ne (Join-Path $root '.apogee-e2e')) { throw "拒绝清理意外目录：$resolved" }
    Remove-Item -LiteralPath $resolved -Recurse -Force
  }
  New-Item -ItemType Directory -Path $dataDir | Out-Null

  $vite = Start-Process 'cmd.exe' -ArgumentList '/c','pnpm dev --host 127.0.0.1 --port 5183' -WorkingDirectory $root -WindowStyle Hidden -PassThru
  $deadline = [DateTime]::UtcNow.AddSeconds(30)
  do {
    try {
      $response = Invoke-WebRequest -UseBasicParsing 'http://127.0.0.1:5183/' -TimeoutSec 2
      if ($response.StatusCode -eq 200) { break }
    } catch {}
    Start-Sleep -Milliseconds 250
  } while ([DateTime]::UtcNow -lt $deadline)
  if ($null -eq $response -or $response.StatusCode -ne 200) { throw 'Vite 未在 30 秒内就绪' }

  $env:APOGEE_DATA_DIR = $dataDir
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9225'
  $app = Start-Process $appPath -WorkingDirectory $root -WindowStyle Hidden -PassThru
  $env:APOGEE_E2E_EXTERNAL = '1'
  & node (Join-Path $root 'scripts\e2e-workspace.mjs')
  if ($LASTEXITCODE -ne 0) { throw "E2E 失败，exit=$LASTEXITCODE" }
} finally {
  Stop-ProcessTree $app
  Stop-ProcessTree $vite
  if (Test-Path -LiteralPath $dataDir) {
    $resolved = (Resolve-Path -LiteralPath $dataDir).Path
    if ($resolved -eq (Join-Path $root '.apogee-e2e')) { Remove-Item -LiteralPath $resolved -Recurse -Force }
  }
}
