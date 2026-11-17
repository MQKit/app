# Records the MQKit window until <stopFile> exists. Usage: record.ps1 <output.mkv> <stopFile> [fps]
param([string]$Output, [string]$StopFile, [int]$Fps = 30)
Add-Type -Path "$PSScriptRoot\Recorder.cs" -ReferencedAssemblies System.Drawing
$p = Get-Process mqkit | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $p) { 'no window'; exit 1 }
$ffmpeg = (Get-Command ffmpeg).Source
[Recorder]::Run($p.MainWindowHandle, $ffmpeg, $Output, $StopFile, $Fps)
