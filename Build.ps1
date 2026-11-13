# Modify:锁定依赖、回归验证与独立Windows发布；不读取或复制个人数据。$saving-ai-agent-tokens
# 功能：测试并构建当前工程；无参数，输出dist运行目录、日志和运行包。
$ErrorActionPreference = 'Stop'
Push-Location -LiteralPath $PSScriptRoot
try {
    $artifactPath = Join-Path $PSScriptRoot 'artifacts'
    $distPath = Join-Path $PSScriptRoot 'dist'
    New-Item -ItemType Directory -Force -Path $artifactPath, $distPath | Out-Null
    # Modify:兼容Windows PowerShell将cargo进度写到stderr的行为；实际失败仍按退出码中止。
    $ErrorActionPreference = 'Continue'
    $testOutput = & cargo test --release --target x86_64-pc-windows-msvc --locked --all-targets -- --test-threads=1 2>&1
    $testExit = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    $testOutput | Set-Content -LiteralPath (Join-Path $artifactPath 'tests.log') -Encoding utf8
    $totals = [regex]::Matches(($testOutput -join "`n"), 'test result: \w+\. (\d+) passed; (\d+) failed;')
    $passed = 0
    $failed = 0
    foreach ($result in $totals) { $passed += [int]$result.Groups[1].Value; $failed += [int]$result.Groups[2].Value }
    @{ checked_at = (Get-Date).ToUniversalTime().ToString('o'); passed = $passed; failed = $failed; exit_code = $testExit } |
        ConvertTo-Json | Set-Content -LiteralPath (Join-Path $artifactPath 'test-results.json') -Encoding utf8
    if ($testExit -ne 0) { throw '自动化测试未通过，详见 artifacts\tests.log。' }
    $ErrorActionPreference = 'Continue'
    & cargo build --release --target x86_64-pc-windows-msvc --locked --bin WorkHoursRecord
    $buildExit = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($buildExit -ne 0) { throw '发布构建失败。' }
    # Modify:仅复制明确列出的公共交付文件，避免把本机凭证、缓存或历史调试产物打入运行包。
    Copy-Item -LiteralPath 'target\x86_64-pc-windows-msvc\release\WorkHoursRecord.exe' -Destination $distPath -Force
    Copy-Item -LiteralPath 'README.md' -Destination (Join-Path $distPath '使用说明.md') -Force
    Copy-Item -LiteralPath 'docs\验证结果.md' -Destination $distPath -Force
    Copy-Item -LiteralPath (Join-Path $artifactPath 'test-results.json') -Destination $distPath -Force
    # Modify:随包保留已取得的验证报告；不从用户数据目录采集身份、凭证或原始打卡缓存。
    # Modify:公开构建仅附加布局检查；个人考勤审计和设备实测报告保留本机，不加入分发包。
    # foreach ($report in @('real-integration.json', 'performance.json', 'verification\layout-checks.json')) {
    foreach ($report in @('verification\layout-checks.json')) {
        $reportPath = Join-Path $artifactPath $report
        if (Test-Path -LiteralPath $reportPath) { Copy-Item -LiteralPath $reportPath -Destination $distPath -Force }
    }
    Compress-Archive -LiteralPath $distPath -DestinationPath (Join-Path $artifactPath 'WorkHoursRecord-rust-gpui-win-x64-v2.0.0.zip') -Force
    $sourceFiles = @('.cargo', 'assets', 'examples', 'src', 'tests', 'docs', '.gitignore', 'Cargo.toml', 'Cargo.lock', 'build.rs', 'Build.ps1', 'README.md') |
        ForEach-Object { Join-Path $PSScriptRoot $_ }
    Compress-Archive -LiteralPath $sourceFiles -DestinationPath (Join-Path $artifactPath 'WorkHoursRecord-rust-gpui-source-v2.0.0.zip') -Force
    Write-Output "完成：$distPath；测试通过 $passed 项。"
} finally { Pop-Location }
