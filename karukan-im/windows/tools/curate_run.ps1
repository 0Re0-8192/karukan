# Runs curate.py with a local model (llama-server, CPU only), only when the PC is idle.
# Scheduled by install_curate_task.ps1; safe to run by hand (-Force skips the idle checks).
param(
    [switch]$Force,
    [string]$Data = (Join-Path (Split-Path $PSScriptRoot -Parent) 'data'),
    [string]$Python = 'C:\Users\yuu\AppData\Local\Python\pythoncore-3.14-64\python.exe',
    [string]$Server = 'D:\Projects\.tools\llama.cpp-b11223-cuda12\extract\llama-server.exe',
    [string]$Model = 'D:\Projects\llm-experiments\model-comparison\models\gemma-4-26B-A4B-it-Q4_0.gguf',
    [int]$Port = 8081,
    [int]$Threads = 12,
    [int]$IdleMinutes = 10,
    [int]$MinFreeGB = 20,
    [int]$StartupSeconds = 240,
    [int]$RunSeconds = 900
)

$ErrorActionPreference = 'Stop'
$tmp = Join-Path $Data 'tmp'
New-Item -ItemType Directory -Force $tmp | Out-Null
$env:TEMP = $tmp
$env:TMP = $tmp
$env:PYTHONIOENCODING = 'utf-8'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$log = Join-Path $Data 'curate.log'

function Log($m) { Add-Content -Path $log -Value ("{0:yyyy-MM-dd HH:mm:ss} {1}" -f (Get-Date), $m) -Encoding utf8 }

Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class IdleTime {
    [StructLayout(LayoutKind.Sequential)] struct LASTINPUTINFO { public uint cbSize; public uint dwTime; }
    [DllImport("user32.dll")] static extern bool GetLastInputInfo(ref LASTINPUTINFO p);
    public static double Seconds() {
        LASTINPUTINFO i = new LASTINPUTINFO(); i.cbSize = (uint)Marshal.SizeOf(i);
        GetLastInputInfo(ref i);
        return (Environment.TickCount - (int)i.dwTime) / 1000.0;
    }
}
'@

$script = Join-Path $PSScriptRoot 'curate.py'

# nothing new? stop before loading any model
$probe = & $Python -I $script --data $Data --dry-run 2>&1 | Select-Object -First 1
if ("$probe" -eq 'nothing new') { exit 0 }

if (-not $Force) {
    $idle = [IdleTime]::Seconds()
    if ($idle -lt $IdleMinutes * 60) { Log "skip: user active ($([int]$idle)s idle)"; exit 0 }
}
# the model runs on the CPU (the GPU is left to whatever else is using it) and needs ~14 GB of RAM
$free = (Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory / 1MB
if ($free -lt $MinFreeGB) { Log ("skip: only {0:N1} GB RAM free" -f $free); exit 0 }

function Test-Server {
    try { (Invoke-WebRequest -UseBasicParsing -TimeoutSec 3 "http://127.0.0.1:$Port/health").StatusCode -eq 200 } catch { $false }
}

$started = $null
try {
    if (-not (Test-Server)) {
        Log 'starting model server'
        $started = Start-Process -FilePath $Server `
            -ArgumentList @('-m', "`"$Model`"", '-ngl', 0, '-c', 16384, '-t', $Threads, '--host', '127.0.0.1', '--port', $Port, '--no-webui') `
            -WindowStyle Hidden -PassThru `
            -RedirectStandardOutput (Join-Path $tmp 'server.out') -RedirectStandardError (Join-Path $tmp 'server.err')
        $t0 = Get-Date
        while (-not (Test-Server)) {
            if (((Get-Date) - $t0).TotalSeconds -gt $StartupSeconds) { throw 'model server did not come up' }
            if ($started.HasExited) { throw 'model server exited early' }
            Start-Sleep -Seconds 3
        }
        Log ("server up after {0:N0}s" -f ((Get-Date) - $t0).TotalSeconds)
    }
    $out = & $Python -I -X utf8 $script --data $Data --url "http://127.0.0.1:$Port" 2>&1
    Log ("curate: " + (($out | Out-String).Trim() -replace "`r?`n", ' | '))
}
catch {
    Log "error: $($_.Exception.Message)"
}
finally {
    if ($started) {
        Log 'stopping model server'
        & taskkill.exe /PID $started.Id /T /F | Out-Null
    }
}
