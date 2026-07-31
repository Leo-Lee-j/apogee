# Spike S6 语料库获取脚本
#
# 目的：拿到「真实的」Postman 导出文件，用官方 v2.1.0 draft-07 schema 去校验，
#      验证 PRD §6.3 / 决策 D3 的「对齐最新版」是否可以走严格校验路线。
#
# 语料来源分两类，必须都覆盖：
#   A. 官方 SDK / Newman 的测试 fixture —— 特性覆盖广（各种 body、auth、script 形态）
#   B. 真实用户导出的集合 —— 反映客户端实际写出的字段，可能包含 schema 未声明的内容
#
# 用法：pwsh -File fetch-corpus.ps1

$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$corpus = Join-Path $root 'corpus'
New-Item -ItemType Directory -Force -Path $corpus | Out-Null

$headers = @{ 'User-Agent' = 'apogee-spike-s6' }
$ok = 0
$fail = 0

function Save-File($url, $name, $tag) {
    $dest = Join-Path $corpus "$tag-$name"
    if (Test-Path $dest) { Write-Host "SKIP $tag-$name"; return $true }
    try {
        Invoke-WebRequest -Uri $url -OutFile $dest -Headers $headers -UseBasicParsing -TimeoutSec 30
        # 只保留能解析为 JSON 且看起来像 collection 的文件
        $raw = Get-Content $dest -Raw -Encoding UTF8
        $j = $raw | ConvertFrom-Json -ErrorAction Stop
        if (-not $j.info) { Remove-Item $dest -Force; Write-Host "DROP $name (无 info 字段，不是 collection)"; return $false }
        $size = (Get-Item $dest).Length
        Write-Host "OK   $tag-$name  $size bytes"
        return $true
    } catch {
        if (Test-Path $dest) { Remove-Item $dest -Force }
        Write-Host "FAIL $name : $($_.Exception.Message)"
        return $false
    }
}

# ── A. 官方 fixture：列目录后批量拉取 ────────────────────────────────
$apiTargets = @(
    @{ repo = 'postmanlabs/newman';               path = 'test/fixtures/run';         tag = 'newman' },
    @{ repo = 'postmanlabs/postman-collection';   path = 'test/fixtures';             tag = 'sdk' }
)

foreach ($t in $apiTargets) {
    $api = "https://api.github.com/repos/$($t.repo)/contents/$($t.path)"
    try {
        $listing = Invoke-RestMethod -Uri $api -Headers $headers -TimeoutSec 30
    } catch {
        Write-Host "!! 目录列取失败 $($t.repo)/$($t.path) : $($_.Exception.Message)"
        continue
    }
    $jsons = $listing | Where-Object { $_.type -eq 'file' -and $_.name -like '*.json' } | Select-Object -First 40
    foreach ($f in $jsons) {
        if (Save-File $f.download_url $f.name $t.tag) { $ok++ } else { $fail++ }
    }
}

# ── B. 真实用户导出 ──────────────────────────────────────────────────
$realWorld = @(
    @{ url = 'https://gist.githubusercontent.com/peterrozsasi/3506abeb5eea65123e2c9845e4426d3c/raw'; name = 'inardua-integration.json' },
    @{ url = 'https://gist.githubusercontent.com/mudassaralichouhan/1f075942c7d9dcf2a372cc1ffa10b5d9/raw'; name = 'zatca.json' }
)
foreach ($r in $realWorld) {
    if (Save-File $r.url $r.name 'real') { $ok++ } else { $fail++ }
}

Write-Host ""
Write-Host "语料库结果：成功 $ok，失败/丢弃 $fail"
Write-Host "目录：$corpus"
Get-ChildItem $corpus -Filter *.json | Measure-Object | ForEach-Object { Write-Host "最终文件数：$($_.Count)" }
