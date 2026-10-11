<#
.SYNOPSIS
Updates a modem that runs OpenUI to the release matching this checkout, from Windows.

.DESCRIPTION
A checkout at version 1.1.0 deploys release v1.1. The script downloads the release
files from GitHub and checks them against sha256sums.txt, prepares the geo data,
and runs scripts/deploy-components.py twice: a dry run, then, once you agree,
the deployment. The modem keeps its agent password and PIN, and components it
already runs at the same checksum are left alone. When the screen program changed,
scripts/verify-screen.py checks the touchscreen afterwards.

Start it with deploy-windows.cmd in the repository root. It only updates an existing
installation: the modem must accept the SSH key on port 2222.

.PARAMETER Gateway
Address of the modem. Remembered; 192.168.0.1 at first.

.PARAMETER SshKey
Private key the modem accepts. Remembered; ~\.ssh\id_ed25519 at first.

.PARAMETER AssetsDir
Folder that already holds the release files, instead of downloading them.

.PARAMETER Repo
GitHub repository that publishes the releases.

.PARAMETER All
Deploy every component, including those the modem already has.

.PARAMETER SkipScreenCheck
Do not check the touchscreen afterwards. Its entry on the web page then stays off
until scripts/verify-screen.py passes.

.PARAMETER Yes
Take the default answer to every question, including deploying after the dry run.
#>

#Requires -Version 5.1
[CmdletBinding()]
param(
    [string]$Gateway,
    [string]$SshKey,
    [string]$AssetsDir,
    [string]$Repo = 'acloudpeng/U60Pro-OpenUI',
    [switch]$All,
    [switch]$SkipScreenCheck,
    [switch]$Yes
)

$ErrorActionPreference = 'Stop'
$Chinese = $PSUICulture -like 'zh*'
$Files = 'zte-agent', 'dashboard-dist.tar.gz', 'openui-screen', 'lpac', 'mihomo'
# Python's output reaches the window line by line.
$env:PYTHONUNBUFFERED = '1'

function Say([string]$Zh, [string]$En) { if ($Chinese) { $Zh } else { $En } }

function Show-Step([string]$Zh, [string]$En) {
    Write-Host ''
    Write-Host ('== ' + (Say $Zh $En)) -ForegroundColor Cyan
}

function Show-Note([string]$Zh, [string]$En) { Write-Host (Say $Zh $En) }

function Show-Warning([string]$Zh, [string]$En) { Write-Host (Say $Zh $En) -ForegroundColor Yellow }

function Stop-Script([string]$Zh, [string]$En) {
    Write-Host ''
    Write-Host (Say $Zh $En) -ForegroundColor Red
    exit 1
}

function Confirm-Choice([string]$Zh, [string]$En, [bool]$Default) {
    if ($Yes) { return $Default }
    $suffix = if ($Default) { ' [Y/n]' } else { ' [y/N]' }
    $answer = (Read-Host ((Say $Zh $En) + $suffix)).Trim()
    if (-not $answer) { return $Default }
    return $answer -match '^(y|yes|是)$'
}

# The output of a command, or $null when it fails; its errors stay hidden.
function Get-Output([string]$Command, [string[]]$Arguments) {
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $output = & $Command @Arguments 2>$null
        if ($LASTEXITCODE -eq 0) { return (@($output) -join "`n").Trim() }
    } catch {
    } finally {
        $ErrorActionPreference = $saved
    }
    return $null
}

function Install-Package([string]$Id, [string]$Zh, [string]$En) {
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) { return $false }
    if (-not (Confirm-Choice $Zh $En $true)) { return $false }
    & winget install --exact --id $Id --accept-package-agreements --accept-source-agreements | Out-Host
    # The installer adds folders this session's PATH does not list yet.
    $env:Path = @([Environment]::GetEnvironmentVariable('Path', 'Machine'), [Environment]::GetEnvironmentVariable('Path', 'User')) -join ';'
    return $true
}

function Find-Python {
    foreach ($candidate in 'py -3', 'python', 'python3') {
        $parts = @($candidate -split ' ')
        if (-not (Get-Command $parts[0] -CommandType Application -ErrorAction SilentlyContinue)) { continue }
        # No double quotes in arguments: Windows PowerShell 5.1 passes them on broken.
        $probe = @($parts | Select-Object -Skip 1) + @('-c', "import sys; print('%d.%d' % sys.version_info[:2])")
        if ((Get-Output $parts[0] $probe) -match '^3\.(\d+)$' -and [int]($Matches[1]) -ge 10) { return ,$parts }
    }
    return $null
}

