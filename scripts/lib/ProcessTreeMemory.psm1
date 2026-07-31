<#
.SYNOPSIS
Windows 进程树内存与量测环境噪声的共享量测原语。

.DESCRIPTION
S1（空壳启动与空闲内存）和 S3（虚拟树帧率 + 负载下内存）必须用**同一套内存口径**，
否则两个报告的数字不可比，而「内存门槛」正是要拿这两个数字去定的。

S1 的工程教训第 4 条说得很直接：同一组数据按三种口径分别是 146.7 / 439.1 / 215.9 MB，
其中两种能让结论完全反过来。两个量测台各自抄一份口径实现，就是给这种事留后门 ——
改了一边忘了另一边，谁都不会发现。所以这里只留一份。

导出的原语分三类：
  · 进程树：Get-ProcessTreeIds / Get-TreeMemorySample / Stop-ProcessTree
  · 环境噪声：Get-SystemCpuLoadPercent / Wait-ForQuietCpu / Get-TopCpuConsumers
  · 格式化与统计：Get-Median / Get-Percentile / Format-Mb / Format-Ms / ...
#>

Set-StrictMode -Version Latest

# 进程树只应包含这两个映像：Rust 宿主 + WebView2 的浏览器/GPU/渲染/工具进程。
# 这是内存统计与「杀进程树」共用的白名单：统计时越界就报错，杀进程时越界就跳过。
# 存在的理由见 Get-ProcessTreeIds 的注释（PID 复用会伪造出父子关系）。
$script:ExpectedProcessNames = @('apogee', 'msedgewebview2')

function Set-ProcessTreeWhitelist {
    <#
    .SYNOPSIS
    覆盖进程树映像名白名单。改产品名或引入新的子进程映像时用。
    #>
    param([Parameter(Mandatory)][string[]]$ImageNames)
    $script:ExpectedProcessNames = @($ImageNames)
}

function Get-ProcessTreeWhitelist {
    return @($script:ExpectedProcessNames)
}

function Get-Median {
    param([double[]]$Values)
    if ($Values.Count -eq 0) { return $null }
    $sorted = @($Values | Sort-Object)
    $middle = [Math]::Floor($sorted.Count / 2)
    if ($sorted.Count % 2 -eq 1) { return [double]$sorted[$middle] }
    return ([double]$sorted[$middle - 1] + [double]$sorted[$middle]) / 2.0
}

function Get-Percentile {
    param(
        [double[]]$Values,
        [ValidateRange(0.0, 1.0)][double]$Percentile
    )
    if ($Values.Count -eq 0) { return $null }
    $sorted = @($Values | Sort-Object)
    $index = [Math]::Max(0, [Math]::Ceiling($Percentile * $sorted.Count) - 1)
    return [double]$sorted[$index]
}

# WMI 偶发「调用已取消」，一次失败就中断整轮量测太脆。
# 重试 3 次；仍失败则抛出，由上层决定 —— 但不要静默返回空表，
# 那会让所有进程都「查不到私有工作集」而被当成 0。
function Invoke-CimQueryWithRetry {
    param(
        [scriptblock]$Query,
        [int]$Attempts = 3,
        [int]$DelayMs = 400
    )

    for ($attempt = 1; $attempt -le $Attempts; $attempt++) {
        try {
            return & $Query
        }
        catch {
            if ($attempt -eq $Attempts) { throw }
            Write-Warning "CIM 查询失败（第 $attempt/$Attempts 次），$DelayMs ms 后重试：$($_.Exception.Message)"
            Start-Sleep -Milliseconds $DelayMs
        }
    }
}

function Get-ProcessSnapshot {
    return @(Invoke-CimQueryWithRetry -Query {
            Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name, CreationDate, CommandLine
        })
}

<#
.SYNOPSIS
从 root 出发按父子关系收集进程树，并用创建时间校验每一条父子边。

