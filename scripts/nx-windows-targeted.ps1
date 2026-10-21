param([Parameter(Mandatory=$true)][string]$OutputDirectory)
$ErrorActionPreference = 'Stop'
if (-not [System.IO.Path]::IsPathRooted($OutputDirectory) -or (Test-Path $OutputDirectory)) { throw 'Choose an absolute NEW output directory.' }
if ($env:OS -ne 'Windows_NT') { throw 'This script records native Windows evidence only.' }
$root = (& git rev-parse --show-toplevel).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Run inside the Metteur repository.' }
Set-Location $root
if (& git status --porcelain) { throw 'Preserve existing changes before checking a frozen candidate.' }
New-Item -ItemType Directory $OutputDirectory | Out-Null
& git rev-parse HEAD | Out-File "$OutputDirectory/source-head.txt"
$PSVersionTable | Out-File "$OutputDirectory/platform.txt"
$env:CARGO_BUILD_JOBS = '1'
$cases = @(
 @('lock','--lib','workspace::lock::tests'),
 @('storage','--test','nx_storage','additive_migration_cas_replay_restart_and_rollback'),
 @('recovery','--test','recovery_admission','unreadable_chat_checkpoint_stops_the_turn_before_tool_effects')
)
foreach ($case in $cases) {
 $name=$case[0]; $arguments=$case[1..($case.Length-1)]
 # Windows PowerShell 5 treats redirected native stderr as ErrorRecords under
 # ErrorActionPreference=Stop, even for successful Cargo progress messages.
 # Capture both native streams directly and decide only from the process code.
 $cargo = (Get-Command cargo -CommandType Application -ErrorAction Stop).Source
 $stdout = Join-Path $OutputDirectory "$name.stdout.log"
 $stderr = Join-Path $OutputDirectory "$name.stderr.log"
 $process = Start-Process -FilePath $cargo -ArgumentList (@('test','--locked','-p','metteur-daemon') + $arguments + @('--','--nocapture')) -WorkingDirectory $root -RedirectStandardOutput $stdout -RedirectStandardError $stderr -WindowStyle Hidden -PassThru
 # Retain the process handle before exit; wait for Cargo itself rather than
 # persistent compiler helpers in its process tree. Preserve the real exit code.
 $handle = $process.Handle
 $process.WaitForExit()
 $exitCode = $process.ExitCode
 @("Exit code: $exitCode", '--- stdout ---', (Get-Content -Raw -LiteralPath $stdout), '--- stderr ---', (Get-Content -Raw -LiteralPath $stderr)) | Set-Content -LiteralPath (Join-Path $OutputDirectory "$name.log") -Encoding UTF8
 if ($null -eq $exitCode -or $exitCode -ne 0) { throw "$name failed with exit code $exitCode; preserve logs and stop." }
}
'Native Windows targeted cases passed; not a full platform suite.' | Out-File "$OutputDirectory/RESULT.txt"
