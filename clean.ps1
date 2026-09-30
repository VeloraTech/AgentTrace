[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'Medium')]
param()

$projectRoot = [System.IO.Path]::GetFullPath($PSScriptRoot)
$rootPrefix = $projectRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar

foreach ($relativePath in @('target', 'dist')) {
    $generatedPath = [System.IO.Path]::GetFullPath((Join-Path $projectRoot $relativePath))
    if (-not $generatedPath.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove a path outside the project directory: $generatedPath"
    }

    if (Test-Path -LiteralPath $generatedPath) {
        if ($PSCmdlet.ShouldProcess($generatedPath, 'Remove generated build output')) {
            Remove-Item -LiteralPath $generatedPath -Recurse -Force
            Write-Output "Removed $relativePath"
        }
    }
}