.DESCRIPTION
Windows 会复用 PID，而 Win32_Process.ParentProcessId 只是一个数字，不带世代信息：
真正的父进程退出后，它的 PID 可能被我们的 apogee.exe 或某个 WebView2 子进程占用，
于是一批毫不相关的进程看起来就成了「我们的后代」。

这不是理论风险 —— S1 早期版本就因此把 13 个无关进程（另一个应用的进程组）
计入了内存，单轮内存从 147MB 跳到 430MB；更糟的是 Stop-ProcessTree 把它们杀了。

真正的子进程一定不早于父进程创建。逐边校验 CreationDate 即可切断这种伪边。
拿不到创建时间的候选进程不静默放行，而是回报给调用方，由调用方决定是否中止。
#>
function Get-ProcessTreeIds {
    param(
        [int]$RootProcessId,
        [object[]]$Snapshot,
        [ref]$UnvalidatedIds
    )

    $all = if ($null -eq $Snapshot) { Get-ProcessSnapshot } else { $Snapshot }

    $byId = @{}
    $childrenOf = @{}
    foreach ($candidate in $all) {
        $candidateId = [int]$candidate.ProcessId
        $byId[$candidateId] = $candidate
        $parentId = [int]$candidate.ParentProcessId
        # 自指（Idle/System 上会出现）会让遍历把整张表吞进来
        if ($candidateId -eq $parentId) { continue }
        if (-not $childrenOf.ContainsKey($parentId)) {
            $childrenOf[$parentId] = [Collections.Generic.List[object]]::new()
        }
        $childrenOf[$parentId].Add($candidate)
    }

    $unvalidated = [Collections.Generic.List[int]]::new()
    $found = [Collections.Generic.HashSet[int]]::new()
    [void]$found.Add($RootProcessId)
    $queue = [Collections.Generic.Queue[int]]::new()
    $queue.Enqueue($RootProcessId)

    while ($queue.Count -gt 0) {
        $parentId = $queue.Dequeue()
        if (-not $childrenOf.ContainsKey($parentId)) { continue }
        $parentRecord = $byId[$parentId]
        $parentCreated = if ($null -ne $parentRecord) { $parentRecord.CreationDate } else { $null }

        foreach ($child in $childrenOf[$parentId]) {
            $childId = [int]$child.ProcessId
            if ($found.Contains($childId)) { continue }
            if ($null -eq $parentCreated -or $null -eq $child.CreationDate) {
                # 无法校验就不纳入，但要让上层知道，绝不静默
                $unvalidated.Add($childId)
                continue
            }
            if ($child.CreationDate -lt $parentCreated) {
                # 子进程早于父进程 → 这条父子边来自 PID 复用，是伪边
                continue
            }
            if ($found.Add($childId)) {
                $queue.Enqueue($childId)
            }
        }
    }

    if ($null -ne $UnvalidatedIds) {
        $UnvalidatedIds.Value = @($unvalidated)
    }
    return @($found | ForEach-Object { [int]$_ })
}

# 私有工作集（Working Set - Private）只能从性能计数器拿：
# Process 类的 WorkingSet64 含共享页，PrivateMemorySize64 是提交量而非常驻量。
# 见 S1 报告里「WorkingSet 求和会重复计数」那条发现。
function Get-PrivateWorkingSetMap {
    $entries = @(Invoke-CimQueryWithRetry -Query { Get-CimInstance Win32_PerfRawData_PerfProc_Process })
    $map = @{}
    foreach ($entry in $entries) {
        $map[[int]$entry.IDProcess] = [long]$entry.WorkingSetPrivate
    }
    return $map
}

function Get-ProcessRole {
    param([object]$Info)

    if ($null -eq $Info) { return 'unknown' }
    if ($Info.Name -ne 'msedgewebview2.exe') { return 'host(rust)' }
    if ($Info.CommandLine -match '--type=([a-z-]+)') { return [string]$Matches[1] }
    return 'browser'
}

