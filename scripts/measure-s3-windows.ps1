<#
.SYNOPSIS
Spike S3 量测台（Windows）：自研虚拟树的帧率与首渲，× GPU 开/关两种配置，并量进程树内存。

.DESCRIPTION
本脚本回答两个问题，一次跑完：

1. **S3 本身**：自研虚拟树渲染 5000 节点，滚动能不能维持 60fps、首渲是否 < 100ms
   （03 文档 §10.1）。帧率与首渲由被测应用自己量（`src/spike/S3Panel.vue`），
   因为只有 WebView 内部才拿得到 rAF 时间戳和 LoAF 条目。

2. **内存门槛的最后一块拼图**：03 文档 §10.4 的选项 (b) 是「从 WebView2 侧找真实节省 ——
   `gpu-process` 单独占 67MB，关硬件加速能省，但会牺牲滚动帧率」，并注明「须先做 S3」。
   所以这里在**同一个二进制**上跑两遍：默认配置和关掉 GPU 的配置，
   两遍都同时量帧率和进程树私有工作集。省多少内存、掉多少帧，放在一张表里看。

## 两阶段协议（为什么不能边跑帧率边采内存）

采一次进程树内存要跑两条 CIM 查询（`Win32_Process` + `Win32_PerfRawData_PerfProc_Process`），
几百毫秒的 CPU。在帧率量测期间做这件事，等于一边测帧率一边给机器加负载 ——
测出来的掉帧有一部分是量测台自己造成的。这正是 S1 工程教训第 5 条（量测台要报告环境噪声）
的反面：不只要报告噪声，更要**不制造**噪声。

所以被测应用把自己的进度写进 `s3-phase.json`：

    phase=fps          → 本脚本什么都不做，绝不碰进程树
    phase=memory-hold  → 应用把 5000 节点的树挂着 + 开满 3 个标签空转，本脚本在这个窗口里采样
    phase=done         → 应用已写出 s3-frontend.json

采样结束后校验阶段文件仍是 `memory-hold`，否则说明采样窗口跑到 hold 之外去了，直接报错 ——
量测台必须能报告失败，不能给出一个来源不明的数字。

.PARAMETER SkipBuild
复用已有的 target/release/apogee.exe。改了前端代码就不能用这个开关（前端产物嵌在 exe 里）。

.PARAMETER Configs
要跑的配置。默认两个都跑；只想复现某一个时可以单独指定。
#>
[CmdletBinding()]
param(
    [switch]$SkipBuild,

    [ValidateSet('gpu-on', 'gpu-off')]
    [string[]]$Configs = @('gpu-on', 'gpu-off'),

    # 应用进入 memory-hold 后先静置，等渲染尾巴和 GC 落定再采样
    [ValidateRange(0, 10000)]
    [int]$IdleSettleMs = 2000,

    [ValidateRange(1, 30)]
    [int]$MemorySamples = 6,

    # 内存要跨**多次启动**取值，不能只起一次进程。
    # 同一份二进制两次启动的驻留内存实测能差 8%（WebView2 的 GPU/着色器缓存状态不同），
    # 拿单次数字去定门槛，就是 S1 报告里批评过的那种做法。
    [ValidateRange(1, 10)]
    [int]$MemoryRounds = 3,

    [ValidateRange(10, 5000)]
    [int]$SampleIntervalMs = 250,

    # 帧率矩阵 + 14s 驻留，实测约 60～90s。留足余量，超时就报错而不是无限等。
    [ValidateRange(30000, 600000)]
    [int]$RunTimeoutMs = 240000,

    [ValidateRange(1, 100)]
    [int]$MaxCpuBeforeLaunchPercent = 25,

    [ValidateRange(0, 120000)]
    [int]$CpuSettleTimeoutMs = 30000
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not $IsWindows) {
    throw 'S3 量测台只能在 Windows 上运行（WebView2 进程模型 + 私有工作集计数器）。'
}

Import-Module (Join-Path $PSScriptRoot 'lib\ProcessTreeMemory.psm1') -Force

$Root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$ResultsDir = Join-Path $Root 'spikes\results'
$ReleaseDir = Join-Path $Root 'target\release'
$Executable = Join-Path $ReleaseDir 'apogee.exe'
$JsonPath = Join-Path $ResultsDir 's3.json'
$PhasePath = Join-Path $ResultsDir 's3-phase.json'
$FrontendPath = Join-Path $ResultsDir 's3-frontend.json'
$Utf8NoBom = [Text.UTF8Encoding]::new($false)
[IO.Directory]::CreateDirectory($ResultsDir) | Out-Null

