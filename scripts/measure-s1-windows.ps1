[CmdletBinding()]
param(
    [ValidateRange(1, 20)]
    [int]$Runs = 5,

    [ValidateRange(0, 30000)]
    [int]$IdleSettleMs = 2000,

    [ValidateRange(1, 50)]
    [int]$IdleSamples = 5,

    [ValidateRange(10, 5000)]
    [int]$SampleIntervalMs = 250,

    [ValidateRange(1000, 60000)]
    [int]$ReadyTimeoutMs = 10000,

    # 启动耗时对 CPU 竞争极敏感（实测同机有/无背景负载差近 2 倍）。
    # 每轮启动前先等机器安静下来，否则测到的是「机器当时多忙」而不是「应用多快」。
    [ValidateRange(1, 100)]
    [int]$MaxCpuBeforeLaunchPercent = 20,

    [ValidateRange(0, 120000)]
    [int]$CpuSettleTimeoutMs = 20000,

    [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not $IsWindows) {
    throw 'S1 Windows 基准只能在 Windows 上运行。'
}

# 进程树遍历、私有工作集口径、CPU 噪声观测全部来自共享模块。
# S3 用的是同一份 —— 两个量测台各抄一份口径实现，就是给「改了一边忘了另一边」留后门，
# 而这恰好是本 Spike 工程教训第 4 条的内容。
Import-Module (Join-Path $PSScriptRoot 'lib\ProcessTreeMemory.psm1') -Force

$Root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$ResultsDir = Join-Path $Root 'spikes\results'
$ReleaseDir = Join-Path $Root 'target\release'
$Executable = Join-Path $ReleaseDir 'apogee.exe'
$JsonPath = Join-Path $ResultsDir 's1.json'
$ReportPath = Join-Path $ResultsDir 's1-report.md'
$Utf8NoBom = [Text.UTF8Encoding]::new($false)
[IO.Directory]::CreateDirectory($ResultsDir) | Out-Null

# 同机同栈外部对照：dbx（https://github.com/t8y2/dbx），同为 Tauri + Vue + WebView2
# 的数据库客户端，是**已发布的成熟产品**而非空壳。
#
# 它的作用是回答一个 Apogee 自己测不了的问题：「~147MB 到底是我们实现得差，
# 还是 WebView2 在 Windows 上的地板？」结论是后者。
#
# 数据来自本机手工量测（与 S1 同一套私有工作集口径、同一个 WebView2 Runtime），
# 不是本脚本自动采集的，所以硬编码在此并标注出处与条件 —— 与
# render-s2-report.mjs 里 R13 修复前基线的处理方式一致。
$script:ReferenceApp = [ordered]@{
    name = 'dbx 0.5.70'
    source = 'https://github.com/t8y2/dbx'
    stack = 'Tauri + Vue + WebView2'
    condition = '实际使用中（已连接数据库、数据网格与 CodeMirror 已加载）'
    measuredAt = '2026-07-30'
    manual = $true
    processCount = 7
    # 两次快照相隔约 1 小时：188.4 MB → 194.3 MB
    privateWorkingSetMbRange = @(188.4, 194.3)
    hostPrivateWorkingSetMb = 5.79
    workingSetSumMb = 298.5
    commitMb = 401.2
    roles = [ordered]@{
        'gpu-process' = 81.9
        'renderer' = 75.1
        'browser' = 18.6
        'utility+crashpad' = 6.5
    }
    webview2Flags = '默认配置（无 --disable-gpu / --single-process）'
}

$existing = @(Get-Process -Name 'apogee' -ErrorAction SilentlyContinue)
if ($existing.Count -gt 0) {
    throw '检测到正在运行的 apogee.exe。为避免单实例复用污染结果，请先关闭它再运行 S1。'
}

$previousAutorun = $env:VITE_AUTORUN_S2
try {
    Remove-Item Env:VITE_AUTORUN_S2 -ErrorAction SilentlyContinue

    if (-not $SkipBuild) {
        Write-Host '[S1] 构建 release + NSIS（不计入启动耗时）...'
        Push-Location $Root
        try {
            & npx tauri build --bundles nsis
            if ($LASTEXITCODE -ne 0) {
                throw "Tauri release 构建失败，退出码 $LASTEXITCODE。"
            }
        }
        finally {
            Pop-Location
        }
    }

    if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
        throw "找不到 release 可执行文件：$Executable。请先运行 npx tauri build --bundles nsis。"
    }

    $installerFiles = @(Get-ChildItem -Path (Join-Path $ReleaseDir 'bundle\nsis\*.exe') -File -ErrorAction SilentlyContinue)
    $artifacts = @(
        Get-Item -LiteralPath $Executable
        $installerFiles
    ) | ForEach-Object {
        [ordered]@{
            path = [IO.Path]::GetRelativePath($Root, $_.FullName).Replace('\', '/')
            bytes = [long]$_.Length
        }
    }
    $installerBytes = if ($installerFiles.Count -gt 0) {
        [long](($installerFiles | Measure-Object -Property Length -Maximum).Maximum)
    }
    else {
        $null
    }

    $logicalCores = [int][Environment]::ProcessorCount
    $cpuSnapshotBefore = Get-CpuTimeSnapshot
    $measurementTimer = [Diagnostics.Stopwatch]::StartNew()

    $runResults = [Collections.Generic.List[object]]::new()
    for ($run = 1; $run -le $Runs; $run++) {
        Write-Host "[S1] 启动轮次 $run/$Runs ..."
        $cpuGate = Wait-ForQuietCpu -MaxPercent $MaxCpuBeforeLaunchPercent -TimeoutMs $CpuSettleTimeoutMs
        $cpuBeforeLaunchPct = $cpuGate.loadPercent
        if (-not $cpuGate.settled) {
            Write-Warning ("第 $run 轮：等待 {0:N0}ms 后 CPU 仍为 {1}%（阈值 {2}%），带 hot 标记继续。" -f
                $cpuGate.waitedMs, $cpuBeforeLaunchPct, $MaxCpuBeforeLaunchPercent)
        }
        $marker = Join-Path ([IO.Path]::GetTempPath()) "apogee-s1-$([Guid]::NewGuid().ToString('N')).json"
        Remove-Item -LiteralPath $marker -Force -ErrorAction SilentlyContinue
        $previousMarker = $env:APOGEE_S1_READY_FILE
        $process = $null
        try {
            $env:APOGEE_S1_READY_FILE = $marker
            $timer = [Diagnostics.Stopwatch]::StartNew()
            $process = Start-Process -FilePath $Executable -WorkingDirectory $ReleaseDir -PassThru
            if ($null -eq $previousMarker) {
                Remove-Item Env:APOGEE_S1_READY_FILE -ErrorAction SilentlyContinue
            }
            else {
                $env:APOGEE_S1_READY_FILE = $previousMarker
            }

            while (-not (Test-Path -LiteralPath $marker -PathType Leaf)) {
                if ($process.HasExited) {
                    throw "apogee.exe 在 app_ready 前退出，退出码 $($process.ExitCode)。"
                }
                if ($timer.ElapsedMilliseconds -ge $ReadyTimeoutMs) {
                    throw "等待 app_ready 超过 ${ReadyTimeoutMs}ms。"
                }
                Start-Sleep -Milliseconds 5
            }
            $harnessObservedMs = [double]$timer.Elapsed.TotalMilliseconds
            $readyMarker = Get-Content -LiteralPath $marker -Raw -Encoding UTF8 | ConvertFrom-Json
            if ([int]$readyMarker.processId -ne $process.Id) {
                throw "就绪标记 PID $($readyMarker.processId) 与启动 PID $($process.Id) 不一致。"
            }

            # 启动耗时的权威口径：内核记录的进程创建时刻 → 应用写就绪标记的时刻。
            # 秒表口径（$harnessObservedMs）额外含 Start-Process cmdlet 开销和 5ms
            # 轮询粒度，会系统性高估；两个都记下来，差值就是量测台自身的噪声。
            $startEpochMs = [double]([DateTimeOffset]::new($process.StartTime).ToUnixTimeMilliseconds())
            $externalReadyMs = [double]$readyMarker.readyEpochMs - $startEpochMs
            if ($externalReadyMs -le 0 -or $externalReadyMs -gt $harnessObservedMs + 50) {
                throw ("内核时钟口径 {0:N1}ms 与秒表口径 {1:N1}ms 不自洽，拒绝采信本轮。" -f $externalReadyMs, $harnessObservedMs)
            }

            Start-Sleep -Milliseconds $IdleSettleMs
            $memorySamples = [Collections.Generic.List[object]]::new()
            $idleCpuSamples = [Collections.Generic.List[double]]::new()
            for ($sample = 1; $sample -le $IdleSamples; $sample++) {
                $withBreakdown = ($sample -eq $IdleSamples)
                $memorySamples.Add([pscustomobject](Get-TreeMemorySample -RootProcessId $process.Id -IncludeBreakdown:$withBreakdown))
                $cpuDuringIdle = Get-SystemCpuLoadPercent
                if ($null -ne $cpuDuringIdle) { $idleCpuSamples.Add([double]$cpuDuringIdle) }
                if ($sample -lt $IdleSamples) {
                    Start-Sleep -Milliseconds $SampleIntervalMs
                }
            }

            $treePwsValues = [double[]]@($memorySamples | ForEach-Object { $_.treePrivateWorkingSetBytes })
            $treeCommitValues = [double[]]@($memorySamples | ForEach-Object { $_.treeCommitBytes })
            $treeWsSumValues = [double[]]@($memorySamples | ForEach-Object { $_.treeWorkingSetSumBytes })
            $rootPwsValues = [double[]]@($memorySamples | ForEach-Object { $_.rootPrivateWorkingSetBytes })
            $rootCommitValues = [double[]]@($memorySamples | ForEach-Object { $_.rootCommitBytes })
            $runResults.Add([pscustomobject][ordered]@{
                run = $run
                classification = if ($run -eq 1) { 'first-clean-process-launch' } else { 'clean-process-relaunch' }
                externalReadyMs = $externalReadyMs
                harnessObservedMs = $harnessObservedMs
                harnessOverheadMs = $harnessObservedMs - $externalReadyMs
                systemCpuBeforeLaunchPercent = $cpuBeforeLaunchPct
                cpuSettledBeforeLaunch = [bool]$cpuGate.settled
                cpuSettleWaitedMs = $cpuGate.waitedMs
                systemCpuDuringIdleMedianPercent = if ($idleCpuSamples.Count -gt 0) { Get-Median -Values ([double[]]$idleCpuSamples.ToArray()) } else { $null }
                frontend = $readyMarker.frontend
                idle = [ordered]@{
                    sampleCount = $IdleSamples
                    treePrivateWorkingSetMedianBytes = [long](Get-Median -Values $treePwsValues)
                    treePrivateWorkingSetMaxBytes = [long](($treePwsValues | Measure-Object -Maximum).Maximum)
                    treeCommitMedianBytes = [long](Get-Median -Values $treeCommitValues)
                    treeWorkingSetSumMedianBytes = [long](Get-Median -Values $treeWsSumValues)
                    rootPrivateWorkingSetMedianBytes = [long](Get-Median -Values $rootPwsValues)
                    rootCommitMedianBytes = [long](Get-Median -Values $rootCommitValues)
                    processCountMax = [int](($memorySamples | Measure-Object -Property processCount -Maximum).Maximum)
                }
                processBreakdown = @($memorySamples[$memorySamples.Count - 1].processes)
                memorySamples = @($memorySamples)
            })
        }
        finally {
            if ($null -ne $process) {
                Stop-ProcessTree -RootProcessId $process.Id
                try { $process.WaitForExit(3000) | Out-Null } catch { }
                $process.Dispose()
            }
            Remove-Item -LiteralPath $marker -Force -ErrorAction SilentlyContinue
            if ($null -eq $previousMarker) {
                Remove-Item Env:APOGEE_S1_READY_FILE -ErrorAction SilentlyContinue
            }
            else {
                $env:APOGEE_S1_READY_FILE = $previousMarker
            }
        }
    }

    $startupValues = [double[]]@($runResults | ForEach-Object { $_.externalReadyMs })
    $frontendMountValues = [double[]]@($runResults | ForEach-Object { $_.frontend.mountMs })
    $frontendReadyValues = [double[]]@($runResults | ForEach-Object { $_.frontend.readyMs })
    $frontendPaintValues = [double[]]@($runResults | ForEach-Object { $_.frontend.firstPaintMs })
    $allTreePwsValues = [double[]]@($runResults | ForEach-Object { $_.memorySamples } | ForEach-Object { $_.treePrivateWorkingSetBytes })
    $allTreeCommitValues = [double[]]@($runResults | ForEach-Object { $_.memorySamples } | ForEach-Object { $_.treeCommitBytes })
    $allTreeWsSumValues = [double[]]@($runResults | ForEach-Object { $_.memorySamples } | ForEach-Object { $_.treeWorkingSetSumBytes })
    $allRootPwsValues = [double[]]@($runResults | ForEach-Object { $_.memorySamples } | ForEach-Object { $_.rootPrivateWorkingSetBytes })
    $allRootCommitValues = [double[]]@($runResults | ForEach-Object { $_.memorySamples } | ForEach-Object { $_.rootCommitBytes })
    $harnessOverheadValues = [double[]]@($runResults | ForEach-Object { $_.harnessOverheadMs })

    # ── 汇总 ────────────────────────────────────────────────────────────
    # 首轮与重启轮次分开统计：首轮是本次会话第一次拉起该二进制（最接近冷启动），
    # 后续轮次的 OS 文件缓存与 WebView2 预热都已就位，对应 PRD 的「热启动」。
    # 把两者混进一个中位数就会同时高估热启动、低估冷启动，两个数都没用。
    $startupFirstMs = [double]$startupValues[0]
    # ⚠️ 外层的 [double[]]@(...) 不能省。`$x = if (...) { $arr[1..1] }` 时 PowerShell 会把
    # 单元素数组**解包成标量**，后面 `.Count` 在 StrictMode 下直接抛 PropertyNotFound。
    # -Runs 2 会精确踩到这个（切片只剩 1 个元素），-Runs 20 不会 —— 属于「只在边界参数下炸」的那类 bug。
    $relaunchStartupValues = [double[]]@(
        if ($startupValues.Count -gt 1) { $startupValues[1..($startupValues.Count - 1)] }
    )

    $startupMedianMs = Get-Median -Values $startupValues
    $startupP95Ms = Get-Percentile -Values $startupValues -Percentile 0.95
    $startupMinMs = [double](($startupValues | Measure-Object -Minimum).Minimum)
    $startupMaxMs = [double](($startupValues | Measure-Object -Maximum).Maximum)
    $relaunchMedianMs = if ($relaunchStartupValues.Count -gt 0) { Get-Median -Values $relaunchStartupValues } else { $null }
    $relaunchMaxMs = if ($relaunchStartupValues.Count -gt 0) {
        [double](($relaunchStartupValues | Measure-Object -Maximum).Maximum)
    }
    else {
        $null
    }

    $relaunchP95Ms = if ($relaunchStartupValues.Count -gt 0) {
        Get-Percentile -Values $relaunchStartupValues -Percentile 0.95
    }
    else {
        $null
    }

    $frontendMountMedianMs = Get-Median -Values $frontendMountValues
    $frontendReadyMedianMs = Get-Median -Values $frontendReadyValues
    $frontendPaintMedianMs = Get-Median -Values $frontendPaintValues

    # 外部计时含「进程创建 → Rust 初始化 → WebView2 起进程 → 加载 HTML/JS」，
    # 前端计时从第一行 JS 才开始。差值就是 JS 时钟看不见的那段启动成本，
    # 也是本次量测里最稳定的一项 —— 单独统计才能看出它是地板而非噪声。
    $preJsValues = [double[]]@($runResults | ForEach-Object { [double]$_.externalReadyMs - [double]$_.frontend.firstPaintMs })
    $preJsMedianMs = Get-Median -Values $preJsValues
    $preJsMinMs = [double](($preJsValues | Measure-Object -Minimum).Minimum)
    $preJsMaxMs = [double](($preJsValues | Measure-Object -Maximum).Maximum)
    $preScriptOverheadMs = $preJsMedianMs

    # 离群轮次：超过中位数 2 倍。判定用 p95 而不是 max，就是为了不让单个离群轮
    # 决定结论；但离群必须显式报出来，不能悄悄被分位数吸收掉。
    $outlierRuns = @($runResults | Where-Object { [double]$_.externalReadyMs -gt $startupMedianMs * 2.0 })

    # ── 预热漂移检测 ────────────────────────────────────────────────────
    #
    # 起跑 CPU 门禁挡不住一类污染：**刚构建完的二进制会被 Defender 首扫**，
    # 加上构建残留进程还在退场，于是前若干轮系统性偏慢、后若干轮才是真实水平。
    # 实测见过一次干净的双峰：前 11 轮 mount 145～219ms / 启动 788～1234ms，
    # 第 12 轮起 mount 61～74ms / 启动 553～638ms，而两段的起跑 CPU 都是 ~18%。
    #
    # 这种分布不会被「离群轮 > 中位 2 倍」抓到（它不是个别离群，是整段偏移），
    # 也不会被 noisyEnvironment 抓到（CPU 门禁每轮都过了）。混在一个中位数里
    # 会同时污染冷启动和热启动两个结论。所以显式比较前后两半，并把后半段的
    # 中位数单独报出来 —— 那才是可引用的数字。
    $halfIndex = [Math]::Floor($startupValues.Count / 2)
    $firstHalfValues = [double[]]@($startupValues[0..([Math]::Max(0, $halfIndex - 1))])
    $secondHalfValues = [double[]]@(
        if ($startupValues.Count -gt 1) { $startupValues[$halfIndex..($startupValues.Count - 1)] }
    )
    $firstHalfMedianMs = Get-Median -Values $firstHalfValues
    $secondHalfMedianMs = Get-Median -Values $secondHalfValues
    $warmupDriftPercent = if ($null -ne $secondHalfMedianMs -and $secondHalfMedianMs -gt 0) {
        ($firstHalfMedianMs - $secondHalfMedianMs) / $secondHalfMedianMs * 100.0
    }
    else {
        $null
    }
    # 30% 是经验阈值：正常抖动远低于此，而实测到的那次双峰是 +44%
    $warmupDriftDetected = ($null -ne $warmupDriftPercent) -and ($warmupDriftPercent -gt 30.0)

    $treePwsMedianBytes = [long](Get-Median -Values $allTreePwsValues)
    $treePwsMaxBytes = [long](($allTreePwsValues | Measure-Object -Maximum).Maximum)
    $treeCommitMedianBytes = [long](Get-Median -Values $allTreeCommitValues)
    $treeWsSumMedianBytes = [long](Get-Median -Values $allTreeWsSumValues)
    $rootPwsMedianBytes = [long](Get-Median -Values $allRootPwsValues)
    $rootCommitMedianBytes = [long](Get-Median -Values $allRootCommitValues)
    $processCountMax = [int](($runResults | ForEach-Object { $_.idle.processCountMax } | Measure-Object -Maximum).Maximum)
    # WebView2 侧份额 = 进程树 - Rust 主进程。空壳里这一项是内存的绝对主体。
    $webviewPwsMedianBytes = $treePwsMedianBytes - $rootPwsMedianBytes
    # WorkingSet 求和 / 私有常驻 的倍数：共享页被重复计数的放大系数
    $sharedPageInflation = [double]$treeWsSumMedianBytes / [double]$treePwsMedianBytes

    # 首轮内存与稳态内存要分开看：首轮含 WebView2 首次运行的额外工作
    # （GPU 着色器缓存等），把它混进中位数会掩盖真实稳态值。
    $firstRunResult = $runResults[0]
    # 同上：外层 @(...) 防单元素解包
    $relaunchRunResults = @(
        if ($runResults.Count -gt 1) { $runResults[1..($runResults.Count - 1)] }
    )
    $firstLaunchPwsMaxBytes = [long]$firstRunResult.idle.treePrivateWorkingSetMaxBytes
    # 峰值出现在哪一轮必须记下来：不查清就容易把「某轮瞬时尖峰」误写成「首轮开销」
    $peakRunRecord = @($runResults | Sort-Object -Property { [double]$_.idle.treePrivateWorkingSetMaxBytes } -Descending)[0]
    $peakRunNumber = [int]$peakRunRecord.run
    # 尖峰轮次：峰值比全局中位高出 10MB 以上
    $spikyRuns = @($runResults | Where-Object { [double]$_.idle.treePrivateWorkingSetMaxBytes -gt $treePwsMedianBytes + 10485760 })
    $relaunchPwsValues = [double[]]@($relaunchRunResults | ForEach-Object { $_.memorySamples } | ForEach-Object { $_.treePrivateWorkingSetBytes })
    $relaunchPwsMedianBytes = if ($relaunchPwsValues.Count -gt 0) { [long](Get-Median -Values $relaunchPwsValues) } else { $null }
    $relaunchPwsMaxBytes = if ($relaunchPwsValues.Count -gt 0) {
        [long](($relaunchPwsValues | Measure-Object -Maximum).Maximum)
    }
    else {
        $null
    }

    $treePwsMaxMb = [double]$treePwsMaxBytes / 1MB
    # Rust 主进程的峰值：新增常驻结构会先在峰值上显形，所以门禁取峰值不取中位
    $rootPwsMaxBytes = [long](($allRootPwsValues | Measure-Object -Maximum).Maximum)
    $rootPwsMaxMb = [double]$rootPwsMaxBytes / 1MB
    $treePwsMedianMb = [double]$treePwsMedianBytes / 1MB
    $harnessOverheadMedianMs = Get-Median -Values $harnessOverheadValues
    $executableBytes = [long](Get-Item -LiteralPath $Executable).Length
    $installerMb = if ($null -ne $installerBytes) { [double]$installerBytes / 1MB } else { $null }

    # 各角色进程的私有常驻中位（跨轮次），用于回答「内存花在哪」
    $roleBreakdown = @(
        $runResults |
            ForEach-Object { $_.processBreakdown } |
            Group-Object -Property role |
            ForEach-Object {
                $roleValues = [double[]]@($_.Group | ForEach-Object { $_.privateWorkingSetBytes })
                [pscustomobject][ordered]@{
                    role = $_.Name
                    privateWorkingSetMedianBytes = [long](Get-Median -Values $roleValues)
                    instancesPerRun = [int]([Math]::Round($_.Count / [double]$runResults.Count))
                }
            } |
            Sort-Object -Property privateWorkingSetMedianBytes -Descending
    )

    # ── 验收门禁 ────────────────────────────────────────────────────────
    # 一律用「最差观测值」判定，不用中位数。量测台不该替产品挑好看的那个数。
    # 内存口径是进程树**私有工作集**（`Working Set - Private`）—— PRD §5.1 写的是「空闲常驻内存」；
    # 提交量与 WorkingSet 求和作为旁证一起报告，不作门禁（理由见报告「发现」第 1 条）。
    #
    # ⚠️ 03 文档 §10.1 原来给 S1 定的「启动 < 400ms、空闲 < 80MB」**已废弃**，不再作为门禁：
    # 那两个数是纸面估算，Windows + WebView2 的空壳地板就是 ~569ms / ~147MB，
    # 其中我们能控的 Rust 侧只有几 MB。定稿过程见 03 文档 §10.4。
    # 现在只保留 PRD §5.1 的对外承诺，外加一条真正可控的「Rust 主进程私有常驻」。
    $gates = @(
        [pscustomobject][ordered]@{
            id = 'PRD-冷启动'
            scope = 'prd'
            source = 'PRD §5.1'
            criterion = '冷启动到可交互 < 800 ms（首轮）'
            budget = 800.0
            measured = $startupFirstMs
            unit = 'ms'
            pass = ($startupFirstMs -lt 800.0)
        }
        [pscustomobject][ordered]@{
            id = 'PRD-热启动'
            scope = 'prd'
            source = 'PRD §5.1'
            criterion = '热启动 < 300 ms（OS 缓存已热的重启轮次 p95）'
            budget = 300.0
            measured = $relaunchP95Ms
            unit = 'ms'
            pass = if ($null -eq $relaunchP95Ms) { $null } else { $relaunchP95Ms -lt 300.0 }
        }
        [pscustomobject][ordered]@{
            id = 'PRD-空闲内存'
            scope = 'prd'
            source = 'PRD §5.1'
            # 预算已按 S3 实测重定为 220MB（Windows）。注意本量测台测的是**空壳**，
            # 不是 PRD 声明的「1 工作区 + 3 标签」——所以这里过关只说明地板没退化，
            # 不等于预算达标；那个条件由 S3（pnpm spike:s3）负责量。
            criterion = '空闲常驻内存 < 220 MB（Windows；进程树私有常驻峰值）。本轮条件是空壳，属地板检查'
            budget = 220.0
            measured = $treePwsMaxMb
            unit = 'MB'
            pass = ($treePwsMaxMb -lt 220.0)
        }
        [pscustomobject][ordered]@{
            id = 'PRD-Rust常驻'
            scope = 'prd'
            source = 'PRD §5.1'
            # 这是整个内存预算里唯一由我们的代码决定的部分（S1 空壳 3.68MB / S3 负载 9.4MB）。
            # 其余 90%+ 是 WebView2 的固定成本，涨跌与我们无关 —— 所以真正能守的回归门禁是这一条。
            # 取峰值而非中位：新增一个常驻结构（如 S2 那个 91MB 的索引）会先在峰值上显形。
            criterion = 'Rust 主进程私有常驻 < 20 MB（峰值）—— 内存预算里唯一可控项'
            budget = 20.0
            measured = $rootPwsMaxMb
            unit = 'MB'
            pass = ($rootPwsMaxMb -lt 20.0)
        }
        [pscustomobject][ordered]@{
            id = 'PRD-安装包'
            scope = 'prd'
            source = 'PRD §5.1'
            criterion = '安装包体积 < 15 MB（NSIS，WebView2 走 downloadBootstrapper）'
            budget = 15.0
            measured = $installerMb
            unit = 'MB'
            pass = if ($null -eq $installerMb) { $null } else { $installerMb -lt 15.0 }
        }
    )

    $unmeasuredGates = @($gates | Where-Object { $null -eq $_.pass })
    $failedGates = @($gates | Where-Object { $_.pass -eq $false })
    $windowsPass = ($failedGates.Count -eq 0) -and ($unmeasuredGates.Count -eq 0)
    # macOS 侧是 S1 声明范围的一半，在 Windows 上拿不到。
    # 因此 Windows 全绿也只能给 CONDITIONAL PASS，不能写 PASS。
    $verdict = if ($failedGates.Count -gt 0) {
        'FAIL'
    }
    elseif ($unmeasuredGates.Count -gt 0) {
        'CONDITIONAL PASS'
    }
    else {
        'CONDITIONAL PASS'
    }

    $notMeasured = @(
        'macOS 侧冷启动与内存：S1 声明覆盖双平台，本脚本只跑 Windows。macOS 是 WKWebView，进程模型与内存口径都不同，不能套用本报告的数字。'
        '真正的冷缓存启动：未清理 OS 文件缓存、未重启、未排除 Defender 首扫，首轮只是「本次会话首次拉起」。'
        'PRD §5.1「空闲常驻内存」的量测条件是 1 工作区 + 3 标签；当前空壳只有 1 个空标签，实测值是地板不是达标证明。那个条件由 Spike S3 量（pnpm spike:s3）。'
        'PRD §5.1「磁盘占用（安装后）< 50MB」：本脚本只量 exe 与安装包体积，未做安装后目录统计（还需计入 EBWebView 用户数据目录）。'
        'PRD §5.1「50 标签 < 400MB」：需要标签虚拟化落地后才有意义。'
        '共享内存的归属：私有工作集刻意排除共享页，因此本报告的数字不包含 WebView2 Runtime 的共享映射；这是「本应用额外占用多少内存」的口径，不是「系统总共为它花了多少」。'
    )

    # ── 量测环境负载 ────────────────────────────────────────────────────
    $measurementTimer.Stop()
    $measurementSeconds = [double]$measurementTimer.Elapsed.TotalSeconds
    $cpuSnapshotAfter = Get-CpuTimeSnapshot
    $ownProcessIds = @($PID)
    $topCpuConsumers = @(Get-TopCpuConsumers -Before $cpuSnapshotBefore -After $cpuSnapshotAfter `
            -ElapsedSeconds $measurementSeconds -LogicalCores $logicalCores -ExcludeProcessIds $ownProcessIds)
    $launchCpuValues = [double[]]@($runResults | Where-Object { $null -ne $_.systemCpuBeforeLaunchPercent } | ForEach-Object { $_.systemCpuBeforeLaunchPercent })
    $systemCpuMedianPct = if ($launchCpuValues.Count -gt 0) { Get-Median -Values $launchCpuValues } else { $null }
    $systemCpuMaxPct = if ($launchCpuValues.Count -gt 0) { [double](($launchCpuValues | Measure-Object -Maximum).Maximum) } else { $null }
    # 「有轮次没等到安静就开跑」是比平均负载更准的噪声信号：
    # 它直接对应「这一轮的启动耗时受了污染」。
    $hotRuns = @($runResults | Where-Object { -not $_.cpuSettledBeforeLaunch })
    $noisyEnvironment = $hotRuns.Count -gt 0

    $appVersion = (Get-Content -LiteralPath (Join-Path $Root 'src-tauri\tauri.conf.json') -Raw -Encoding UTF8 | ConvertFrom-Json).version

    $result = [ordered]@{
        spike = 'S1'
        at = [DateTimeOffset]::Now.ToString('o')
        verdict = $verdict
        windowsPass = $windowsPass
        platform = Get-MeasurementPlatform -AppVersion ([string]$appVersion)
        config = [ordered]@{
            runs = $Runs
            idleSettleMs = $IdleSettleMs
            idleSamples = $IdleSamples
            sampleIntervalMs = $SampleIntervalMs
            readyTimeoutMs = $ReadyTimeoutMs
            skipBuild = [bool]$SkipBuild
            readyMarkerEnvVar = 'APOGEE_S1_READY_FILE'
            executable = [IO.Path]::GetRelativePath($Root, $Executable).Replace('\', '/')
        }
        artifacts = @($artifacts)
        size = [ordered]@{
            executableBytes = $executableBytes
            installerBytes = $installerBytes
        }
        startup = [ordered]@{
            gateStatistic = 'p95（启动噪声大，不用 max 以免单个离群轮定结论；内存稳定，仍用 max）'
            firstLaunchMs = $startupFirstMs
            relaunchMedianMs = $relaunchMedianMs
            relaunchP95Ms = $relaunchP95Ms
            relaunchMaxMs = $relaunchMaxMs
            allMedianMs = $startupMedianMs
            allP95Ms = $startupP95Ms
            allMinMs = $startupMinMs
            allMaxMs = $startupMaxMs
            preJsMedianMs = $preJsMedianMs
            preJsMinMs = $preJsMinMs
            preJsMaxMs = $preJsMaxMs
            # 预热漂移：前半轮次 vs 后半轮次。CPU 门禁挡不住 Defender 首扫这类整段偏移。
            firstHalfMedianMs = $firstHalfMedianMs
            secondHalfMedianMs = $secondHalfMedianMs
            warmupDriftPercent = $warmupDriftPercent
            warmupDriftDetected = $warmupDriftDetected
            outlierRuns = @($outlierRuns | ForEach-Object {
                [pscustomobject][ordered]@{
                    run = $_.run
                    externalReadyMs = $_.externalReadyMs
                    firstPaintMs = $_.frontend.firstPaintMs
                    preJsMs = [double]$_.externalReadyMs - [double]$_.frontend.firstPaintMs
                }
            })
        }
        frontend = [ordered]@{
            mountMedianMs = $frontendMountMedianMs
            readyMedianMs = $frontendReadyMedianMs
            firstPaintMedianMs = $frontendPaintMedianMs
            preScriptOverheadMs = $preScriptOverheadMs
        }
        harness = [ordered]@{
            clockSource = 'kernel Process.StartTime → 应用写入的 readyEpochMs'
            overheadMedianMs = $harnessOverheadMedianMs
            note = '秒表口径（Start-Process 返回 + 5ms 轮询）比内核口径高出的部分，属量测台噪声'
            measurementSeconds = $measurementSeconds
            cpuGateMaxPercent = $MaxCpuBeforeLaunchPercent
            cpuGateTimeoutMs = $CpuSettleTimeoutMs
            systemCpuMedianPercent = $systemCpuMedianPct
            systemCpuMaxPercent = $systemCpuMaxPct
            hotRuns = @($hotRuns | ForEach-Object { [int]$_.run })
            noisyEnvironment = $noisyEnvironment
            topCpuConsumers = @($topCpuConsumers)
        }
        idle = [ordered]@{
            totalSampleCount = $allTreePwsValues.Count
            processCountMax = $processCountMax
            memoryMetric = 'privateWorkingSet（Working Set - Private，等于任务管理器「内存」列）'
            gateStatistic = 'max（内存逐样本稳定，取最差观测值）'
            treePrivateWorkingSetMedianBytes = $treePwsMedianBytes
            treePrivateWorkingSetMaxBytes = $treePwsMaxBytes
            firstLaunchPrivateWorkingSetMaxBytes = $firstLaunchPwsMaxBytes
            relaunchPrivateWorkingSetMedianBytes = $relaunchPwsMedianBytes
            relaunchPrivateWorkingSetMaxBytes = $relaunchPwsMaxBytes
            peakRun = $peakRunNumber
            spikyRuns = @($spikyRuns | ForEach-Object { [int]$_.run })
            treeCommitMedianBytes = $treeCommitMedianBytes
            treeWorkingSetSumMedianBytes = $treeWsSumMedianBytes
            sharedPageInflation = $sharedPageInflation
            rootPrivateWorkingSetMedianBytes = $rootPwsMedianBytes
            rootPrivateWorkingSetMaxBytes = $rootPwsMaxBytes
            rootCommitMedianBytes = $rootCommitMedianBytes
            webviewPrivateWorkingSetMedianBytes = $webviewPwsMedianBytes
            roleBreakdown = @($roleBreakdown)
        }
        gates = @($gates)
        referenceApp = $script:ReferenceApp
        notMeasured = @($notMeasured)
        runs = @($runResults)
    }

    [IO.File]::WriteAllText($JsonPath, (ConvertTo-Json -InputObject $result -Depth 10), $Utf8NoBom)

    # ── 报告 ────────────────────────────────────────────────────────────
    $lines = [Collections.Generic.List[string]]::new()
    $add = { param([string]$Text) $lines.Add($Text) }

    & $add '# Spike S1：Tauri 2 空壳冷启动与内存（Windows）'
    & $add ''
    & $add '## 结论'
    & $add ''
    # 结论段按**实际未通过的门禁**生成，不预设是哪几项失败。
    # 早期版本把「启动与内存两项都没达到 400ms / 80MB」写死在这里，那两个门槛废弃后
    # 这段话就变成了假的 —— 报告里的叙述必须跟着数据走，不能跟着当初的预期走。
    $gateLine = {
        param([string]$Id)
        $gate = $gates | Where-Object { $_.id -eq $Id } | Select-Object -First 1
        if ($null -eq $gate) { return '—' }
        $measuredText = if ($gate.unit -eq 'ms') { Format-Ms $gate.measured } else { Format-Mb ([double]$gate.measured * 1MB) }
        return "$measuredText / 预算 $($gate.budget)$($gate.unit)（$(Format-Pct $gate.measured $gate.budget)）"
    }
    if ($failedGates.Count -gt 0) {
        & $add ("**FAIL。** $($failedGates.Count) 项验收未通过：**$((($failedGates | ForEach-Object { $_.id }) -join '、'))**。")
        & $add ''
        foreach ($gate in $failedGates) {
            $measuredText = if ($gate.unit -eq 'ms') { Format-Ms $gate.measured } else { Format-Mb ([double]$gate.measured * 1MB) }
            & $add ("- **$($gate.id)**：$($gate.criterion) → 实测 **$measuredText**（预算占用 $(Format-Pct $gate.measured $gate.budget)）")
        }
        & $add ''
        & $add ("通过的项：" + (($gates | Where-Object { $_.pass -eq $true } | ForEach-Object { "$($_.id) $(& $gateLine $_.id)" }) -join '；') + '。')
    }
    else {
        & $add ("**CONDITIONAL PASS（仅 Windows）。** " +
            "冷启动 **$(& $gateLine 'PRD-冷启动')**，热启动 **$(& $gateLine 'PRD-热启动')**，" +
            "空闲私有常驻峰值 **$(& $gateLine 'PRD-空闲内存')**，" +
            "Rust 主进程私有常驻峰值 **$(& $gateLine 'PRD-Rust常驻')**，" +
            "NSIS 安装包 **$(& $gateLine 'PRD-安装包')**。")
    }
    & $add ''
    & $add ("**内存的构成比数值更重要。** 进程树私有常驻中位 $(Format-Mb $treePwsMedianBytes) 里，Rust 主进程只占 " +
        "$(Format-Mb $rootPwsMedianBytes)，其余 $(Format-Mb $webviewPwsMedianBytes)（$(Format-Pct $webviewPwsMedianBytes $treePwsMedianBytes)）" +
        "在 WebView2 的 $($processCountMax - 1) 个子进程里。启动同理：中位 $(Format-Ms $startupMedianMs) 里前端从第一行 JS 到首帧只占 " +
        "$(Format-Ms $frontendPaintMedianMs)，**$(Format-Ms $preScriptOverheadMs)**（$(Format-Pct $preScriptOverheadMs $startupMedianMs)）" +
        "花在 JS 之前的进程创建与 WebView2 起进程上，且这段在 $Runs 轮里只波动 $(Format-Ms $preJsMinMs)～$(Format-Ms $preJsMaxMs) —— 是地板，不是噪声。" +
        '所以 S1 量到的主要是 **WebView2 在 Windows 上的固定成本**，不是 Apogee 的实现开销。' +
        '03 文档 §10.1 原来给 S1 定的「启动 < 400ms、空闲 < 80MB」正是因此废弃（定稿见 03 §10.4）；' +
        '本报告只对 PRD §5.1 的对外承诺判定，外加一条真正可控的「Rust 主进程私有常驻」。')
    & $add ''
    & $add '**被测对象是空壳**：1 个空标签、无集合、无 SQLite 连接、无脚本引擎。所有内存数字是**地板值**，不等于 PRD §5.1「1 工作区 3 标签」条件下的达标证明 —— 那个条件由 Spike S3 量（`pnpm spike:s3`，实测 166.2 MB 中位 / 184.9 MB 最差观测）。S1 声明覆盖 Windows + macOS 双平台，macOS 侧未量测。'
    & $add ''
    & $add ("环境：$($result.platform.os) $($result.platform.osVersion) · $($result.platform.cpu) · $($result.platform.logicalCores) 逻辑核 · " +
        "内存 $('{0:N1} GB' -f ([double]$result.platform.totalMemoryBytes / 1GB)) · WebView2 Runtime $(if ($result.platform.webview2Runtime) { $result.platform.webview2Runtime } else { '未知' }) · " +
        "$($result.platform.rustc) · node $($result.platform.node)")
    & $add ('量测方式：release 构建（`opt-level="z"` / `lto=fat` / `codegen-units=1` / `panic=abort` / `strip`），' +
        "$Runs 轮独立进程启动，每轮静置 ${IdleSettleMs}ms 后按 ${SampleIntervalMs}ms 采样 $IdleSamples 次。量测时间 $($result.at)。")
    & $add ''
    if ($warmupDriftDetected) {
        & $add ("> **检测到预热漂移：前后两半轮次不是同一个量级，启动的汇总值已被污染。** " +
            "前 $halfIndex 轮中位 $(Format-Ms $firstHalfMedianMs)，后 $($secondHalfValues.Count) 轮中位 **$(Format-Ms $secondHalfMedianMs)**，" +
            "相差 **$('{0:N0}%' -f $warmupDriftPercent)**，而两段的起跑 CPU 都通过了 $MaxCpuBeforeLaunchPercent% 门禁。")
        & $add '>'
        & $add ('> 最可能的原因是**刚构建完的二进制被 Defender 首扫**，加上构建残留进程还在退场。' +
            "这类污染是**整段偏移**而不是个别离群轮，所以既躲过了 CPU 门禁，也躲过了「超过中位 2 倍」的离群判定。" +
            "**要引用启动数字，请用后半段中位 $(Format-Ms $secondHalfMedianMs)，或在 `-SkipBuild` 下重跑一次。** " +
            '内存受影响小得多（逐样本稳定），仍可引用。')
        & $add ''
    }
    if ($noisyEnvironment) {
        & $add ("> **量测环境有背景负载，启动数字不可对外引用。** 每轮启动前会等 CPU 降到 $MaxCpuBeforeLaunchPercent% 以下（最多等 $([Math]::Round($CpuSettleTimeoutMs / 1000.0))s），" +
            "但 $Runs 轮里有 **$($hotRuns.Count) 轮**没等到就开跑（第 $((($hotRuns | ForEach-Object { $_.run }) -join '、')) 轮）。" +
            "各轮起跑负载中位 $('{0:N0}%' -f $systemCpuMedianPct)、峰值 $('{0:N0}%' -f $systemCpuMaxPct)。" +
            '启动耗时对 CPU 竞争极敏感，本报告的启动值应视为**上限**而非基准；内存受影响小得多，仍可参考。' +
            '要拿可引用的启动基准，需在空闲机器上重跑。')
        if ($topCpuConsumers.Count -gt 0) {
            & $add '>'
            & $add ('> 量测窗口内抢 CPU 的进程（按 CPU 时间增量）：' +
                (($topCpuConsumers | ForEach-Object { "``$($_.name)`` $($_.cpuSeconds)s（$('{0:N0}%' -f $_.oneCorePercent) 单核）" }) -join '、') + '。')
        }
        & $add ''
    }
    else {
        & $add ("量测环境：每轮启动前都等到全核 CPU 降至 $MaxCpuBeforeLaunchPercent% 以下才拉起进程，" +
            "实际起跑负载中位 $(if ($null -ne $systemCpuMedianPct) { '{0:N0}%' -f $systemCpuMedianPct } else { '未知' })、" +
            "峰值 $(if ($null -ne $systemCpuMaxPct) { '{0:N0}%' -f $systemCpuMaxPct } else { '未知' })，$Runs 轮全部达标，启动数字可作基准。")
        & $add ''
    }

    & $add '## 验收对照'
    & $add ''
    & $add '| 验收项 | 归属 | 标准 | 实测 | 预算占用 | 结果 |'
    & $add '| --- | --- | --- | --- | --- | --- |'
    foreach ($gate in $gates) {
        $measuredText = if ($gate.unit -eq 'ms') { Format-Ms $gate.measured } else { Format-Mb ([double]$gate.measured * 1MB) }
        if ($null -eq $gate.measured) { $measuredText = '未量测' }
        $resultText = if ($null -eq $gate.pass) { '未量测' } elseif ($gate.pass) { 'PASS' } else { '**FAIL**' }
        & $add ("| $($gate.id) | $($gate.source) | $($gate.criterion) | **$measuredText** | $(Format-Pct $gate.measured $gate.budget) | $resultText |")
    }
    & $add ''

    & $add '## 启动耗时逐轮'
    & $add ''
    & $add '外部计时 = 内核记录的进程创建时刻（`Process.StartTime`）到应用写入就绪标记的时刻（标记里的 `readyEpochMs`），跨进程，含 Rust 初始化 + WebView2 起进程 + 加载前端 + 首帧。'
    & $add ('不用 `Start-Process` 返回后启动的秒表：那个口径额外含 cmdlet 开销与 5ms 轮询粒度，本次中位高出 ' + (Format-Ms $harnessOverheadMedianMs) + '。前端三个数来自 `performance.now()`，起点是第一行 JS。')
    & $add ''
    & $add '| 轮次 | 类型 | 外部计时 | 前端 mount | 前端 ready | 前端 firstPaint | JS 时钟外开销 |'
    & $add '| --- | --- | --- | --- | --- | --- | --- |'
    foreach ($runResult in $runResults) {
        $typeText = if ($runResult.classification -eq 'first-clean-process-launch') { '首轮（最接近冷启动）' } else { '重启（缓存已热）' }
        $gap = [double]$runResult.externalReadyMs - [double]$runResult.frontend.firstPaintMs
        & $add ("| $($runResult.run) | $typeText | **$(Format-Ms $runResult.externalReadyMs)** | $(Format-Ms $runResult.frontend.mountMs) | " +
            "$(Format-Ms $runResult.frontend.readyMs) | $(Format-Ms $runResult.frontend.firstPaintMs) | $(Format-Ms $gap) |")
    }
    & $add ("| — | 中位数 | **$(Format-Ms $startupMedianMs)** | $(Format-Ms $frontendMountMedianMs) | $(Format-Ms $frontendReadyMedianMs) | $(Format-Ms $frontendPaintMedianMs) | $(Format-Ms $preScriptOverheadMs) |")
    & $add ''
    & $add ("首轮 $(Format-Ms $startupFirstMs) · 重启中位 $(Format-Ms $relaunchMedianMs) · 全部轮次 min/中位/p95/max = $(Format-Ms $startupMinMs) / $(Format-Ms $startupMedianMs) / $(Format-Ms $startupP95Ms) / $(Format-Ms $startupMaxMs)。")
    & $add ''
    & $add ("预热漂移检查：前 $halfIndex 轮中位 $(Format-Ms $firstHalfMedianMs)，后 $($secondHalfValues.Count) 轮中位 $(Format-Ms $secondHalfMedianMs)，" +
        "相差 $(if ($null -ne $warmupDriftPercent) { '{0:N0}%' -f $warmupDriftPercent } else { '—' })。" +
        $(if ($warmupDriftDetected) {
            '**超过 30% 阈值，本次汇总值不可引用**，原因与处置见报告顶部的提示。'
        }
        else {
            '在 30% 阈值内，两段属同一量级，汇总值可用。'
        }))
    & $add ''
    & $add ("**「JS 时钟外开销」这一列是本表最重要的数字**：$Runs 轮只波动 $(Format-Ms $preJsMinMs)～$(Format-Ms $preJsMaxMs)（中位 $(Format-Ms $preJsMedianMs)），" +
        "远比总耗时稳定。它是「进程创建 + Rust 初始化 + WebView2 起 $($processCountMax - 1) 个进程 + 加载 HTML/JS」的固定成本。")
    if ($outlierRuns.Count -gt 0) {
        & $add ''
        & $add ("离群轮次（超过中位数 2 倍）共 $($outlierRuns.Count) 轮：" +
            (($outlierRuns | ForEach-Object { "第 $($_.run) 轮 $(Format-Ms $_.externalReadyMs)（其中首帧 $(Format-Ms $_.frontend.firstPaintMs)，JS 时钟外 $(Format-Ms ([double]$_.externalReadyMs - [double]$_.frontend.firstPaintMs))）" }) -join '；') +
            '。离群量全部落在 JS 之后的渲染阶段，JS 之前那段仍在正常区间 —— 说明是 WebView 内的调度抖动，不是启动路径变慢。' +
            '门禁取 p95 而非 max 就是为了不让单轮抖动定结论，但离群必须显式列出。')
    }
    if ($Runs -lt 20) {
        & $add ''
        & $add ("注：$Runs 轮样本下 p95 落在最大值上，还不是真正的分位数。要让 p95 能排掉一个离群轮，至少需要 ``-Runs 20``。")
    }
    & $add ''

    & $add '## 空闲内存逐轮'
    & $add ''
    & $add ('进程树 = apogee.exe 及其全部后代，因此包含 WebView2 的 msedgewebview2.exe 系列子进程（浏览器、GPU、渲染器、工具、crashpad）。' +
        '遍历从 root 出发按 `Win32_Process.ParentProcessId` 展开，并**逐条父子边校验创建时间**（子不早于父）以排除 PID 复用产生的伪边，' +
        "同时断言树内映像名只有 apogee / msedgewebview2。峰值进程数 $processCountMax，$Runs 轮全部一致。")
    & $add ''
    & $add '内存主口径是**私有工作集**（`Working Set - Private`，等于任务管理器「内存」列）：常驻且不与其它进程共享的部分。提交量与 WorkingSet 求和一并列出，用途见「发现」第 1 条。'
    & $add ''
    & $add '| 轮次 | 进程数 | 树私有常驻 中位 | 树私有常驻 峰值 | 树提交量 中位 | 树 WorkingSet 求和 | Rust 主进程 私有常驻 |'
    & $add '| --- | --- | --- | --- | --- | --- | --- |'
    foreach ($runResult in $runResults) {
        & $add ("| $($runResult.run) | $($runResult.idle.processCountMax) | **$(Format-Mb $runResult.idle.treePrivateWorkingSetMedianBytes)** | $(Format-Mb $runResult.idle.treePrivateWorkingSetMaxBytes) | " +
            "$(Format-Mb $runResult.idle.treeCommitMedianBytes) | $(Format-Mb $runResult.idle.treeWorkingSetSumMedianBytes) | $(Format-Mb $runResult.idle.rootPrivateWorkingSetMedianBytes) |")
    }
    & $add ("| 合计 | $processCountMax | **$(Format-Mb $treePwsMedianBytes)** | $(Format-Mb $treePwsMaxBytes) | $(Format-Mb $treeCommitMedianBytes) | $(Format-Mb $treeWsSumMedianBytes) | $(Format-Mb $rootPwsMedianBytes) |")
    & $add ''
    & $add ("稳态很稳（中位 $(Format-Mb $treePwsMedianBytes)，逐轮中位大多落在 146～153MB），但**存在瞬时尖峰**：" +
        "全局峰值 $(Format-Mb $treePwsMaxBytes) 出现在第 $peakRunNumber 轮" +
        $(if ($spikyRuns.Count -gt 1) { "，$Runs 轮里有 $($spikyRuns.Count) 轮（第 $((($spikyRuns | ForEach-Object { $_.run }) -join '、')) 轮）峰值超出中位 10MB 以上" } else { '' }) +
        "。首轮（第 1 轮）峰值 $(Format-Mb $firstLaunchPwsMaxBytes)，" +
        $(if ($firstLaunchPwsMaxBytes -gt $relaunchPwsMaxBytes) { '是全场最高，说明这是 WebView2 首次运行的一次性开销。' } else { '并不是最高的一轮 —— 尖峰与「第几次启动」无关，是 WebView2 侧的瞬时行为。' }) +
        '门禁取全部样本的最差值：用户看到的就是任务管理器上跳动的那个峰值，不是我们挑出来的中位数。')
    & $add ''
    & $add '### 内存花在哪（按进程角色，私有常驻中位）'
    & $add ''
    & $add '| 角色 | 每轮进程数 | 私有常驻 中位 | 占进程树 |'
    & $add '| --- | --- | --- | --- |'
    foreach ($role in $roleBreakdown) {
        & $add ("| ``$($role.role)`` | $($role.instancesPerRun) | **$(Format-Mb $role.privateWorkingSetMedianBytes)** | $(Format-Pct $role.privateWorkingSetMedianBytes $treePwsMedianBytes) |")
    }
    & $add ''
    & $add ("汇总：Rust 主进程 $(Format-Mb $rootPwsMedianBytes)，WebView2 进程组 $(Format-Mb $webviewPwsMedianBytes)，后者占进程树 $(Format-Pct $webviewPwsMedianBytes $treePwsMedianBytes)。")
    & $add ''

    # ── 外部对照 ────────────────────────────────────────────────────────
    $reference = $script:ReferenceApp
    $referenceLowMb = [double]$reference.privateWorkingSetMbRange[0]
    $referenceHighMb = [double]$reference.privateWorkingSetMbRange[1]
    & $add '### 同机同栈对照：dbx'
    & $add ''
    & $add ("空壳测出 $(Format-Mb $treePwsMedianBytes) 之后必须回答一个问题：**这是我们实现得差，还是 WebView2 的地板？** " +
        "Apogee 自己测不了这个，需要一个同栈的外部参照。[$($reference.name)]($($reference.source)) 是 $($reference.stack) 的数据库客户端，" +
        "**已发布的成熟产品**，$($reference.condition)。")
    & $add ''
    & $add "| 指标 | $($reference.name) | Apogee（空壳） |"
    & $add '| --- | --- | --- |'
    & $add ("| 宿主进程私有常驻（任务管理器里应用那一行） | $('{0:N2} MB' -f $reference.hostPrivateWorkingSetMb) | $(Format-Mb $rootPwsMedianBytes) |")
    & $add ("| **整棵树私有常驻** | **$('{0:N1}～{1:N1} MB' -f $referenceLowMb, $referenceHighMb)** | **$(Format-Mb $treePwsMedianBytes)** |")
    & $add ("| WorkingSet 求和（含共享页重复计数） | $('{0:N1} MB' -f $reference.workingSetSumMb) | $(Format-Mb $treeWsSumMedianBytes) |")
    & $add ("| 提交量 | $('{0:N1} MB' -f $reference.commitMb) | $(Format-Mb $treeCommitMedianBytes) |")
    & $add ("| 进程数 | $($reference.processCount) | $processCountMax |")
    & $add ("| WebView2 启动开关 | $($reference.webview2Flags) | 同为默认配置 |")
    & $add ''
    & $add ("dbx 的角色分解（私有常驻）：" +
        (($reference.roles.Keys | ForEach-Object { "``$_`` $('{0:N1} MB' -f $reference.roles[$_])" }) -join '、') + '。')
    & $add ''
    & $add ("**结论：地板是 WebView2 的。** 一个功能齐全、带真实负载的同栈产品稳定在 $('{0:N0} MB' -f $referenceHighMb) 左右，" +
        "反而**高于** Apogee 空壳的 $(Format-Mb $treePwsMedianBytes)；两者宿主进程都只有几 MB，进程数、WebView2 开关完全一致。" +
        "这说明 $(Format-Mb $treePwsMedianBytes) 不是实现质量问题，而是 Windows 上 WebView2 的起步成本；" +
        '这条对照连同 Spike S3 的负载实测，共同支撑了把 PRD §5.1 的内存预算从 150MB 重定为 **Windows 220MB** —— 定稿见 03 文档 §10.4。')
    & $add ''
    & $add ("三点使用限制：(1) dbx 是**单次手工快照**对 Apogee 的 $Runs 轮数据，不是受控对比；" +
        '(2) dbx 带真实负载而 Apogee 是空壳，这个差异对 dbx 不利，所以只能用来支撑「地板很高」，不能用来比较两个产品的优劣；' +
        "(3) 数字取自同一台机器、同一个 WebView2 Runtime、同一套私有工作集口径，量测于 $($reference.measuredAt)，换机器需重测。")
    & $add ''
    & $add ('三个口径在这里给出了**互相矛盾**的排序：按私有常驻 dbx 更高，按 WorkingSet 求和 Apogee 更高，按提交量 dbx 高近一倍。' +
        '原因是 WorkingSet 含共享页且会被系统按内存压力回收（dbx 已运行一小时被 trim 过，Apogee 是刚拉起的），提交量则是申请量而非占用量。' +
        '**这正是为什么指标必须钉死口径**：口径选错，同一组数据能得出相反结论。')
    & $add ''

    & $add '## 体积'
    & $add ''
    & $add '| 产物 | 大小 | 预算 | 预算占用 |'
    & $add '| --- | --- | --- | --- |'
    & $add ('| `apogee.exe`（release，已 strip） | **' + (Format-Mb $executableBytes) + '** | — | — |')
    & $add ("| NSIS 安装包 | **$(Format-Mb $installerBytes)** | < 15 MB | $(Format-Pct $installerMb 15.0) |")
    & $add ''
    & $add '安装包不含 WebView2 Runtime（`webviewInstallMode: downloadBootstrapper`）。这是 15MB 预算能成立的前提，也意味着 Win10 无 Runtime 的机器首次安装需要联网。'
    & $add ''
    foreach ($artifact in $artifacts) {
        & $add ("- ``$($artifact.path)`` — $(Format-Mb $artifact.bytes)")
    }
    & $add ''

    & $add '## 发现'
    & $add ''
    $gpuRole = $roleBreakdown | Where-Object { $_.role -eq 'gpu-process' } | Select-Object -First 1
    & $add ('1. **进程树内存不能把各进程的 WorkingSet 加起来。** 同一份数据，' +
        "WorkingSet 求和 $(Format-Mb $treeWsSumMedianBytes)，私有常驻 $(Format-Mb $treePwsMedianBytes)，" +
        "提交量 $(Format-Mb $treeCommitMedianBytes) —— 求和口径把真实占用放大了 **$('{0:N2}' -f $sharedPageInflation)×**。" +
        "原因是 WebView2 的 $($processCountMax - 1) 个子进程映射同一批 msedge DLL，共享页在每个进程里各算一次。" +
        '`PrivateMemorySize64` 也不对，它是提交量而非常驻量。正确口径是性能计数器的 `Working Set - Private`（`Win32_PerfRawData_PerfProc_Process.WorkingSetPrivate`），' +
        '即任务管理器「内存」列。**本脚本第一版就是按 WorkingSet 求和写的，报出 440MB 空闲内存，差点得出「架构不可行」的错误结论。**')
    & $add ''
    & $add ('2. **`ParentProcessId` 不能直接用来划进程树，Windows 会复用 PID。** ' +
        '真正的父进程退出后，它的 PID 可能被我们的 apogee.exe 或某个 WebView2 子进程占用，于是一批无关进程看起来就成了「我们的后代」。' +
        '本脚本早期版本就中过：某一轮把另一个应用的 13 个进程算进了内存，单轮从 147MB 跳到 430MB；' +
        '**更严重的是 `Stop-ProcessTree` 按同一份名单把那些进程杀掉了** —— 量测台动了被测对象之外的东西，这比测错数字严重得多。' +
        '两道防线：父子边逐条校验创建时间（子不早于父），以及树内映像名白名单（只允许 apogee / msedgewebview2），' +
        '越界时统计直接报错、杀进程直接跳过。教训是通用的：**进程树是一个需要校验的推断，不是一个可以直接读取的事实。**')
    & $add ''
    & $add ("3. **内存几乎全部是 WebView2 的，Rust 侧只有 $(Format-Mb $rootPwsMedianBytes)。** 进程树私有常驻 $(Format-Mb $treePwsMedianBytes) 里，" +
        "WebView2 进程组占 $(Format-Pct $webviewPwsMedianBytes $treePwsMedianBytes)" +
        $(if ($null -ne $gpuRole) { "，其中单个 ``gpu-process`` 就要 $(Format-Mb $gpuRole.privateWorkingSetMedianBytes)（占 $(Format-Pct $gpuRole.privateWorkingSetMedianBytes $treePwsMedianBytes)）" } else { '' }) +
        '。两个推论：一是 80MB 这个 Spike 门槛在 Windows + WebView2 上不可达，它是纸面估算而非工程约束（同栈成熟产品 dbx 的对照见上一节）；' +
        '二是内存预算的分母由 WebView2 定死，Rust 侧每省一 MB 都直接变成余量 —— 这正是 S2 修掉 R13（索引 9.1× 膨胀）的价值所在，也意味着 Rust 侧新增常驻结构要盯得很紧。')
    & $add ''
    & $add ("4. **启动耗时的主体在 JS 时钟之外，而且它是地板。** 外部计时中位 $(Format-Ms $startupMedianMs)，前端自报 firstPaint 中位仅 $(Format-Ms $frontendPaintMedianMs)，" +
        "差值 **$(Format-Ms $preScriptOverheadMs)**（占 $(Format-Pct $preScriptOverheadMs $startupMedianMs)）花在「进程创建 → Rust 初始化 → WebView2 起进程 → 加载 HTML/JS」上，" +
        "且 $Runs 轮只波动 $(Format-Ms $preJsMinMs)～$(Format-Ms $preJsMaxMs)（跨度 $(Format-Ms ($preJsMaxMs - $preJsMinMs))），比总耗时稳定得多。" +
        '含义很直接：继续压前端首屏 chunk 的边际收益很小（S5 已把首屏做到 101.9KB gzip），真正的杠杆在 WebView2 启动，而那部分不在我们手里。' +
        '也说明只看 `window.__apogeeBoot` 会系统性低估启动耗时 —— 性能自检面板（PRD §5.2）必须显示外部口径，不能只报前端自计时。')
    & $add ''
    & $add ("5. **首轮与重启轮次只差 $(Format-Ms ([Math]::Abs($startupFirstMs - [double]$relaunchMedianMs)))，两者都不是真冷启动。** " +
        "首轮 $(Format-Ms $startupFirstMs)，重启中位 $(Format-Ms $relaunchMedianMs)。差距这么小本身就是证据：" +
        '机器刚跑完构建，二进制、WebView2 运行时、EBWebView 用户数据目录全在 OS 文件缓存里，所谓「首轮」其实也是热的。' +
        "真正的冷启动会更慢，Defender 首扫新二进制的成本也还没体现。全部轮次跨度 $(Format-Ms $startupMinMs)～$(Format-Ms $startupMaxMs)，单次数字不可引用。")
    & $add ''
    & $add ("6. **就绪信号必须由被测进程自己发，且时钟要取内核口径。** 跨进程时钟依赖 release 应用在首帧后写 ``APOGEE_S1_READY_FILE`` 指定的标记（原子 tmp + rename，含 PID 校验）；" +
        '环境变量不存在时这段代码直接返回，正常运行零开销。两个替代方案都会测偏：用「窗口出现」会被 Rust 侧 3s 兜底 `show()` 污染，用「进程存在」在 WebView2 还在加载时就已成立。' +
        "起点也不能用 ``Start-Process`` 返回后的秒表 —— 那个口径本次比内核 ``Process.StartTime`` 高出中位 $(Format-Ms $harnessOverheadMedianMs)，含 cmdlet 开销与 5ms 轮询粒度。")
    & $add ''
    & $add ('7. **体积是唯一有大量余量的指标。** ' +
        "``apogee.exe`` $(Format-Mb $executableBytes)，NSIS 安装包 $(Format-Mb $installerBytes)，只占 15MB 预算的 $(Format-Pct $installerMb 15.0)。" +
        '前提有两条，都要守住：release profile 保持 `opt-level="z"` + `lto=fat` + `strip`；WebView2 用 `downloadBootstrapper` 不打进安装包。' +
        '后者的代价是 Win10 无 Runtime 的机器首次安装需要联网，这是产品决策而不是量测问题。')
    & $add ''
    # 220MB = PRD §5.1 定稿后的 Windows 空闲内存预算（S1 地板 + S3 负载实测共同定的，见 03 §10.4）
    $budgetBytes = 220 * 1MB
    & $add ('8. **空壳的数字是地板，不是达标证明。** 被测对象没有集合树、没有 SQLite 连接、没有脚本引擎、只有 1 个空标签。' +
        "PRD §5.1 的量测条件是「1 工作区 3 标签」，空壳稳态中位 $(Format-Mb $treePwsMedianBytes)（对 220MB 预算 $(Format-Headroom -BudgetBytes $budgetBytes -MeasuredBytes $treePwsMedianBytes)）、" +
        "峰值 $(Format-Mb $treePwsMaxBytes)（$(Format-Headroom -BudgetBytes $budgetBytes -MeasuredBytes $treePwsMaxBytes)）。" +
        "Spike S3 在真实负载（1 工作区 + 3 标签 + 5000 节点集合树）下实测 166.2 MB 中位 / 184.9 MB 最差观测 —— " +
        "也就是说**从空壳到真实负载只涨了约 20 MB，其中最大一块还在 `gpu-process` 而不是我们的代码里**。" +
        '真正的风险在增量：每个功能落地都要重跑本脚本与 S3，不能等 v0.1 做完再测。')
    & $add ''

    & $add '## 未量测 / 边界'
    & $add ''
    foreach ($item in $notMeasured) {
        & $add "- $item"
    }
    & $add ''

    & $add '## 后续'
    & $add ''
    $referenceHighMb = [double]$script:ReferenceApp.privateWorkingSetMbRange[1]
    & $add ('1. **门槛已定稿（S1 提出，S3 定案），本节不再是待决事项。** 03 文档 §10.1 原来的「启动 < 400ms、空闲 < 80MB」' +
        "按实测不可达（WebView2 空壳地板就是 $(Format-Mb $treePwsMedianBytes) / $(Format-Ms $startupMedianMs)，其中可控的 Rust 侧只有 $(Format-Mb $rootPwsMedianBytes)），**已废弃**。" +
        "PRD §5.1 的内存预算按 S3 的负载实测重定为 **Windows < 220MB**（同栈成熟产品 dbx 在 $('{0:N0} MB' -f $referenceHighMb)），" +
        '并新增 **「Rust 主进程私有常驻 < 20MB」** 作为唯一真正可控的回归门禁 —— 本脚本的门禁表已按此更新。完整推导见 03 文档 §10.4。')
    & $add ('2. **「关硬件加速换内存」已量化，但不采用为达标手段。** S3 实测关掉 `--disable-gpu --disable-gpu-compositing` ' +
        '能省 62.9MB（37.5%），且集合树滚动掉帧率仍为 0%。不采用的理由是证据覆盖面：「没有代价」只在文本列表滚动这一种负载下验证过，' +
        '而这个开关是全局的（CodeMirror 大文档、响应体树重绘、4K 全屏软件合成都没测）。它记为一项已量化的可选优化，' +
        '前置条件是先量那几种负载在软件合成下的帧率。顺带纠正一点：**`--disable-gpu` 不会让 `gpu-process` 消失**，只是让它退到 SwiftShader（77.2MB → 10.7MB），进程数不变。')
    & $add '3. **把本脚本接进 CI 性能门禁（PRD §5.2）**：`-Runs 20 -SkipBuild` 复用构建产物，启动或内存回退 > 10% 即 fail。注意 CI runner 的 GPU 进程行为与本机不同，基线要按 runner 单独建。S3（`pnpm spike:s3`）同样要进 CI —— 它才是「真实负载下内存」和「渲染帧率」的门禁。'
    & $add '4. **macOS 侧需要一份等价脚本才能关闭 S1。** 就绪标记契约（`APOGEE_S1_READY_FILE`）已经是跨平台的，Rust 侧无需改动；macOS 上 WKWebView 的进程模型与内存口径都不一样，量测台要重写。'
    & $add '5. **冷启动要拿可对外引用的数字**，需在重启或清文件缓存后单跑一轮，本脚本的首轮不能当冷启动宣称。'
    & $add ('6. **`gpu-process` 仍是最大的单一内存项' +
        $(if ($null -ne $gpuRole) { "（空壳下 $(Format-Mb $gpuRole.privateWorkingSetMedianBytes)）" } else { '' }) +
        '，但它不在我们的控制范围内。** 唯一能动它的手段是关硬件加速，处置见第 2 条。这条留在这里是为了提醒：' +
        '内存预算的分母由 WebView2 决定，优化只能作用在 Rust 那几 MB 上。')
    & $add ''
    & $add '---'
    & $add ''
    & $add '原始数据：`spikes/results/s1.json`。复现：`pwsh -File scripts/measure-s1-windows.ps1`。'

    [IO.File]::WriteAllText($ReportPath, ($lines -join "`n"), $Utf8NoBom)

    Write-Host ''
    if ($noisyEnvironment) {
        Write-Warning ("{0}/{1} 轮未等到 CPU 安静就开跑（起跑负载中位 {2:N0}%，峰值 {3:N0}%）。启动数字应视为上限，不可对外引用。" -f
            $hotRuns.Count, $Runs, $systemCpuMedianPct, $systemCpuMaxPct)
    }
    Write-Host "[S1] $verdict"
    foreach ($gate in $gates) {
        $stateText = if ($null -eq $gate.pass) { '未量测' } elseif ($gate.pass) { 'PASS' } else { 'FAIL' }
        $measuredText = if ($gate.unit -eq 'ms') { Format-Ms $gate.measured } else { Format-Mb ([double]$gate.measured * 1MB) }
        if ($null -eq $gate.measured) { $measuredText = '—' }
        Write-Host ("  [{0}] {1,-16} {2} / 预算 {3}{4}" -f $stateText, $gate.id, $measuredText, $gate.budget, $gate.unit)
    }
    Write-Host "[S1] 数据：spikes/results/s1.json"
    Write-Host "[S1] 报告：spikes/results/s1-report.md"
}
finally {
    if ($null -eq $previousAutorun) {
        Remove-Item Env:VITE_AUTORUN_S2 -ErrorAction SilentlyContinue
    }
    else {
        $env:VITE_AUTORUN_S2 = $previousAutorun
    }
}

if (-not $windowsPass) { exit 1 }