function Get-TreeMemorySample {
    <#
    .SYNOPSIS
    对进程树取一次内存快照。主口径是**私有工作集**（等于任务管理器「内存」列）。

    .PARAMETER IncludeBreakdown
    附带逐进程明细（角色 + 各口径），用于「内存花在哪」一节。

    .PARAMETER IncludeCommandLines
    附带各进程的命令行。S3 要靠它验证 `--disable-gpu` 这类开关**真的**送到了
    WebView2 浏览器进程，而不是只在环境变量里写了一遍。
    #>
    param(
        [int]$RootProcessId,
        [switch]$IncludeBreakdown,
        [switch]$IncludeCommandLines
    )

    $snapshot = Get-ProcessSnapshot
    $unvalidated = @()
    $ids = @(Get-ProcessTreeIds -RootProcessId $RootProcessId -Snapshot $snapshot -UnvalidatedIds ([ref]$unvalidated))
    if ($unvalidated.Count -gt 0) {
        throw ("有 {0} 个候选进程拿不到创建时间、无法校验父子关系（PID {1}）。拒绝在无法确认归属的情况下计入内存。" -f
            $unvalidated.Count, ($unvalidated -join ','))
    }
    # PID → 快照记录 的索引。
    #
    # 原来这里对每个进程都 `$snapshot | Where-Object {...}` 扫一遍全表，是 O(树大小 × 进程总数)。
    # 机器上有 500+ 个进程时单次采样要一两秒，而 S3 的驻留窗口是有限的 ——
    # 采样跑出窗口就会被判为无效数据。量测台自己的开销也是噪声源，越小越好。
    $byPid = @{}
    foreach ($record in $snapshot) { $byPid[[int]$record.ProcessId] = $record }

    # 第二道防线：进程树只可能是 apogee.exe + msedgewebview2.exe。
    # 出现别的映像名就说明遍历越界了（历史上正是 PID 复用导致），必须立刻失败，
    # 而不是把别人的内存算进我们的预算。
    $unexpected = @(
        foreach ($processId in $ids) {
            $info = $byPid[[int]$processId]
            if ($null -eq $info) { continue }
            $imageName = [IO.Path]::GetFileNameWithoutExtension([string]$info.Name)
            if ($script:ExpectedProcessNames -notcontains $imageName) { "$imageName(PID $processId)" }
        }
    )
    if ($unexpected.Count -gt 0) {
        throw ("进程树里出现非预期映像：{0}。预期只有 {1}。" -f ($unexpected -join '、'), ($script:ExpectedProcessNames -join '、'))
    }
    $privateWorkingSets = Get-PrivateWorkingSetMap

    $rows = [Collections.Generic.List[object]]::new()
    foreach ($processId in $ids) {
        $process = Get-Process -Id $processId -ErrorAction SilentlyContinue
        if ($null -eq $process) { continue }
        $info = $byPid[[int]$processId]
        # 计数器里查不到就记 $null，绝不当 0 —— 否则少算的内存会静默变成「达标」
        $privateWorkingSet = if ($privateWorkingSets.ContainsKey($processId)) {
            [long]$privateWorkingSets[$processId]
        }
        else {
            $null
        }
        $row = [ordered]@{
            processId = $processId
            name = [string]$process.ProcessName
            role = Get-ProcessRole -Info $info
            privateWorkingSetBytes = $privateWorkingSet
            workingSetBytes = [long]$process.WorkingSet64
            commitBytes = [long]$process.PrivateMemorySize64
        }
        if ($IncludeCommandLines) {
            $row['commandLine'] = if ($null -ne $info) { [string]$info.CommandLine } else { $null }
        }
        $rows.Add([pscustomobject]$row)
    }
    if ($rows.Count -eq 0) {
        throw "进程树已退出（root PID=$RootProcessId）。"
    }
    $missing = @($rows | Where-Object { $null -eq $_.privateWorkingSetBytes })
    if ($missing.Count -gt 0) {
        throw ("取不到私有工作集计数器的进程：{0}。拒绝按 0 计入。" -f (($missing | ForEach-Object { $_.processId }) -join ','))
    }

    $rootRow = $rows | Where-Object { $_.processId -eq $RootProcessId } | Select-Object -First 1
    if ($null -eq $rootRow) {
        throw "root 进程 $RootProcessId 已退出。"
    }

    $sample = [ordered]@{
        sampledAt = [DateTimeOffset]::Now.ToString('o')
        processCount = $rows.Count
        # 私有常驻：内存主口径，等于任务管理器「内存」列
        treePrivateWorkingSetBytes = [long](($rows | Measure-Object -Property privateWorkingSetBytes -Sum).Sum)
        # 提交量：包含已提交但未驻留的页，比常驻大
        treeCommitBytes = [long](($rows | Measure-Object -Property commitBytes -Sum).Sum)
        # WorkingSet 求和：含共享页，跨进程重复计数，只作为「不能这么算」的对照
        treeWorkingSetSumBytes = [long](($rows | Measure-Object -Property workingSetBytes -Sum).Sum)
        rootPrivateWorkingSetBytes = [long]$rootRow.privateWorkingSetBytes
        rootCommitBytes = [long]$rootRow.commitBytes
        rootWorkingSetBytes = [long]$rootRow.workingSetBytes
    }
    if ($IncludeBreakdown) {
        $sample['processes'] = @($rows | Sort-Object -Property privateWorkingSetBytes -Descending)
    }
    return $sample
}

