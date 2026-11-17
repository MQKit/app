# Saves a password-protected copy of a .docx through Word COM (test fixture helper).
# Usage: powershell -File word_protect.ps1 <source.docx> <target.docx>   (password in $env:OW_TEST_PASSWORD)
# Only the WINWORD.EXE started here is ever killed (identified by PID diff + start time).
param([Parameter(Mandatory = $true)][string]$Source, [Parameter(Mandatory = $true)][string]$Target)
$ErrorActionPreference = 'Stop'
$password = $env:OW_TEST_PASSWORD
if (-not $password) { throw 'OW_TEST_PASSWORD is not set' }

$before = @{}
Get-Process WINWORD -ErrorAction SilentlyContinue | ForEach-Object { $before[$_.Id] = $_.StartTime }
$word = New-Object -ComObject Word.Application
$mine = Get-Process WINWORD -ErrorAction SilentlyContinue | Where-Object { -not $before.ContainsKey($_.Id) -or $before[$_.Id] -ne $_.StartTime }
try {
    $word.Visible = $false
    $word.DisplayAlerts = 0
    $word.AutomationSecurity = 3
    $doc = $word.Documents.Open($Source, $false, $true, $false)
    if (Test-Path -LiteralPath $Target) { Remove-Item -LiteralPath $Target -Force }
    # SaveAs2(FileName, FileFormat=wdFormatDocumentDefault, LockComments, Password)
    $doc.SaveAs2($Target, 16, $false, $password)
    $doc.Close(0)
    [void][System.Runtime.InteropServices.Marshal]::FinalReleaseComObject($doc)
    Write-Output "protected copy saved"
}
finally {
    try { $word.Quit(0) } catch {}
    [void][System.Runtime.InteropServices.Marshal]::FinalReleaseComObject($word)
    [GC]::Collect(); [GC]::WaitForPendingFinalizers()
    foreach ($p in $mine) {
        if (-not $p.WaitForExit(5000)) {
            $cur = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
            if ($cur -and $cur.StartTime -eq $p.StartTime) { Stop-Process -Id $p.Id -Force }
        }
    }
}
