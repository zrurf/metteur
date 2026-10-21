#!/usr/bin/env bash
# Explicit local verification only. No install, paid provider, full suite or git publication.
set -euo pipefail
if [[ $# != 1 || "$1" != /* || -e "$1" ]]; then
  echo 'Usage: bash scripts/nx-m6-targeted-verify.sh /absolute/new/evidence-directory' >&2
  exit 2
fi
out="$1"
root="$(git rev-parse --show-toplevel)"
cd "$root"
if [[ -n "$(git status --porcelain)" ]]; then
  echo 'Preserve or commit existing changes before reproducing a frozen-source check.' >&2
  exit 2
fi
command -v cargo >/dev/null
command -v pnpm >/dev/null
mkdir -p "$out"
git rev-parse HEAD > "$out/source-head.txt"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
# Keep the Wasmtime cache in the chosen output if no user cache is configured.
export XDG_CACHE_HOME="${XDG_CACHE_HOME:-$out/cache}"
cargo test --locked -p metteur-daemon --lib nx_m6_bounded_duplicate_burst_and_repeated_recovery -- --nocapture > "$out/bounded-recovery.log" 2>&1
cargo test --locked -p metteur-daemon --test nx_storage additive_migration_cas_replay_restart_and_rollback -- --exact --nocapture > "$out/storage.log" 2>&1
cargo test --locked -p metteur-daemon --test recovery_admission unreadable_chat_checkpoint_stops_the_turn_before_tool_effects -- --exact --nocapture > "$out/recovery.log" 2>&1
cargo test --locked -p metteur-daemon --test nx_prompt_snapshots selected_snapshots_and_run_manifests_survive_source_edit_delete_and_restart -- --exact --nocapture > "$out/prompt-snapshots.log" 2>&1
cargo test --locked -p metteur-daemon --test nx_launch_live confirmed_saved_blueprint_runs_once_and_returns_actual_outputs_to_parent -- --exact --nocapture > "$out/launch.log" 2>&1
cargo build --locked -p metteur-daemon --bin metteurd -p metteur-web-server --bin metteur-web > "$out/linux-build.log" 2>&1
(
 cd modules/webcore
 pnpm exec vue-tsc --noEmit > "$out/types.log" 2>&1
 pnpm exec vite build --outDir "$out/web" > "$out/web-build.log" 2>&1
)
echo 'Targeted checks and builds passed. This is not a full matrix or actual-model evaluation.'