function Stop-ProcessTree {
    param([int]$RootProcessId)

    # 白名单在这里是**安全措施**，不是优化。早期版本因 PID 复用把无关进程
    # 判成后代，然后真的把用户正在用的另一个应用杀掉了。
    # 量测台可以测错再重跑，但绝不允许动被测对象之外的进程。
    $snapshot = Get-ProcessSnapshot
    $ids = @(Get-ProcessTreeIds -RootProcessId $RootProcessId -Snapshot $snapshot)
    $byPid = @{}
    foreach ($record in $snapshot) { $byPid[[int]$record.ProcessId] = $record }
    $killable = [Collections.Generic.List[int]]::new()
    foreach ($processId in $ids) {
        $info = $byPid[[int]$processId]
        if ($null -eq $info) { continue }
        $imageName = [IO.Path]::GetFileNameWithoutExtension([string]$info.Name)
        if ($script:ExpectedProcessNames -contains $imageName) {
            $killable.Add($processId)
        }
        else {
            Write-Warning "跳过非预期进程 $imageName (PID $processId)：不在进程树白名单内，不予终止。"
        }
    }

    foreach ($processId in $killable | Where-Object { $_ -ne $RootProcessId }) {
        Stop-Process -Id $processId -Force -ErrorAction SilentlyContinue
    }
    if ($killable -contains $RootProcessId) {
        Stop-Process -Id $RootProcessId -Force -ErrorAction SilentlyContinue
    }
}

function Format-Mb {
    param([object]$Bytes)
    if ($null -eq $Bytes) { return '—' }
    return '{0:N2} MB' -f ([double]$Bytes / 1MB)
}

function Format-Ms {
    param([object]$Milliseconds)
    if ($null -eq $Milliseconds) { return '—' }
    return '{0:N1} ms' -f [double]$Milliseconds
}

function Format-Pct {
    param([object]$Measured, [double]$Budget)
    if ($null -eq $Measured -or $Budget -le 0) { return '—' }
    return '{0:N0}%' -f ([double]$Measured / $Budget * 100.0)
}

