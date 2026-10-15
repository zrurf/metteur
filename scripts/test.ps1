# Targeted Rust helper. Use scripts/verify.py for the complete acceptance gate.
# Run from an x64 MSVC developer terminal on Windows.
try {
    $cargoCommand = Get-Command cargo -ErrorAction Stop
} catch {
    Write-Error 'cargo is unavailable; install the documented Rust toolchain.'
    exit 127
}
& $cargoCommand.Source test --locked -j 1 --workspace @args
exit $LASTEXITCODE
