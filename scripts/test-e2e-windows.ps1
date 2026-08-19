$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$dataDir = Join-Path $root '.apogee-e2e'
$appPath = Join-Path $root 'target\debug\apogee.exe'
$viteLog = Join-Path $dataDir 'vite.log'
$appStdout = Join-Path $dataDir 'apogee.stdout.log'
$appStderr = Join-Path $dataDir 'apogee.stderr.log'
$webViewDataDir = Join-Path $dataDir 'webview2'
$vite = $null
$app = $null

function Stop-ProcessTree([System.Diagnostics.Process]$process) {
  if ($null -eq $process -or $process.HasExited) { return }
  & taskkill /PID $process.Id /T /F | Out-Null
}

function Get-Log([string]$path, [string]$emptyMessage) {
  if (-not (Test-Path -LiteralPath $path)) { return $emptyMessage }
  return Get-Content -LiteralPath $path -Raw
}

function Get-ViteLog {
  return Get-Log $viteLog '(Vite 未写入日志)'
}

function Get-AppLogs {
  $stdout = Get-Log $appStdout '(应用未写入 stdout)'
  $stderr = Get-Log $appStderr '(应用未写入 stderr)'
  return "stdout:`n$stdout`nstderr:`n$stderr"
}

function Wait-ForVite([System.Diagnostics.Process]$process) {
  $deadline = [DateTime]::UtcNow.AddSeconds(30)
  $lastError = $null
  do {
    if ($process.HasExited) {
      throw "Vite 提前退出（exit=$($process.ExitCode)）。日志：`n$(Get-ViteLog)"
    }
    try {
      $response = Invoke-WebRequest -UseBasicParsing 'http://localhost:5183/' -TimeoutSec 2
      if ($response.StatusCode -eq 200) { return }
      $lastError = "HTTP $($response.StatusCode)"
    } catch {
      $lastError = $_.Exception.Message
    }
    Start-Sleep -Milliseconds 250
  } while ([DateTime]::UtcNow -lt $deadline)

  throw "Vite 未在 30 秒内就绪。最后错误：$lastError。日志：`n$(Get-ViteLog)"
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
  New-Item -ItemType Directory -Path $webViewDataDir | Out-Null

  # Tauri 的 devUrl 是 localhost:5183；使用同一地址并启用 strictPort，避免端口冲突时
  # Vite 静默切换到别的端口、E2E 却持续轮询旧端口。
  $viteCommand = "pnpm exec vite --host localhost --port 5183 --strictPort 1> `"$viteLog`" 2>&1"
  $vite = Start-Process -FilePath 'cmd.exe' -ArgumentList @('/d', '/s', '/c', $viteCommand) -WorkingDirectory $root -WindowStyle Hidden -PassThru
  Wait-ForVite $vite

  # 仅由 Tauri Context 在启动前识别的测试开关；正常运行不设置，绝不暴露 CDP。
  # WebView2 数据目录同样隔离，避免与用户已打开的 Apogee 实例共享环境而拒绝创建 WebView。
  $env:APOGEE_DATA_DIR = $dataDir
  $env:APOGEE_E2E_CDP_PORT = '9225'
  $env:WEBVIEW2_USER_DATA_FOLDER = $webViewDataDir
  $app = Start-Process -FilePath $appPath -WorkingDirectory $root -WindowStyle Hidden -RedirectStandardOutput $appStdout -RedirectStandardError $appStderr -PassThru
  $env:APOGEE_E2E_EXTERNAL = '1'
  $env:APOGEE_E2E_APP_PID = $app.Id
  & node (Join-Path $root 'scripts\e2e-workspace.mjs')
  if ($LASTEXITCODE -ne 0) {
    $status = if ($app.HasExited) { "应用已退出（exit=$($app.ExitCode)）" } else { "应用仍在运行（PID=$($app.Id)）" }
    throw "E2E 失败，exit=$LASTEXITCODE；$status。应用日志：`n$(Get-AppLogs)"
  }
} finally {
  Stop-ProcessTree $app
  Stop-ProcessTree $vite
  if (Test-Path -LiteralPath $dataDir) {
    $resolved = (Resolve-Path -LiteralPath $dataDir).Path
    if ($resolved -eq (Join-Path $root '.apogee-e2e')) { Remove-Item -LiteralPath $resolved -Recurse -Force }
  }
}