# 余量可能是负数。写成「只剩 -3.04 MB 余量」是在用中性措辞掩盖超标，
# 报告里必须直接说「已超线」。
function Format-Headroom {
    param([long]$BudgetBytes, [long]$MeasuredBytes)
    if ($MeasuredBytes -gt $BudgetBytes) {
        return '已超线 ' + (Format-Mb ($MeasuredBytes - $BudgetBytes))
    }
    return '余量 ' + (Format-Mb ($BudgetBytes - $MeasuredBytes))
}

# WebView2 Runtime 的版本不在进程里，只能问 EdgeUpdate 注册表。
# 这个数字必须进报告：同一份二进制在不同 Runtime 版本上的内存差异是真实存在的。
function Get-WebView2Version {
    $clientKeys = @(
        'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
        'HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
    )
    foreach ($key in $clientKeys) {
        $entry = Get-ItemProperty -LiteralPath $key -Name 'pv' -ErrorAction SilentlyContinue
        if ($null -ne $entry -and -not [string]::IsNullOrWhiteSpace($entry.pv)) {
            return [string]$entry.pv
        }
    }
    return $null
}

# 启动耗时与帧率都对 CPU 竞争极其敏感（S1 实测同一份二进制在有/无背景负载下启动差 2 倍）。
# 不记录量测期间的机器负载，报告出来的数字就无法判断是否可引用。
function Get-SystemCpuLoadPercent {
    # 优先用 % Processor Time(_Total)，与任务管理器口径一致；
    # 取不到再退回 Win32_Processor.LoadPercentage（更粗，但总是有）。
    $total = Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor -Filter "Name='_Total'" -ErrorAction SilentlyContinue
    if ($null -ne $total) {
        $value = @($total)[0].PercentProcessorTime
        if ($null -ne $value) { return [double]$value }
    }
    $loads = @(Get-CimInstance Win32_Processor -ErrorAction SilentlyContinue | ForEach-Object { [double]$_.LoadPercentage })
    if ($loads.Count -eq 0) { return $null }
    return [double](($loads | Measure-Object -Average).Average)
}

<#
.SYNOPSIS
等待全核 CPU 占用降到阈值以下，返回本轮实际的起跑负载。

.DESCRIPTION
不做这一步，量到的就是「机器当时多忙」而不是「应用多快」。
超时后不中止：宁可带着 hot 标记继续量并在报告里说明，也不要因为
机器一直不空闲就交不出数据。
#>
function Wait-ForQuietCpu {
    param(
        [int]$MaxPercent,
        [int]$TimeoutMs
    )

    $timer = [Diagnostics.Stopwatch]::StartNew()
    $load = Get-SystemCpuLoadPercent
    while ($null -ne $load -and $load -gt $MaxPercent -and $timer.ElapsedMilliseconds -lt $TimeoutMs) {
        Start-Sleep -Milliseconds 500
        $load = Get-SystemCpuLoadPercent
    }
    return [pscustomobject][ordered]@{
        loadPercent = $load
        waitedMs = [double]$timer.Elapsed.TotalMilliseconds
        settled = ($null -ne $load -and $load -le $MaxPercent)
    }
}

function Get-CpuTimeSnapshot {
    $snapshot = @{}
    foreach ($process in Get-Process -ErrorAction SilentlyContinue) {
        try {
            $snapshot[[int]$process.Id] = [pscustomobject]@{
                name = [string]$process.ProcessName
                cpuSeconds = [double]$process.TotalProcessorTime.TotalSeconds
            }
        }
        catch {
            # 拿不到 CPU 时间的进程（权限/已退出）直接跳过
        }
    }
    return $snapshot
}

