# Launch the portable debug build and verify it comes up.
# UTF-8 with BOM is required for Windows PowerShell 5.1.
$p = Start-Process -FilePath 'D:\LapPortable\Lap.exe' -PassThru
Start-Sleep -Seconds 12
$proc = Get-Process -Name Lap -ErrorAction SilentlyContinue
if ($null -ne $proc) {
    Write-Output ("running pid=" + ($proc.Id -join ','))
} else {
    Write-Output 'NOT RUNNING'
}