# ── 配置矩阵 ──────────────────────────────────────────────────────────
#
# `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 是 WebView2 官方提供的浏览器参数入口
# （见 WebView2 browser flags 文档），值会被追加到 msedgewebview2 浏览器进程的命令行。
#
# ⚠️ `--disable-gpu` **不保证** gpu-process 消失：现代 Chromium 会退到 SwiftShader，
# 进程还在，只是不用硬件。所以再加 `--disable-gpu-compositing` 把合成也推回 CPU。
# 到底省不省内存不能推理，只能量 —— 本脚本会把实际观测到的进程角色分解报出来，
# 并断言开关确实进了命令行。
$ConfigMatrix = [ordered]@{
    'gpu-on' = [ordered]@{
        label = 'GPU 开（默认配置）'
        browserArgs = ''
        expectGpuProcess = $true
    }
    'gpu-off' = [ordered]@{
        label = 'GPU 关（--disable-gpu --disable-gpu-compositing）'
        browserArgs = '--disable-gpu --disable-gpu-compositing'
        expectGpuProcess = $false
    }
}

# ── 阶段文件 ──────────────────────────────────────────────────────────

function Read-PhaseFile {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $null }
    try {
        $raw = Get-Content -LiteralPath $Path -Raw -Encoding UTF8
        if ([string]::IsNullOrWhiteSpace($raw)) { return $null }
        return $raw | ConvertFrom-Json
    }
    catch {
        # 正在写入时读到半个文件属正常，下一轮轮询再读
        return $null
    }
}

<#
.SYNOPSIS
等被测应用报告到达指定阶段。进程提前退出或超时都抛错。
#>
function Wait-ForPhase {
    param(
        [string]$Path,
        [string[]]$Phases,
        [System.Diagnostics.Process]$Process,
        [int]$TimeoutMs
    )

    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ($timer.ElapsedMilliseconds -lt $TimeoutMs) {
        $phase = Read-PhaseFile -Path $Path
        if ($null -ne $phase -and $Phases -contains [string]$phase.phase) {
            return $phase
        }
        if ($Process.HasExited) {
            $last = if ($null -ne $phase) { [string]$phase.phase } else { '（还没写出任何阶段）' }
            throw "apogee.exe 在到达阶段 [$($Phases -join '/')] 之前退出，退出码 $($Process.ExitCode)，最后阶段：$last。"
        }
        Start-Sleep -Milliseconds 200
    }
    $last = Read-PhaseFile -Path $Path
    $lastText = if ($null -ne $last) { [string]$last.phase } else { '（阶段文件不存在）' }
    throw "等待阶段 [$($Phases -join '/')] 超过 ${TimeoutMs}ms，当前阶段：$lastText。"
}

# ── 单个配置跑一轮 ────────────────────────────────────────────────────

