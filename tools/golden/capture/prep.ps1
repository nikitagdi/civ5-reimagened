<#
.SYNOPSIS
  Prepares (or restores) the user's Civ5 ini files for RenderDoc captures.

.DESCRIPTION
  Touches only the keys of the table below, in two files of
  Documents\My Games\Sid Meier's Civilization 5: config.ini and GraphicsSettingsDX11.ini.
  The game install folder is never read or written.

  First run copies both files to <cache>\captures\ini-backup\; an existing copy is never
  overwritten. -Restore copies them back byte for byte.

  Files are edited as bytes (Latin-1 round trip): line endings, BOM and every other line
  stay exactly as they were.

.PARAMETER Scene
  S1, S2, S3 or S4 (S4 uses the S1 keys; S2-bake and S2 are one preparation, S2).
.PARAMETER Restore
  Put the original files back from the backup.
.PARAMETER MyDocs
  Documents folder (contains "My Games"). Default: $env:CIV5_MYDOCS, else the user's Documents.
.PARAMETER CacheDir
  the cache folder. Default: $env:CIV5_CACHE, else `cache` next to the repository.
#>
[CmdletBinding()]
param(
    [ValidateSet('S1', 'S2', 'S3', 'S4')][string]$Scene,
    [switch]$Restore,
    [string]$MyDocs,
    [string]$CacheDir
)

$ErrorActionPreference = 'Stop'

$GfxFile = 'GraphicsSettingsDX11.ini'
$CfgFile = 'config.ini'
$Latin1 = [Text.Encoding]::GetEncoding(28591)

function Resolve-MyDocs {
    if ($MyDocs) { return $MyDocs }
    if ($env:CIV5_MYDOCS) { return $env:CIV5_MYDOCS }
    return [Environment]::GetFolderPath('MyDocuments')
}

function Resolve-CacheDir {
    if ($CacheDir) { return $CacheDir }
    if ($env:CIV5_CACHE) { return $env:CIV5_CACHE }
    # tools/golden/capture -> the repository root -> its parent: `cache` next to the repository.
    $repo = Split-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) -Parent
    return (Join-Path (Split-Path $repo -Parent) 'cache')
}

# Reads a file as text with a 1:1 byte mapping.
function Read-Raw([string]$path) { return $Latin1.GetString([IO.File]::ReadAllBytes($path)) }

# Splits text into lines that keep their end-of-line (the last one may have none).
function Split-Lines([string]$text) {
    $parts = [regex]::Split($text, '(?<=\n)')
    if ($parts.Count -gt 0 -and $parts[$parts.Count - 1] -eq '') {
        return , @($parts[0..($parts.Count - 2)])
    }
    return , @($parts)
}

function Get-Body([string]$line) { return $line.TrimEnd("`r", "`n") }

# Value of a key in a section, or $null.
function Get-IniValue([string]$text, [string]$section, [string]$key) {
    $cur = $null
    foreach ($l in (Split-Lines $text)) {
        $b = Get-Body $l
        if ($b -match '^\s*\[(.+?)\]\s*$') { $cur = $Matches[1].Trim(); continue }
        if ($cur -ieq $section -and $b -match ('^\s*' + [regex]::Escape($key) + '\s*=\s*(.*?)\s*$')) {
            return $Matches[1]
        }
    }
    return $null
}

# Sets one key; returns the new text and fills $report with a line describing the change.
function Set-IniValue([string]$text, [string]$section, [string]$key, [string]$value, [ref]$report) {
    $lines = [Collections.Generic.List[string]](Split-Lines $text)
    $eol = "`r`n"
    foreach ($l in $lines) {
        if ($l.EndsWith("`r`n")) { $eol = "`r`n"; break }
        if ($l.EndsWith("`n")) { $eol = "`n"; break }
    }
    $secStart = -1
    $secEnd = $lines.Count   # exclusive
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $b = Get-Body $lines[$i]
        if ($b -match '^\s*\[(.+?)\]\s*$') {
            if ($secStart -ge 0) { $secEnd = $i; break }
            if ($Matches[1].Trim() -ieq $section) { $secStart = $i }
        }
    }
    $rx = '^(\s*' + [regex]::Escape($key) + '\s*=\s*)(.*?)(\s*)$'
    if ($secStart -ge 0) {
        for ($i = $secStart + 1; $i -lt $secEnd; $i++) {
            $b = Get-Body $lines[$i]
            $m = [regex]::Match($b, $rx, 'IgnoreCase')
            if ($m.Success) {
                $old = $m.Groups[2].Value
                if ($old -ceq $value) {
                    $report.Value = "[$section] $key = $value (unchanged)"
                } else {
                    $tail = $lines[$i].Substring($b.Length)
                    $lines[$i] = $m.Groups[1].Value + $value + $m.Groups[3].Value + $tail
                    $report.Value = "[$section] $key : $old -> $value"
                }
                return ($lines -join '')
            }
        }
        # Key missing: insert after the last non-blank line of the section.
        $at = $secEnd
        while ($at -gt $secStart + 1 -and (Get-Body $lines[$at - 1]).Trim() -eq '') { $at-- }
        if (-not ($lines[$at - 1].EndsWith("`n"))) { $lines[$at - 1] += $eol }
        $lines.Insert($at, "$key = $value$eol")
        $report.Value = "[$section] $key : (absent) -> $value"
        return ($lines -join '')
    }
    # Section missing: append it.
    if ($lines.Count -gt 0 -and -not ($lines[$lines.Count - 1].EndsWith("`n"))) { $lines[$lines.Count - 1] += $eol }
    $lines.Add("[$section]$eol")
    $lines.Add("$key = $value$eol")
    $report.Value = "[$section] $key : (absent, section added) -> $value"
    return ($lines -join '')
}