function Invoke-Python([string[]]$Arguments) {
    $prefix = @($Python | Select-Object -Skip 1)
    & $Python[0] @prefix @Arguments | Out-Host
}

# Every release file is present and matches sha256sums.txt.
function Test-Release([string]$Directory, [switch]$Report) {
    $sumsFile = Join-Path $Directory 'sha256sums.txt'
    if (-not (Test-Path -LiteralPath $sumsFile -PathType Leaf)) {
        if ($Report) { Show-Warning "缺少 sha256sums.txt：${Directory}" "sha256sums.txt is missing from ${Directory}" }
        return $false
    }
    $sums = @{}
    foreach ($line in Get-Content -LiteralPath $sumsFile) {
        if ($line -match '^([0-9a-fA-F]{64}) [ *](\S+)\s*$') { $sums[$Matches[2]] = $Matches[1].ToLowerInvariant() }
    }
    $complete = $true
    foreach ($name in $Files) {
        $path = Join-Path $Directory $name
        $actual = $null
        if (Test-Path -LiteralPath $path -PathType Leaf) { $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant() }
        if (-not $sums.ContainsKey($name) -or $actual -ne $sums[$name]) {
            if ($Report) { Show-Warning "缺少或校验不符：${name}" "Missing or not matching its checksum: ${name}" }
            $complete = $false
        }
    }
    return $complete
}

function Show-Hints {
    $known = [IO.Path]::Combine($HOME, '.ssh', 'known_hosts.d', 'zte')
    Show-Warning '常见原因：' 'Common causes:'
    Show-Note "- 电脑没有连着这台设备（Wi-Fi 或 USB 网络），或者设备地址不是 ${Gateway}（用 -Gateway 指定）。" "- This computer is not connected to the modem (Wi-Fi or USB network), or its address is not ${Gateway} (use -Gateway)."
    Show-Note '- 设备还没有装 OpenUI，或者没有开 SSH（2222 端口）。这个脚本只更新已有的安装。' '- The modem does not run OpenUI yet, or SSH (port 2222) is off. This script only updates an existing installation.'
    Show-Note '- Permission denied：私钥不对。用 -SshKey 指定安装 OpenUI 时用的那把私钥（不能设密码短语）。' '- Permission denied: wrong key. Pass the key used to install OpenUI with -SshKey (it cannot have a passphrase).'
    Show-Note "- Host key verification failed：设备换过或重刷过。确认是这台设备后，删掉 ${known} 里它的那一行。" "- Host key verification failed: the modem was replaced or reflashed. If it is this modem, delete its line from ${known}."
    Show-Note '- 触摸屏正在显示 OpenUI：在屏幕上回到原厂界面，再运行。' '- The touchscreen is showing OpenUI: return it to the stock screen, then run this again.'
}

$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Set-Location -LiteralPath $Root
if (-not (Test-Path -LiteralPath (Join-Path $Root 'scripts/deploy-components.py'))) {
    Stop-Script '请在完整的仓库目录里运行这个脚本。' 'Run this script from a complete checkout of the repository.'
}
$version = (Get-Content -Raw -LiteralPath (Join-Path $Root 'web-app/package.json') | ConvertFrom-Json).version
if ($version -match '^(\d+)\.(\d+)\.(\d+)$') {
    # Release tags drop a zero patch number, as scripts/check-release.py does.
    $Tag = if ($Matches[3] -eq '0') { "v$($Matches[1]).$($Matches[2])" } else { "v${version}" }
} else {
    Stop-Script "无法识别版本号：${version}" "Unrecognised version: ${version}"
}
Write-Host (Say "OpenUI 一键更新：${Tag}" "OpenUI update: ${Tag}") -ForegroundColor Cyan