# 用两次快照的 CPU 时间差算出「量测窗口内谁在抢 CPU」，
# 而不是用 Get-Process 的累计 CPU（那是自进程启动以来的总量，会把长跑进程排到最前）。
function Get-TopCpuConsumers {
    param(
        [hashtable]$Before,
        [hashtable]$After,
        [double]$ElapsedSeconds,
        [int]$LogicalCores,
        [int[]]$ExcludeProcessIds,
        [int]$Top = 6
    )

    if ($ElapsedSeconds -le 0) { return @() }
    $excluded = [Collections.Generic.HashSet[int]]::new()
    foreach ($processId in $ExcludeProcessIds) { [void]$excluded.Add($processId) }

    $rows = [Collections.Generic.List[object]]::new()
    foreach ($processId in $After.Keys) {
        if ($excluded.Contains([int]$processId)) { continue }
        $afterEntry = $After[$processId]
        $beforeSeconds = if ($Before.ContainsKey($processId)) { [double]$Before[$processId].cpuSeconds } else { 0.0 }
        $deltaSeconds = [double]$afterEntry.cpuSeconds - $beforeSeconds
        if ($deltaSeconds -le 0.5) { continue }
        $rows.Add([pscustomobject][ordered]@{
            name = [string]$afterEntry.name
            cpuSeconds = [Math]::Round($deltaSeconds, 1)
            # 占「一颗核」的百分比更直观：120% 表示吃掉 1.2 个核
            oneCorePercent = [Math]::Round($deltaSeconds / $ElapsedSeconds * 100.0, 1)
            allCoresPercent = [Math]::Round($deltaSeconds / ($ElapsedSeconds * $LogicalCores) * 100.0, 1)
        })
    }
    return @($rows | Sort-Object -Property cpuSeconds -Descending | Select-Object -First $Top)
}

function Get-ToolVersion {
    param([string]$FileName, [string[]]$ToolArguments)
    try {
        # 显式清零：本函数可能是本次会话第一个外部调用，
        # StrictMode 下读未初始化的 $LASTEXITCODE 会抛。
        $global:LASTEXITCODE = 0
        $output = & $FileName @ToolArguments 2>$null
        if ($LASTEXITCODE -ne 0) { return $null }
        return (@($output) -join ' ').Trim()
    }
    catch {
        return $null
    }
}

function Get-MeasurementPlatform {
    <#
    .SYNOPSIS
    量测环境快照。报告里必须带这一段，否则数字无法判断能不能引用。
    #>
    param([string]$AppVersion)

    $operatingSystem = Get-CimInstance Win32_OperatingSystem
    $processor = @(Get-CimInstance Win32_Processor)[0]
    $computerSystem = Get-CimInstance Win32_ComputerSystem
    return [ordered]@{
        os = [string]$operatingSystem.Caption
        osVersion = [string]$operatingSystem.Version
        cpu = ([string]$processor.Name).Trim()
        logicalCores = [int][Environment]::ProcessorCount
        totalMemoryBytes = [long]$computerSystem.TotalPhysicalMemory
        webview2Runtime = Get-WebView2Version
        rustc = Get-ToolVersion -FileName 'rustc' -ToolArguments @('--version')
        node = Get-ToolVersion -FileName 'node' -ToolArguments @('--version')
        appVersion = $AppVersion
        powerShell = [string]$PSVersionTable.PSVersion
    }
}

Export-ModuleMember -Function @(
    'Set-ProcessTreeWhitelist'
    'Get-ProcessTreeWhitelist'
    'Get-Median'
    'Get-Percentile'
    'Invoke-CimQueryWithRetry'
    'Get-ProcessSnapshot'
    'Get-ProcessTreeIds'
    'Get-PrivateWorkingSetMap'
    'Get-ProcessRole'
    'Get-TreeMemorySample'
    'Stop-ProcessTree'
    'Format-Mb'
    'Format-Ms'
    'Format-Pct'
    'Format-Headroom'
    'Get-WebView2Version'
    'Get-SystemCpuLoadPercent'
    'Wait-ForQuietCpu'
    'Get-CpuTimeSnapshot'
    'Get-TopCpuConsumers'
    'Get-ToolVersion'
    'Get-MeasurementPlatform'
)
