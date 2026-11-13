# Close the portable test instance cleanly.
$p = Get-Process -Name Lap -ErrorAction SilentlyContinue
if ($null -ne $p) {
    $p | Stop-Process -Force
    Start-Sleep -Seconds 2
    if ($null -ne (Get-Process -Name Lap -ErrorAction SilentlyContinue)) {
        Write-Output 'still running'
    } else {
        Write-Output 'closed'
    }
} else {
    Write-Output 'not running'
}