Show-Step '检查工具' 'Checking tools'
$Python = Find-Python
if (-not $Python -and (Install-Package 'Python.Python.3.12' '没有找到 Python 3.10 或更新的版本。用 winget 安装 Python 3.12？' 'Python 3.10 or newer was not found. Install Python 3.12 with winget?')) {
    $Python = Find-Python
}
if (-not $Python) {
    Stop-Script '需要 Python 3.10 或更新的版本：从 https://www.python.org/downloads/ 安装（勾选 Add python.exe to PATH），再运行本脚本。' 'Python 3.10 or newer is required: install it from https://www.python.org/downloads/ (tick Add python.exe to PATH), then run this again.'
}
if (-not (Get-Command ssh -CommandType Application -ErrorAction SilentlyContinue)) {
    Stop-Script '没有找到 ssh：在「设置 → 应用 → 可选功能」里添加「OpenSSH 客户端」，再运行本脚本。' 'ssh was not found: add OpenSSH Client under Settings > Apps > Optional features, then run this again.'
}
Show-Note ('Python：' + ($Python -join ' ')) ('Python: ' + ($Python -join ' '))

Show-Step "准备发布包 ${Tag}" "Preparing release ${Tag}"
if ($AssetsDir) {
    $Release = (Resolve-Path -LiteralPath $AssetsDir).Path
    if (-not (Test-Release $Release -Report)) {
        Stop-Script "${Release} 里的文件不全，或者与 sha256sums.txt 不符。" "The files in ${Release} are incomplete or do not match sha256sums.txt."
    }
} else {
    $Release = [IO.Path]::Combine($Root, 'build', $Tag)
    if (-not (Test-Release $Release)) {
        Invoke-Python (@('scripts/download-release.py', '--repo', $Repo, '--tag', $Tag, '--output', $Release) + $Files)
        if ($LASTEXITCODE -ne 0) {
            Stop-Script "下载 ${Tag} 失败。确认能访问 GitHub、这个版本已经发布；也可以把发布页上的文件下载到同一个文件夹，用 -AssetsDir 指定。" "Downloading ${Tag} failed. Check that GitHub is reachable and the release is published, or download the files from the release page into one folder and pass it with -AssetsDir."
        }
        if (-not (Test-Release $Release -Report)) {
            Stop-Script '下载的文件与 sha256sums.txt 不符，已停止。' 'The downloaded files do not match sha256sums.txt; stopped.'
        }
    }
    $latest = Get-Output $Python[0] (@($Python | Select-Object -Skip 1) + @('scripts/download-release.py', '--repo', $Repo, '--latest'))
    if ($latest -match '^v(\d+\.\d+(\.\d+)?)$' -and ([version]($Matches[1])) -gt ([version]($Tag.Substring(1)))) {
        Show-Warning "已经发布了更新的版本 ${latest}。要部署它，先更新这个仓库（git pull），再运行本脚本。" "A newer release, ${latest}, is out. To deploy it, update this checkout (git pull) and run this again."
    }
}
Show-Note "文件已核对：${Release}" "Files checked: ${Release}"

Show-Step '准备地理数据' 'Preparing geo data'
$Geo = [IO.Path]::Combine($Root, 'build', 'mihomo', 'geo')
Invoke-Python @('scripts/prepare-geodata.py', '--output', $Geo)
if ($LASTEXITCODE -ne 0) {
    if (Test-Path -LiteralPath (Join-Path $Geo 'SHA256SUMS')) {
        Show-Warning '下载失败，改用上次下载的地理数据。' 'The download failed; using the geo data downloaded last time.'
    } elseif (Confirm-Choice '地理数据下载失败。不更新地理数据，继续？' 'The geo data download failed. Continue without updating it?' $false) {
        $Geo = $null
    } else {
        Stop-Script '已停止，设备没有改动。' 'Stopped; nothing on the modem changed.'
    }
}

Show-Step '连接设置' 'Connection'
$SettingsFile = [IO.Path]::Combine($Root, 'build', 'windows-deploy.json')
$saved = $null
if (Test-Path -LiteralPath $SettingsFile) {
    try { $saved = Get-Content -Raw -LiteralPath $SettingsFile | ConvertFrom-Json } catch { $saved = $null }
}
if (-not $Gateway) { $Gateway = if ($saved -and $saved.gateway) { [string]$saved.gateway } else { '192.168.0.1' } }
if (-not $SshKey -and $saved -and $saved.ssh_key -and (Test-Path -LiteralPath $saved.ssh_key -PathType Leaf)) { $SshKey = [string]$saved.ssh_key }
if (-not $SshKey) {
    $default = [IO.Path]::Combine($HOME, '.ssh', 'id_ed25519')
    if (Test-Path -LiteralPath $default -PathType Leaf) {
        $SshKey = $default
    } elseif (-not $Yes) {
        $SshKey = (Read-Host (Say '登录设备用的 SSH 私钥文件（安装 OpenUI 时用的那把）' 'SSH private key the modem accepts (the one used to install OpenUI)')).Trim().Trim('"')
    }
}
if (-not $SshKey -or -not (Test-Path -LiteralPath $SshKey -PathType Leaf)) {
    Stop-Script "找不到 SSH 私钥：${SshKey}。用 -SshKey 指定。" "SSH private key not found: ${SshKey}. Pass it with -SshKey."
}
$SshKey = (Resolve-Path -LiteralPath $SshKey).Path
Show-Note "设备：${Gateway}　私钥：${SshKey}" "Modem: ${Gateway}  Key: ${SshKey}"

