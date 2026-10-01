```powershell
$name = "World"
function Get-Greeting {
    param([string]$Who = "you")
    Write-Host "Hello, $Who" -ForegroundColor Green
}
Get-ChildItem -Path . | Where-Object { $_.Length -gt 1kb } | ForEach-Object { $_.Name }
if ($true -and $name -eq "World") { exit 0 }
```