# The table: file, section, key, value. $null value = "as it was in the backup".
function Get-KeyTable([string]$scene) {
    $t = @(
        , @($GfxFile, 'UserSettings', 'FullScreen', '0')
        , @($GfxFile, 'UserSettings', 'WindowResX', '1920')
        , @($GfxFile, 'UserSettings', 'WindowResY', '1080')
        , @($GfxFile, 'UserSettings', 'MSAASamples', '4')
        , @($CfgFile, 'Debug', 'ThreadingMode', '1')
        , @($CfgFile, 'User Settings', 'Threaded Submission', '0')
        , @($CfgFile, 'Debug', 'LoggingEnabled', '1')
    )
    $seed = if ($scene -eq 'S3') { $null } else { '12345' }
    $fow = if ($scene -eq 'S2') { '1' } else { '0' }
    $t += , @($CfgFile, 'CONFIG', 'SyncRandSeed', $seed)
    $t += , @($CfgFile, 'CONFIG', 'MapRandSeed', $seed)
    $t += , @($CfgFile, 'DisableSystems', 'FOW', $fow)
    $t += , @($CfgFile, 'DisableSystems', 'FluidFOW', $fow)
    return , $t
}

function Get-Sha256([string]$path) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($sha.ComputeHash([IO.File]::ReadAllBytes($path))) } finally { $sha.Dispose() }
}

try {
    if ($Restore -eq [bool]$Scene) { throw 'give exactly one of -Scene S1|S2|S3|S4 or -Restore' }

    $docs = Resolve-MyDocs
    $iniDir = Join-Path $docs "My Games\Sid Meier's Civilization 5"
    if ($iniDir -match 'steamapps') { throw "refusing to work in a Steam library path: $iniDir" }
    $cache = Resolve-CacheDir
    $backupDir = Join-Path $cache 'captures\ini-backup'
    $files = @($CfgFile, $GfxFile)

    foreach ($f in $files) {
        if (-not (Test-Path -LiteralPath (Join-Path $iniDir $f))) { throw "not found: $(Join-Path $iniDir $f)" }
    }

    if ($Restore) {
        foreach ($f in $files) {
            $bak = Join-Path $backupDir $f
            if (-not (Test-Path -LiteralPath $bak)) { throw "no backup: $bak" }
        }
        foreach ($f in $files) {
            $dst = Join-Path $iniDir $f
            Copy-Item -LiteralPath (Join-Path $backupDir $f) -Destination $dst -Force
            if ((Get-Sha256 $dst) -ne (Get-Sha256 (Join-Path $backupDir $f))) { throw "restore check failed: $f" }
            Write-Output "restored $dst"
        }
        Write-Output 'done: your original settings are back.'
        exit 0
    }

    New-Item -ItemType Directory -Force -Path $backupDir | Out-Null
    foreach ($s in 'S1', 'S2', 'S3', 'S4') {
        New-Item -ItemType Directory -Force -Path (Join-Path $cache "captures\$s") | Out-Null
    }
    foreach ($f in $files) {
        $bak = Join-Path $backupDir $f
        if (Test-Path -LiteralPath $bak) {
            Write-Output "backup kept (not overwritten): $bak"
        } else {
            Copy-Item -LiteralPath (Join-Path $iniDir $f) -Destination $bak
            Write-Output "backup created: $bak"
        }
    }

    $table = Get-KeyTable $Scene
    foreach ($f in $files) {
        $path = Join-Path $iniDir $f
        $text = Read-Raw $path
        $bakText = Read-Raw (Join-Path $backupDir $f)
        $changed = @()
        foreach ($row in $table) {
            if ($row[0] -ne $f) { continue }
            $val = $row[3]
            if ($null -eq $val) {
                $val = Get-IniValue $bakText $row[1] $row[2]
                if ($null -eq $val) { $changed += "[$($row[1])] $($row[2]) : not in the backup, left alone"; continue }
            }
            $rep = [ref]''
            $text = Set-IniValue $text $row[1] $row[2] $val $rep
            $changed += $rep.Value
        }
        [IO.File]::WriteAllBytes($path, $Latin1.GetBytes($text))
        Write-Output "$f :"
        $changed | ForEach-Object { Write-Output "  $_" }
    }
    Write-Output "done: settings for $Scene are set. Put yours back later with -Restore."
    exit 0
} catch {
    [Console]::Error.WriteLine("prep.ps1: $($_.Exception.Message)")
    exit 1
}