$Deploy = @('scripts/deploy-components.py', '--gateway', $Gateway, '--ssh-key', $SshKey, '--keep-credentials',
    '--agent', (Join-Path $Release 'zte-agent'), '--dashboard', (Join-Path $Release 'dashboard-dist.tar.gz'),
    '--screen', (Join-Path $Release 'openui-screen'), '--lpac', (Join-Path $Release 'lpac'), '--mihomo', (Join-Path $Release 'mihomo'))
if ($Geo) { $Deploy += @('--mihomo-geodata', $Geo) }
if (-not $All) { $Deploy += '--skip-unchanged' }

Show-Step '试运行（只读取，不改动设备）' 'Dry run (reads only, changes nothing)'
Invoke-Python ($Deploy + '--dry-run')
if ($LASTEXITCODE -ne 0) {
    Show-Hints
    Stop-Script '试运行没有通过，设备没有任何改动。' 'The dry run did not pass; nothing on the modem changed.'
}
# Remembered once they have worked.
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $SettingsFile) | Out-Null
[pscustomobject]@{ gateway = $Gateway; ssh_key = $SshKey } | ConvertTo-Json | Set-Content -LiteralPath $SettingsFile -Encoding UTF8
if (-not (Confirm-Choice '试运行通过。现在部署到设备？' 'The dry run passed. Deploy to the modem now?' $true)) {
    Show-Note '已取消，设备没有改动。' 'Cancelled; nothing on the modem changed.'
    exit 0
}

Show-Step '部署（不要关闭这个窗口，也不要断开设备）' 'Deploying (keep this window open and the modem connected)'
Invoke-Python $Deploy
if ($LASTEXITCODE -ne 0) {
    Stop-Script "部署没有完成。部署工具会自动恢复原来的安装；如果上面说恢复没有完成，用 ssh -i `"${SshKey}`" -p 2222 root@${Gateway} 登录设备，执行它给出的恢复命令，并保存这个窗口的内容。" "The deployment did not finish. The deployment tool restores the previous installation itself; if it says the recovery did not finish, sign in with ssh -i `"${SshKey}`" -p 2222 root@${Gateway}, run the recovery command it printed, and keep the contents of this window."
}

if ($SkipScreenCheck) {
    Show-Warning '跳过了触摸屏检查：检查通过之前，网页上的触摸屏入口保持关闭。' 'Skipped the touchscreen check: the touchscreen entry on the web page stays off until it passes.'
} else {
    Show-Step '检查触摸屏程序' 'Checking the touchscreen program'
    Show-Note '如果屏幕程序更新了，这一步要 3 分钟左右：触摸屏会自己打开、关闭几次，期间不要操作设备。' 'If the screen program changed, this takes about 3 minutes: the touchscreen opens and closes by itself. Leave the modem alone meanwhile.'
    Invoke-Python @('scripts/verify-screen.py', '--gateway', $Gateway, '--ssh-key', $SshKey, '--if-needed')
    if ($LASTEXITCODE -ne 0) {
        Stop-Script '触摸屏检查没有通过：其他部分已经更新，网页上的触摸屏入口保持关闭。请保存这个窗口的内容。' 'The touchscreen check did not pass: everything else is updated, and the touchscreen entry on the web page stays off. Keep the contents of this window.'
    }
}

Show-Step '完成' 'Done'
Show-Note "已更新到 ${Tag}。网页：http://${Gateway}:8080" "Updated to ${Tag}. Web page: http://${Gateway}:8080"
exit 0
