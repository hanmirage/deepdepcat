#!/usr/bin/env pwsh
<#
.SYNOPSIS
  DeepDepCat 一键发版：版本号 → 冒烟/回归 → 签名构建 → GitHub Release →
  云端更新服务 publish → 官网 changelog/manifest/安装包同步 → 官网快速部署。

.DESCRIPTION
  用法示例：
    .\scripts\release.ps1 -Version 1.1.9 -NotesFile docs\release-notes\1.1.9.md
    .\scripts\release.ps1 -Version 1.1.9 -SiteOnly        # 只同步官网（产物已存在）
    .\scripts\release.ps1 -Version 1.1.9 -DryRun          # 只打印将执行的动作

  配置（二选一，优先级：环境变量 > ~/.deepdepcat/release-config.json）：
    TAURI_SIGNING_PRIVATE_KEY / TAURI_SIGNING_PRIVATE_KEY_PASSWORD
    DDC_PUBLISH_KEY / GH_TOKEN
  配置文件字段见 scripts\release-config.example.json。
#>
param(
  [Parameter(Mandatory = $true)][string]$Version,
  [string]$NotesFile,
  [switch]$SkipVersion,
  [switch]$SkipChecks,
  [switch]$SkipBuild,
  [switch]$SkipGitHub,
  [switch]$SkipServer,
  [switch]$SkipSite,
  [switch]$SiteOnly,
  [switch]$DryRun
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$ConfigPath = Join-Path $env:USERPROFILE ".deepdepcat\release-config.json"

function Get-ReleaseConfig {
  $cfg = @{}
  if (Test-Path $ConfigPath) {
    $cfg = Get-Content $ConfigPath -Raw | ConvertFrom-Json -AsHashtable
  }
  return $cfg
}

function Get-CfgValue($cfg, $envName, $key) {
  $envVal = [Environment]::GetEnvironmentVariable($envName)
  if ($envVal) { return $envVal }
  if ($cfg) { return $cfg.$key }
  return $null
}

function Step($name) {
  Write-Host ""
  Write-Host "==== $name ====" -ForegroundColor Cyan
}

function Run-Check($title, $scriptBlock) {
  if ($DryRun) { Write-Host "[dry-run] $title"; return }
  Write-Host ">> $title"
  & $scriptBlock
  if ($LASTEXITCODE -ne 0) { throw "FAILED: $title" }
}

function Get-GitHubToken {
  if ($env:GH_TOKEN) { return $env:GH_TOKEN }
  $input = "protocol=https`nhost=github.com`n`n"
  $cred = $input | git credential fill 2>$null
  $tok = ($cred | Select-String "^password=").Line -replace "^password=", ""
  if (-not $tok) { throw "No GitHub token (set GH_TOKEN or run git credential fill)" }
  return $tok
}

function Parse-ReleaseNotes($path) {
  if (-not (Test-Path $path)) { throw "Release notes file not found: $path" }
  $title = ""; $summary = ""; $items = @()
  foreach ($line in Get-Content $path) {
    if ($line -match "^title:\s*(.+)$") { $title = $Matches[1].Trim() }
    elseif ($line -match "^summary:\s*(.+)$") { $summary = $Matches[1].Trim() }
    elseif ($line -match "^\s*-\s+(.+)$") { $items += $Matches[1].Trim() }
  }
  if (-not $title) { throw "Release notes must start with 'title: ...'" }
  if ($items.Count -eq 0) { throw "Release notes need at least one '- item' line" }
  if (-not $summary) { $summary = "v$Version - $title" }
  return @{ title = $title; summary = $summary; items = $items }
}

function Set-FileUtf8($path, $content) {
  [System.IO.File]::WriteAllText($path, $content, (New-Object System.Text.UTF8Encoding($false)))
}

function Update-VersionFiles {
  Step "1/7 版本号 $Version"
  $targets = @(
    # Cargo.toml: only the top-level [package] version (line-start), never
    # dependency "version = ..." lines inside the file.
    @{ p = "src-tauri\Cargo.toml";        re = '(?m)^version = "[^"]*"'; rep = "version = `"$Version`"" },
    @{ p = "src-tauri\tauri.conf.json";   re = '"version":\s*"[^"]*"';   rep = "`"version`": `"$Version`"" },
    @{ p = "package.json";                re = '"version":\s*"[^"]*"';   rep = "`"version`": `"$Version`"" }
  )
  if ($DryRun) {
    foreach ($t in $targets) { Write-Host "[dry-run] bump $($t.p)" }
    return
  }
  foreach ($t in $targets) {
    $p = Join-Path $RepoRoot $t.p
    $c = Get-Content $p -Raw
    if ($c -notmatch $t.re) { throw "Cannot find version marker in $($t.p)" }
    $c = [regex]::Replace($c, $t.re, $t.rep)
    Set-FileUtf8 $p $c
    Write-Host "  bumped $($t.p)"
  }
  # package-lock.json root entries (name=deepdepcat), not dependency versions.
  $lock = Join-Path $RepoRoot "package-lock.json"
  $lc = Get-Content $lock -Raw
  $pattern = '("name": "deepdepcat",\s*\n\s*"version": ")[^"]+(")'
  if ($lc -notmatch $pattern) { throw "package-lock.json root version marker not found" }
  $lc = [regex]::Replace($lc, $pattern, "`${1}$Version`${2}")
  Set-FileUtf8 $lock $lc
  Write-Host "  bumped package-lock.json"

  # Idempotent re-run: when the version files already carry $Version (e.g. a
  # previous run bumped + pushed, then a smoke failed), skip the empty commit
  # instead of aborting the pipeline.
  $versionFiles = @(
    "src-tauri\Cargo.toml",
    "src-tauri\tauri.conf.json",
    "package.json",
    "package-lock.json"
  )
  $dirty = git status --porcelain -- $versionFiles
  if (-not $dirty) {
    Write-Host "  version files already at $Version (skip add/commit/push)"
    return
  }
  Run-Check "git add version files" {
    git add src-tauri/Cargo.toml src-tauri/tauri.conf.json package.json package-lock.json
  }
  Run-Check "git commit" {
    git commit -m "release: bump version to $Version"
  }
  Run-Check "git push (proxy override)" {
    git -c http.version=HTTP/1.1 -c http.proxy= push origin main
  }
}

function Invoke-Checks {
  Step "2/7 冒烟与回归"
  Push-Location (Join-Path $RepoRoot "src-tauri")
  try {
    Run-Check "cargo test --lib" {
      $env:RUST_MIN_STACK = "33554432"; cargo test --lib
    }
    Run-Check "cargo clippy" {
      $env:RUST_MIN_STACK = "33554432"; cargo clippy -- -D warnings
    }
    Run-Check "browser smoke (visible+headless)" {
      $env:RUST_MIN_STACK = "33554432"; cargo test --lib -- --ignored browser::
    }
    Run-Check "WPS office-host smoke" {
      $env:RUST_MIN_STACK = "33554432"; cargo test --lib -- --ignored host_live_write_window_sync
    }
  } finally {
    Pop-Location
  }
}

function Invoke-Build {
  Step "3/7 签名构建"
  $cfg = Get-ReleaseConfig
  $keyPath = Get-CfgValue $cfg "TAURI_SIGNING_PRIVATE_KEY_PATH" "signingKeyPath"
  $keyPwd = Get-CfgValue $cfg "TAURI_SIGNING_PRIVATE_KEY_PASSWORD" "signingKeyPassword"
  if (-not $keyPath -or -not (Test-Path $keyPath)) { throw "Signing key path missing (config signingKeyPath or TAURI_SIGNING_PRIVATE_KEY_PATH)" }
  if (-not $keyPwd) { throw "Signing key password missing (config signingKeyPassword or env)" }
  if ($DryRun) { Write-Host "[dry-run] npm run tauri build + tauri signer sign"; return }

  $env:TAURI_SIGNING_PRIVATE_KEY = (Get-Content $keyPath -Raw).Trim()
  $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $keyPwd
  Push-Location $RepoRoot
  try {
    npm run tauri build
    if ($LASTEXITCODE -ne 0) {
      Write-Warning "tauri build exited $LASTEXITCODE — will sign artifacts if they exist"
    }
    $exe = Join-Path $RepoRoot "src-tauri\target\release\bundle\nsis\DeepDepCat_${Version}_x64-setup.exe"
    if (-not (Test-Path $exe)) { throw "Installer not produced: $exe" }
    if (Test-Path "$exe.sig") {
      Write-Host "  .sig already produced by tauri build — skipping explicit sign"
    } else {
      # The env key (set above) conflicts with -f; drop it for the explicit
      # fallback so signer takes the file path.
      Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY -ErrorAction SilentlyContinue
      npx tauri signer sign -f $keyPath $exe
      if ($LASTEXITCODE -ne 0) { throw "Signing failed" }
    }
    if (-not (Test-Path "$exe.sig")) { throw "Signature not produced: $exe.sig" }
  } finally {
    Pop-Location
  }
}

function Invoke-GitHubRelease($notes) {
  Step "4/7 GitHub Release (public repo)"
  $cfg = Get-ReleaseConfig
  $repo = Get-CfgValue $cfg "GH_REPO" "githubRepo"
  if (-not $repo) { $repo = "hanmirage/deepdepcat" }
  if ($DryRun) { Write-Host "[dry-run] gh release create v$Version -R $repo"; return }
  $env:GH_TOKEN = Get-GitHubToken
  $exe = Join-Path $RepoRoot "src-tauri\target\release\bundle\nsis\DeepDepCat_${Version}_x64-setup.exe"
  $body = "## v$Version`n`n" + ($notes.items | ForEach-Object { "- $_" }) -join "`n"
  Run-Check "gh release create" {
    gh release create "v$Version" -R $repo --title "v$Version" --notes $body $exe "$exe.sig"
  }
  Run-Check "gh release verify" {
    gh release view "v$Version" -R $repo --json tagName,assets
  }
}

function Invoke-ServerPublish($notes) {
  Step "5/7 云端更新服务 publish"
  $cfg = Get-ReleaseConfig
  $publishKey = Get-CfgValue $cfg "DDC_PUBLISH_KEY" "publishKey"
  if (-not $publishKey) { throw "Missing DDC_PUBLISH_KEY" }
  if ($DryRun) { Write-Host "[dry-run] POST /api/v1/updates/publish"; return }
  $exe = Join-Path $RepoRoot "src-tauri\target\release\bundle\nsis\DeepDepCat_${Version}_x64-setup.exe"
  $sig = [System.IO.File]::ReadAllText("$exe.sig").Trim()
  $sigEnc = [uri]::EscapeDataString($sig)
  $notesEnc = [uri]::EscapeDataString($notes.summary)
  $url = "https://deepdepcat.hsmiai.xyz/api/v1/updates/publish?version=$Version&platform=windows-x86_64&channel=stable&signature=$sigEnc&release_notes=$notesEnc&silent=false"
  $resp = curl.exe -s -X POST $url -H "X-Publish-Key: $publishKey" -F "file=@$exe"
  Write-Host $resp
  $respText = $resp -join "`n"
  if ($respText -notmatch '"status"\s*:\s*"published"') { throw "Server publish did not confirm published" }
}

function Sync-SiteFiles($notes) {
  Step "6/7 官网 changelog / manifest / 安装包同步"
  if ($DryRun) {
    Write-Host "[dry-run] insert changelog entry, rewrite manifest.json, copy exe+sig"
    return
  }
  $cfg = Get-ReleaseConfig
  $siteDir = Get-CfgValue $cfg "WEBSITE_DIR" "websiteDir"
  if (-not $siteDir -or -not (Test-Path $siteDir)) { throw "websiteDir not found: $siteDir" }
  $updatesDir = Join-Path $siteDir "public\updates"
  $downloadsDir = Join-Path $siteDir "public\downloads"
  $exe = Join-Path $RepoRoot "src-tauri\target\release\bundle\nsis\DeepDepCat_${Version}_x64-setup.exe"
  $sig = [System.IO.File]::ReadAllText("$exe.sig").Trim()

  # changelog: surgical insert at the top of "updates": [ ... ]
  $entry = [ordered]@{
    version = $Version
    date = (Get-Date -Format "yyyy-MM-dd")
    title = $notes.title
    tag = "release"
    items = $notes.items
  } | ConvertTo-Json -Depth 5 -Compress
  $changelogPath = Join-Path $updatesDir "changelog.json"
  $cl = Get-Content $changelogPath -Raw
  if ($cl -match "`"version`": `"$Version`"") {
    Write-Host "  changelog already contains $Version — skipping insert"
  } else {
    $marker = '"updates": ['
    $cl = $cl.Replace($marker, "$marker`n  $entry,")
    Set-FileUtf8 $changelogPath $cl
    Write-Host "  changelog entry inserted"
  }

  # manifest: full rewrite with fresh signature
  $manifest = [ordered]@{
    version = $Version
    pub_date = (Get-Date -AsUTC).ToString("yyyy-MM-ddTHH:mm:ssZ")
    notes = $notes.summary
    platforms = [ordered]@{
      "windows-x86_64" = [ordered]@{
        signature = $sig
        url = "/downloads/DeepDepCat_${Version}_x64-setup.exe"
      }
    }
  } | ConvertTo-Json -Depth 6 -Compress
  Set-FileUtf8 (Join-Path $updatesDir "manifest.json") $manifest
  Write-Host "  manifest.json rewritten"

  Copy-Item -LiteralPath $exe -Destination $downloadsDir -Force
  Copy-Item -LiteralPath "$exe.sig" -Destination $downloadsDir -Force
  Write-Host "  installer + sig copied"
}

function Invoke-SiteDeploy {
  Step "7/7 官网快速部署（仅 public/updates + downloads）"
  $cfg = Get-ReleaseConfig
  $siteDir = Get-CfgValue $cfg "WEBSITE_DIR" "websiteDir"
  $syncScript = Join-Path $siteDir "scripts\sync_public.py"
  if (-not (Test-Path $syncScript)) { throw "Missing website sync script: $syncScript" }
  if ($DryRun) { Write-Host "[dry-run] python $syncScript"; return }
  Run-Check "python sync_public.py" {
    python $syncScript
  }
}

function Invoke-Verify {
  Step "验证线上"
  if ($DryRun) { Write-Host "[dry-run] verify summary/changelog/download"; return }
  $s = curl.exe -s -L --max-time 30 "https://deepdepcat.hsmiai.xyz/api/updates/summary"
  if ($s -notmatch "\`"version\`":\s*\`"$Version\`"") { throw "summary does not show $Version" }
  Write-Host "  summary OK: $Version"
  $c = curl.exe -s -L --max-time 30 "https://deepdepcat.hsmiai.xyz/api/updates/changelog"
  if ($c -notmatch "\`"version\`":\s*\`"$Version\`"") { throw "changelog missing $Version" }
  Write-Host "  changelog OK: $Version"
  $d = curl.exe -s -L -r 0-0 -o NUL -w "%{http_code}" --max-time 30 "https://deepdepcat.hsmiai.xyz/downloads/DeepDepCat_${Version}_x64-setup.exe"
  if ($d -ne "206" -and $d -ne "200") { throw "download endpoint returned $d" }
  Write-Host "  download OK: $d"
}

# ── Main ────────────────────────────────────────────────────────────────
if ($Version -notmatch "^\d+\.\d+\.\d+$") { throw "Version must be like 1.1.9" }
if (-not $NotesFile) { $NotesFile = Join-Path $RepoRoot "docs\release-notes\$Version.md" }

if ($SiteOnly) {
  $notes = Parse-ReleaseNotes $NotesFile
  Sync-SiteFiles $notes
  Invoke-SiteDeploy
  Invoke-Verify
  Write-Host "`n=== Site-only sync complete: v$Version ===" -ForegroundColor Green
  exit 0
}

$notes = Parse-ReleaseNotes $NotesFile
$cfg = Get-ReleaseConfig
if (-not $SkipVersion) { Update-VersionFiles }
if (-not $SkipChecks) { Invoke-Checks }
if (-not $SkipBuild) { Invoke-Build }
if (-not $SkipGitHub) { Invoke-GitHubRelease $notes }
if (-not $SkipServer) { Invoke-ServerPublish $notes }
if (-not $SkipSite) {
  Sync-SiteFiles $notes
  Invoke-SiteDeploy
}
Invoke-Verify

Write-Host ""
Write-Host "=== Release v$Version complete ===" -ForegroundColor Green
Write-Host "GitHub : https://github.com/$((Get-CfgValue $cfg 'GH_REPO' 'githubRepo') ?? 'hanmirage/deepdepcat')/releases/tag/v$Version"
Write-Host "Website: https://deepdepcat.hsmiai.xyz/updates"