function Invoke-S3Run {
    param(
        [string]$ConfigKey,
        # 刻意用 object 而不是 hashtable：配置表是 [ordered]@{}（OrderedDictionary），
        # 声明成 hashtable 会触发一次隐式转换，把顺序丢掉。
        [object]$Config,

        # 'fps' = 跑帧率矩阵再驻留（内存里含会话残留）
        # 'mem' = 干净启动直接驻留（内存稳态）
        # 两者都跑，因为差值本身就是「重负载会话之后内存回不回来」的答案。
        [ValidateSet('fps', 'mem')]
        [string]$Mode = 'fps',

        [int]$Round = 1
    )

    $autorunValue = if ($Mode -eq 'mem') { 's3-mem' } else { 's3' }
    Write-Host "[S3] 配置 $ConfigKey · $($Config.label) · 模式 $Mode · 第 $Round 轮"

    $cpuGate = Wait-ForQuietCpu -MaxPercent $MaxCpuBeforeLaunchPercent -TimeoutMs $CpuSettleTimeoutMs
    if (-not $cpuGate.settled) {
        Write-Warning ("等待 {0:N0}ms 后 CPU 仍为 {1}%（阈值 {2}%），带 hot 标记继续。帧率对 CPU 竞争极敏感，本轮数字应视为下限。" -f
            $cpuGate.waitedMs, $cpuGate.loadPercent, $MaxCpuBeforeLaunchPercent)
    }

    Remove-Item -LiteralPath $PhasePath -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $FrontendPath -Force -ErrorAction SilentlyContinue

    $previous = @{
        autorun = $env:APOGEE_SPIKE_AUTORUN
        resultsDir = $env:APOGEE_SPIKE_RESULTS_DIR
        browserArgs = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
    }
    $process = $null
    $cpuSamples = [Collections.Generic.List[double]]::new()

    try {
        $env:APOGEE_SPIKE_AUTORUN = $autorunValue
        # 显式指定报告目录：release 二进制的 cwd 是 target\release，
        # 按 cwd 推断会落到 target\release\spike-results，量测台就找不到文件了。
        $env:APOGEE_SPIKE_RESULTS_DIR = $ResultsDir
        if ([string]::IsNullOrWhiteSpace($Config.browserArgs)) {
            Remove-Item Env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS -ErrorAction SilentlyContinue
        }
        else {
            $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = [string]$Config.browserArgs
        }

        $launchedAt = [DateTimeOffset]::Now
        $process = Start-Process -FilePath $Executable -WorkingDirectory $ReleaseDir -PassThru

        # ① 等到帧率阶段开始，只为确认应用真的跑起来了。这一段之后本脚本必须闭嘴。
        # 也接受 memory-hold：阶段文件只保留最新阶段，万一 fps 阶段比轮询间隔还短就会错过，
        # 那时不该报「超时」——它已经跑过去了。
        $fpsPhase = Wait-ForPhase -Path $PhasePath -Phases @('fps', 'memory-hold') -Process $process -TimeoutMs 60000
        if ([string]$fpsPhase.phase -eq 'fps') {
            Write-Host ("[S3]   帧率阶段开始（显示器标称 {0}Hz，每帧预算 {1:N2}ms）；量测台在此期间不采样。" -f
                $fpsPhase.nominalHz, $fpsPhase.frameBudgetMs)
        }

        # ② 等应用把树挂好进入驻留阶段
        $holdPhase = Wait-ForPhase -Path $PhasePath -Phases @('memory-hold') -Process $process -TimeoutMs $RunTimeoutMs
        # 量测台观察到驻留开始的时刻，用量测台自己的时钟记录（跨进程比较时间字符串不可靠，见下）
        $holdObservedAt = [DateTimeOffset]::Now
        Write-Host "[S3]   驻留阶段：$($holdPhase.condition)"

        # 驻留窗口的截止时刻由**被测应用**申报，不由本脚本假设。
        # 一次采样要跑两条 CIM 查询，耗时随机器上的进程总数变化 —— 按固定次数硬采，
        # 在慢机器上就会跑出窗口。所以改成看着截止时刻采，采不满就少采并如实上报。
        #
        # ⚠️ 用 epoch 毫秒数，不解析 ISO 字符串：`ConvertFrom-Json` 会把 ISO 串自动转成 `DateTime`，
        # 再 `[string]` 一次就丢掉时区，UTC 值被当本地时间重新解析，截止时刻会偏掉整个时区偏移量
        # （实测表现为「窗口内只采到 0 个样本」）。数字不会被 culture 和时区弄坏。
        $holdMs = if ($null -ne $holdPhase.holdMs) { [double]$holdPhase.holdMs } else { 14000.0 }
        # 留 2s 安全边界：最后一次采样必须整个落在窗口内
        $holdDeadline = if ($null -ne $holdPhase.holdUntilEpochMs) {
            [DateTimeOffset]::FromUnixTimeMilliseconds([long]$holdPhase.holdUntilEpochMs).AddMilliseconds(-2000)
        }
        else {
            # 兜底：用量测台自己的时钟。会略微低估窗口（含一次轮询间隔），偏保守，安全。
            [DateTimeOffset]::Now.AddMilliseconds($holdMs - 2000)
        }

        Start-Sleep -Milliseconds $IdleSettleMs

        # ③ 采内存。第一样本带逐进程明细与命令行，用于验证 GPU 开关是否真的生效。
        $samples = [Collections.Generic.List[object]]::new()
        $sampleBudgetExhausted = $false
        for ($i = 1; $i -le $MemorySamples; $i++) {
            if ([DateTimeOffset]::Now -ge $holdDeadline) {
                $sampleBudgetExhausted = $true
                break
            }
            $first = ($i -eq 1)
            $samples.Add([pscustomobject](Get-TreeMemorySample -RootProcessId $process.Id `
                        -IncludeBreakdown:$first -IncludeCommandLines:$first))
            $cpu = Get-SystemCpuLoadPercent
            if ($null -ne $cpu) { $cpuSamples.Add([double]$cpu) }
            if ($i -lt $MemorySamples) { Start-Sleep -Milliseconds $SampleIntervalMs }
        }

        # ④ 校验：样本太少就没有统计意义，宁可失败也不给一个来源不明的数字。
        $minimumSamples = 3
        if ($samples.Count -lt $minimumSamples) {
            throw ("驻留窗口内只采到 {0} 个内存样本（要求至少 {1} 个）。" -f $samples.Count, $minimumSamples) +
                "本机进程数多、CIM 查询慢，请调大 S3Panel.vue 的 MEMORY_HOLD_MS 或调小 -MemorySamples。"
        }
        if ($sampleBudgetExhausted) {
            Write-Warning ("驻留窗口只够采 {0}/{1} 个样本，其余跳过。" -f $samples.Count, $MemorySamples)
        }

        # 采样窗口确实落在驻留期内 —— 跑到 hold 之外去了就是测了别的状态。
        $phaseAfter = Read-PhaseFile -Path $PhasePath
        $phaseAfterName = if ($null -ne $phaseAfter) { [string]$phaseAfter.phase } else { '(缺失)' }
        if ($phaseAfterName -ne 'memory-hold') {
            throw "内存采样结束时阶段已变为 [$phaseAfterName]，采样窗口跑出了驻留期。拒绝采信本轮内存数据。"
        }

        # ⑤ 峰值工作集：单次读取，零采样开销。
        #    注意口径 —— 这是 WorkingSet 峰值（含共享页，S1 实测放大 2.99×），
        #    Windows 没有「私有工作集峰值」计数器。只作为「过程中有没有更高的尖峰」的旁证，
        #    **不进门禁**，也不能和私有工作集的数字混在一起比。
        $treeIds = @(Get-ProcessTreeIds -RootProcessId $process.Id)
        $peakWorkingSet = [long](
            ($treeIds | ForEach-Object {
                $proc = Get-Process -Id $_ -ErrorAction SilentlyContinue
                if ($null -ne $proc) { [long]$proc.PeakWorkingSet64 } else { 0 }
            } | Measure-Object -Sum).Sum
        )

        # ⑥ 等前端把报告写完
        [void](Wait-ForPhase -Path $PhasePath -Phases @('done') -Process $process -TimeoutMs $RunTimeoutMs)
        $frontendTimer = [Diagnostics.Stopwatch]::StartNew()
        while (-not (Test-Path -LiteralPath $FrontendPath -PathType Leaf)) {
            if ($frontendTimer.ElapsedMilliseconds -gt 15000) {
                throw '应用报告 phase=done 但没有写出 s3-frontend.json。'
            }
            Start-Sleep -Milliseconds 200
        }
        $frontend = Get-Content -LiteralPath $FrontendPath -Raw -Encoding UTF8 | ConvertFrom-Json

        # ── 汇总本配置 ──────────────────────────────────────────────
        $pwsValues = [double[]]@($samples | ForEach-Object { $_.treePrivateWorkingSetBytes })
        $commitValues = [double[]]@($samples | ForEach-Object { $_.treeCommitBytes })
        $wsSumValues = [double[]]@($samples | ForEach-Object { $_.treeWorkingSetSumBytes })
        $rootPwsValues = [double[]]@($samples | ForEach-Object { $_.rootPrivateWorkingSetBytes })
        $breakdown = @($samples[0].processes)

        $roleRows = @(
            $breakdown | Group-Object -Property role | ForEach-Object {
                [pscustomobject][ordered]@{
                    role = $_.Name
                    instances = $_.Count
                    privateWorkingSetBytes = [long](($_.Group | Measure-Object -Property privateWorkingSetBytes -Sum).Sum)
                }
            } | Sort-Object -Property privateWorkingSetBytes -Descending
        )

        # GPU 开关是否真的生效：命令行里有没有那些 flag + 进程树里还有没有 gpu-process。
        # 两件事都要报 —— 「环境变量设了」不等于「flag 到了浏览器进程」，
        # 「flag 到了」也不等于「gpu-process 消失了」。
        $allCommandLines = @($breakdown | ForEach-Object { [string]$_.commandLine })
        $flagsObserved = @()
        if (-not [string]::IsNullOrWhiteSpace($Config.browserArgs)) {
            foreach ($flag in ([string]$Config.browserArgs).Split(' ', [StringSplitOptions]::RemoveEmptyEntries)) {
                $seen = @($allCommandLines | Where-Object { $_ -like "*$flag*" }).Count -gt 0
                $flagsObserved += [pscustomobject][ordered]@{ flag = $flag; presentInCommandLine = $seen }
            }
            $missing = @($flagsObserved | Where-Object { -not $_.presentInCommandLine })
            if ($missing.Count -gt 0) {
                $message = ("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS 里的开关没有出现在任何进程的命令行上：{0}。" -f
                    (($missing | ForEach-Object { $_.flag }) -join '、')) +
                    '说明该配置根本没生效 —— 此时的内存/帧率数字会被误读成「关 GPU 也没省内存」，' +
                    '这正是必须验证开关而不是相信环境变量的原因。'
                throw $message
            }
        }
        $gpuRole = $roleRows | Where-Object { $_.role -eq 'gpu-process' } | Select-Object -First 1

        $rootPwsMedian = [long](Get-Median -Values $rootPwsValues)
        $treePwsMedian = [long](Get-Median -Values $pwsValues)

        return [pscustomobject][ordered]@{
            config = $ConfigKey
            mode = $Mode
            round = $Round
            label = [string]$Config.label
            browserArgs = [string]$Config.browserArgs
            launchedAt = $launchedAt.ToString('o')
            holdObservedAt = $holdObservedAt.ToString('o')
            holdCondition = [string]$holdPhase.condition
            cpuBeforeLaunchPercent = $cpuGate.loadPercent
            cpuSettledBeforeLaunch = [bool]$cpuGate.settled
            cpuDuringHoldMedianPercent = if ($cpuSamples.Count -gt 0) { Get-Median -Values ([double[]]$cpuSamples.ToArray()) } else { $null }
            gpu = [ordered]@{
                expectGpuProcess = [bool]$Config.expectGpuProcess
                gpuProcessPresent = ($null -ne $gpuRole)
                gpuProcessPrivateWorkingSetBytes = if ($null -ne $gpuRole) { $gpuRole.privateWorkingSetBytes } else { 0 }
                flagsObserved = @($flagsObserved)
            }
            memory = [ordered]@{
                metric = 'privateWorkingSet（Working Set - Private，等于任务管理器「内存」列）'
                gateStatistic = 'max（内存逐样本稳定，取最差观测值）'
                sampleCount = $samples.Count
                sampleBudgetExhausted = $sampleBudgetExhausted
                processCount = [int](($samples | Measure-Object -Property processCount -Maximum).Maximum)
                treePrivateWorkingSetMedianBytes = $treePwsMedian
                treePrivateWorkingSetMaxBytes = [long](($pwsValues | Measure-Object -Maximum).Maximum)
                treeCommitMedianBytes = [long](Get-Median -Values $commitValues)
                treeWorkingSetSumMedianBytes = [long](Get-Median -Values $wsSumValues)
                rootPrivateWorkingSetMedianBytes = $rootPwsMedian
                webviewPrivateWorkingSetMedianBytes = $treePwsMedian - $rootPwsMedian
                treePeakWorkingSetBytes = $peakWorkingSet
                treePeakWorkingSetCaveat = 'WorkingSet 峰值含共享页（S1 实测放大 2.99×），Windows 无「私有工作集峰值」计数器。只作尖峰旁证，不进门禁。'
                roleBreakdown = @($roleRows | Select-Object role, instances, privateWorkingSetBytes)
                samples = @($samples | Select-Object -Property * -ExcludeProperty processes)
                processBreakdown = @($breakdown | Select-Object -Property * -ExcludeProperty commandLine)
            }
            frontend = $frontend
        }
    }
    finally {
        if ($null -ne $process) {
            Stop-ProcessTree -RootProcessId $process.Id
            try { $process.WaitForExit(5000) | Out-Null } catch { }
            $process.Dispose()
        }
        foreach ($pair in @(
                @{ name = 'APOGEE_SPIKE_AUTORUN'; value = $previous.autorun },
                @{ name = 'APOGEE_SPIKE_RESULTS_DIR'; value = $previous.resultsDir },
                @{ name = 'WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'; value = $previous.browserArgs }
            )) {
            if ($null -eq $pair.value) {
                Remove-Item ("Env:" + $pair.name) -ErrorAction SilentlyContinue
            }
            else {
                Set-Item ("Env:" + $pair.name) -Value $pair.value
            }
        }
    }
}

# ── 主流程 ────────────────────────────────────────────────────────────

$existing = @(Get-Process -Name 'apogee' -ErrorAction SilentlyContinue)
if ($existing.Count -gt 0) {
    throw '检测到正在运行的 apogee.exe。单实例复用会让新进程直接退出、量测拿不到自己的进程树，请先关闭它。'
}

if (-not $SkipBuild) {
    Write-Host '[S3] 构建 release（前端产物嵌在 exe 里，改了前端就必须重建）...'
    Push-Location $Root
    try {
        & npx tauri build --no-bundle
        if ($LASTEXITCODE -ne 0) {
            throw "Tauri release 构建失败，退出码 $LASTEXITCODE。"
        }
    }
    finally {
        Pop-Location
    }
}

if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
    throw "找不到 release 可执行文件：$Executable。先跑一次不带 -SkipBuild 的 S3，或 npx tauri build --no-bundle。"
}

$logicalCores = [int][Environment]::ProcessorCount
$cpuBefore = Get-CpuTimeSnapshot
$measurementTimer = [Diagnostics.Stopwatch]::StartNew()

# 每种 GPU 配置起 1 + $MemoryRounds 次进程：
#   fps 轮（1 次）  —— 跑帧率矩阵，帧率数据来自这一轮；它的内存含会话残留，只作上界观测
#   mem 轮（N 次）  —— 干净启动直接进驻留，内存稳态来自这几轮
#
# 两条理由要分开看：
#   ① 内存门槛要用**稳态**定，不能用「刚跑完 5 次组件创建/销毁 + 铺过 5000 行 DOM」之后的残留定
#   ② 内存要跨**多次启动**取值 —— 单次启动的数字受 WebView2 缓存状态影响，实测能差 8%
$runs = [Collections.Generic.List[object]]::new()
foreach ($configKey in $Configs) {
    $runs.Add((Invoke-S3Run -ConfigKey $configKey -Config $ConfigMatrix[$configKey] -Mode 'fps' -Round 1))
    for ($round = 1; $round -le $MemoryRounds; $round++) {
        $runs.Add((Invoke-S3Run -ConfigKey $configKey -Config $ConfigMatrix[$configKey] -Mode 'mem' -Round $round))
    }
}

# ── 跨启动的内存汇总 ──────────────────────────────────────────────────
# 门槛按「多轮启动里的最差观测值」定：用户看到的是任务管理器上那个峰值，
# 不是我们挑出来的中位数。中位数一并给出，用来判断峰值是常态还是离群。
function Get-MemorySummary {
    param([string]$ConfigKey, [string]$Mode)

    $subset = @($runs | Where-Object { $_.config -eq $ConfigKey -and $_.mode -eq $Mode })
    if ($subset.Count -eq 0) { return $null }
    $medians = [double[]]@($subset | ForEach-Object { $_.memory.treePrivateWorkingSetMedianBytes })
    $maxes = [double[]]@($subset | ForEach-Object { $_.memory.treePrivateWorkingSetMaxBytes })
    $roots = [double[]]@($subset | ForEach-Object { $_.memory.rootPrivateWorkingSetMedianBytes })
    $perRoundMax = [long](($maxes | Measure-Object -Maximum).Maximum)
    $perRoundMin = [long](($maxes | Measure-Object -Minimum).Minimum)
    return [ordered]@{
        config = $ConfigKey
        mode = $Mode
        rounds = $subset.Count
        # 跨轮次：中位取各轮中位的中位，峰值取各轮峰值的最大
        treePrivateWorkingSetMedianBytes = [long](Get-Median -Values $medians)
        treePrivateWorkingSetMaxBytes = $perRoundMax
        treePrivateWorkingSetMaxSpreadBytes = $perRoundMax - $perRoundMin
        launchVariancePercent = if ($perRoundMin -gt 0) { ($perRoundMax - $perRoundMin) / [double]$perRoundMin * 100.0 } else { $null }
        rootPrivateWorkingSetMedianBytes = [long](Get-Median -Values $roots)
        perRoundMaxBytes = @($maxes | ForEach-Object { [long]$_ })
        roleBreakdown = @($subset[0].memory.roleBreakdown)
        processCount = [int]$subset[0].memory.processCount
    }
}

$memorySummary = @(
    foreach ($configKey in $Configs) {
        foreach ($m in @('mem', 'fps')) {
            $summary = Get-MemorySummary -ConfigKey $configKey -Mode $m
            if ($null -ne $summary) { [pscustomobject]$summary }
        }
    }
)

$measurementTimer.Stop()
$cpuAfter = Get-CpuTimeSnapshot
$topCpu = @(Get-TopCpuConsumers -Before $cpuBefore -After $cpuAfter `
        -ElapsedSeconds $measurementTimer.Elapsed.TotalSeconds -LogicalCores $logicalCores -ExcludeProcessIds @($PID))

# ── 验收门禁 ──────────────────────────────────────────────────────────
#
# 门禁只按**默认配置（GPU 开）**判定 —— 那才是产品要发布的形态。
# GPU 关是 03 §10.4 里待评估的**选项**，它的数字进报告、参与决策，但不作门禁：
# 让一个候选优化方案的表现去决定 Spike 是否通过，等于把结论和选项搅在一起。
$primary = $runs | Where-Object { $_.config -eq 'gpu-on' -and $_.mode -eq 'fps' } | Select-Object -First 1
if ($null -eq $primary) {
    $primary = @($runs | Where-Object { $_.mode -eq 'fps' })[0]
    Write-Warning "没有跑 gpu-on 的帧率轮，门禁改按 $($primary.config) 判定，结论不可直接引用。"
}
$primaryAcceptance = $primary.frontend.acceptance

function Get-GateRow {
    param(
        [string]$Id,
        [string]$Scope,
        [string]$Source,
        [string]$Criterion,
        [object]$Budget,
        [object]$Measured,
        [string]$Unit,
        [object]$Pass
    )
    return [pscustomobject][ordered]@{
        id = $Id
        scope = $Scope
        source = $Source
        criterion = $Criterion
        budget = $Budget
        measured = $Measured
        unit = $Unit
        pass = $Pass
    }
}

$firstPaint5000 = $primaryAcceptance.'S3 首渲 5000 节点 < 100ms'
$scroll5000 = $primaryAcceptance.'S3 滚动 5000 节点 60fps'
$tree1000 = $primaryAcceptance.'PRD 集合树 1000 请求 < 100ms'

$gates = @(
    Get-GateRow -Id 'S3-首渲' -Scope 'spike' -Source '03 文档 §10.1' `
        -Criterion '自研虚拟树 5000 节点首次渲染 < 100 ms（数据就绪 → 像素上屏）' `
        -Budget 100.0 -Measured ([double]$firstPaint5000.measuredMs) -Unit 'ms' -Pass ([bool]$firstPaint5000.pass)
    Get-GateRow -Id 'S3-帧率' -Scope 'spike' -Source '03 文档 §10.1' `
        -Criterion ('滚动 60fps → 口径：' + [string]$scroll5000.criterion) `
        -Budget 5.0 -Measured ([double]$scroll5000.droppedPct) -Unit '% 掉帧' -Pass ([bool]$scroll5000.pass)
    Get-GateRow -Id 'PRD-集合树' -Scope 'prd' -Source 'PRD §5.1' `
        -Criterion '1000 个请求的集合首次展开 < 100 ms' `
        -Budget 100.0 -Measured ([double]$tree1000.measuredMs) -Unit 'ms' -Pass ([bool]$tree1000.pass)
)

$failed = @($gates | Where-Object { $_.pass -eq $false })
$verdict = if ($failed.Count -gt 0) { 'FAIL' } else { 'PASS' }

# ── GPU 开/关对照 ─────────────────────────────────────────────────────
#
# 内存差值用 **mem 轮**（干净驻留）算，帧率差值用 **fps 轮**算。
# 混着用会拿「含会话残留的内存」去和「稳态内存」比，得出的节省量是假的。
$onFps = $runs | Where-Object { $_.config -eq 'gpu-on' -and $_.mode -eq 'fps' } | Select-Object -First 1
$offFps = $runs | Where-Object { $_.config -eq 'gpu-off' -and $_.mode -eq 'fps' } | Select-Object -First 1
$onMem = $memorySummary | Where-Object { $_.config -eq 'gpu-on' -and $_.mode -eq 'mem' } | Select-Object -First 1
$offMem = $memorySummary | Where-Object { $_.config -eq 'gpu-off' -and $_.mode -eq 'mem' } | Select-Object -First 1
$gpuComparison = $null
if ($null -ne $onFps -and $null -ne $offFps -and $null -ne $onMem -and $null -ne $offMem) {
    $onMax = [double]$onMem.treePrivateWorkingSetMaxBytes
    $offMax = [double]$offMem.treePrivateWorkingSetMaxBytes
    $onDropPct = [double]$onFps.frontend.acceptance.'S3 滚动 5000 节点 60fps'.droppedPct
    $offDropPct = [double]$offFps.frontend.acceptance.'S3 滚动 5000 节点 60fps'.droppedPct
    $onFpsValue = [double]$onFps.frontend.acceptance.'S3 滚动 5000 节点 60fps'.fps
    $offFpsValue = [double]$offFps.frontend.acceptance.'S3 滚动 5000 节点 60fps'.fps
    # 单步主线程成本：帧间隔饱和时唯一能看出 GPU 开关代价的数字
    $onStep = @($onFps.frontend.cells | Where-Object { $_.scaleKey -eq 's3-5000' -and $_.impl -eq 'virtual' })[0]
    $offStep = @($offFps.frontend.cells | Where-Object { $_.scaleKey -eq 's3-5000' -and $_.impl -eq 'virtual' })[0]
    $gpuComparison = [ordered]@{
        question = '03 文档 §10.4 选项 (b)：关掉硬件加速换内存，值不值？'
        memoryBasis = "mem 轮（干净驻留）$MemoryRounds 次启动里的进程树私有常驻最差观测值"
        memorySavedBytes = [long]($onMax - $offMax)
        memorySavedPercent = if ($onMax -gt 0) { ($onMax - $offMax) / $onMax * 100.0 } else { $null }
        gpuProcessStillPresentWhenDisabled = [bool](
            ($runs | Where-Object { $_.config -eq 'gpu-off' -and $_.mode -eq 'mem' } | Select-Object -First 1).gpu.gpuProcessPresent
        )
        droppedPctOn = $onDropPct
        droppedPctOff = $offDropPct
        droppedPctDelta = $offDropPct - $onDropPct
        fpsOn = $onFpsValue
        fpsOff = $offFpsValue
        fpsDelta = $offFpsValue - $onFpsValue
        stepCostMedianMsOn = if ($null -ne $onStep) { [double]$onStep.stepCost.medianMs } else { $null }
        stepCostMedianMsOff = if ($null -ne $offStep) { [double]$offStep.stepCost.medianMs } else { $null }
        firstPaintPatchMsOn = if ($null -ne $onStep) { [double]$onStep.patchMs } else { $null }
        firstPaintPatchMsOff = if ($null -ne $offStep) { [double]$offStep.patchMs } else { $null }
        stillPassesFrameGateWithGpuOff = ($offDropPct -lt 5.0)
    }
}

$notMeasured = @(
    '真实滚轮/触控板滚动：帧率由应用内主线程写 scrollTop 驱动，不经过合成器线程，也不含输入事件延迟。这是更悲观的一侧，但不能宣称为「用户实际手感」。'
    'paint / composite 的耗时：单步成本只含主线程侧（patch + 强制 layout）。GPU 开关真正影响的合成阶段在另一条线程上，本量测拿不到它的逐帧耗时，只能通过「有没有掉帧」间接判断。'
    '50 标签内存（PRD §5.1 < 400MB）：本轮驻留条件是 3 标签 + 一棵 5000 节点集合树，未测 50 标签，也未实现标签虚拟化。'
    '磁盘占用（安装后）：本脚本用 --no-bundle 构建，不产出安装包，体积一项仍以 S1 为准。'
    'macOS：WKWebView 的进程模型、内存口径与合成实现都不同，本报告数字不可套用。'
    '长时间运行的内存增长：驻留窗口只有十几秒，不能回答「开一天会不会涨」。'
    'GPU 关配置下的启动耗时：本脚本不量启动，S1 才是启动的量测台。'
)

$appVersion = (Get-Content -LiteralPath (Join-Path $Root 'src-tauri\tauri.conf.json') -Raw -Encoding UTF8 | ConvertFrom-Json).version

$result = [ordered]@{
    spike = 'S3'
    at = [DateTimeOffset]::Now.ToString('o')
    verdict = $verdict
    platform = Get-MeasurementPlatform -AppVersion ([string]$appVersion)
    config = [ordered]@{
        configs = @($Configs)
        idleSettleMs = $IdleSettleMs
        memorySamples = $MemorySamples
        sampleIntervalMs = $SampleIntervalMs
        memoryRounds = $MemoryRounds
        skipBuild = [bool]$SkipBuild
        executable = [IO.Path]::GetRelativePath($Root, $Executable).Replace('\', '/')
        autorunEnvVar = 'APOGEE_SPIKE_AUTORUN'
        resultsDirEnvVar = 'APOGEE_SPIKE_RESULTS_DIR'
        browserArgsEnvVar = 'WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'
        phaseProtocol = 'fps → memory-hold → done；帧率阶段量测台不采样，避免自己制造掉帧'
    }
    harness = [ordered]@{
        measurementSeconds = [double]$measurementTimer.Elapsed.TotalSeconds
        cpuGateMaxPercent = $MaxCpuBeforeLaunchPercent
        hotConfigs = @($runs | Where-Object { -not $_.cpuSettledBeforeLaunch } | ForEach-Object { [string]$_.config })
        topCpuConsumers = @($topCpu)
    }
    gates = @($gates)
    memorySummary = @($memorySummary)
    gpuComparison = $gpuComparison
    notMeasured = @($notMeasured)
    runs = @($runs)
}

[IO.File]::WriteAllText($JsonPath, (ConvertTo-Json -InputObject $result -Depth 12), $Utf8NoBom)

# 阶段文件是过程产物，跑完就清掉，避免下次运行读到上一轮的残留
Remove-Item -LiteralPath $PhasePath -Force -ErrorAction SilentlyContinue

Write-Host ''
Write-Host "[S3] $verdict"
foreach ($gate in $gates) {
    $state = if ($null -eq $gate.pass) { '未量测' } elseif ($gate.pass) { 'PASS' } else { 'FAIL' }
    Write-Host ("  [{0}] {1,-12} {2:N2}{3} / 预算 {4}{3}" -f $state, $gate.id, $gate.measured, $gate.unit, $gate.budget)
}
foreach ($summary in $memorySummary) {
    Write-Host ("  内存 {0,-8} {1,-4} {2} 轮：中位 {3} / 最差 {4}（跨轮波动 {5:N1}%）" -f
        $summary.config, $summary.mode, $summary.rounds,
        (Format-Mb $summary.treePrivateWorkingSetMedianBytes),
        (Format-Mb $summary.treePrivateWorkingSetMaxBytes),
        $summary.launchVariancePercent)
}
if ($null -ne $gpuComparison) {
    Write-Host ("[S3] 关 GPU：省内存 {0} / 掉帧 {1:N2}% → {2:N2}%（gpu-process {3}）" -f
        (Format-Mb $gpuComparison.memorySavedBytes), $gpuComparison.droppedPctOn, $gpuComparison.droppedPctOff,
        $(if ($gpuComparison.gpuProcessStillPresentWhenDisabled) { '仍在' } else { '已消失' }))
}
Write-Host "[S3] 数据：spikes/results/s3.json"

# 报告渲染交给 Node（字符串处理方便，且与 S2 的 render-s2-report.mjs 一致）。
# 放在退出码之前跑：门槛没过也要有报告可读。
Push-Location $Root
try {
    & node (Join-Path $Root 'scripts\render-s3-report.mjs')
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "渲染 s3-report.md 失败，退出码 $LASTEXITCODE。原始数据仍在 s3.json。"
    }
}
finally {
    Pop-Location
}

if ($failed.Count -gt 0) { exit 1 }
