# Launch the portable debug build the way a double-click does: the working
# directory MUST be the exe's own directory, otherwise the smoke test inherits
# the caller's cwd and passes even when real double-click launches would fail.
# UTF-8 with BOM is required for Windows PowerShell 5.1.
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = 'D:\LapPortable\Lap.exe'
$psi.WorkingDirectory = 'D:\LapPortable'
$psi.UseShellExecute = $true
$p = [System.Diagnostics.Process]::Start($psi)
Start-Sleep -Seconds 12
$proc = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
if ($null -ne $proc) {
    Write-Output ("running pid=" + $proc.Id + " title=[" + $proc.MainWindowTitle + "]")
} else {
    try { Write-Output ("EXITED exitCode=" + $p.ExitCode) } catch { Write-Output 'NOT RUNNING' }
}
