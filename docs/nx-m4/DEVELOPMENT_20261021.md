# DeepSeek development checkpoint · 2026-10-21

The current owner authorized completion, paid tests within the existing total USD 5 cap, and synthetic acceptance data only. The existing CommandCode credential stays in its original local TOML file. Flash (`deepseek/deepseek-v4.1-flash`) and Pro (`deepseek/deepseek-v4-pro`) both returned real responses through the actual Windows production Web proxy and daemon.

Six development seeds now have concrete files, task inputs, objective assertions and canonical digests. Twelve held-out seeds were frozen before their first run. `scripts/nx_model_eval.py` uses the same daemon binary, workspace path, seed files, model, limits, tools and permissions for each old/new Prompt pair. Only the approved harness fragments differ at the isolated provider relay. The old seven Prompt texts remain unchanged in `baseline/`; the candidate adds its runtime contract. Pair order is counterbalanced.

The shared kernel-locked cost ledger reserves a byte-based conservative amount before a request, settles using actual reported input/output counts at the published peak rates, stops on unknown usage or insufficient cap, and never retries a generation automatically. Each run owns copies of the binaries, separate ports, synthetic workspaces and fresh session databases. Original credentials and user project databases are never copied into those workspaces.

The r2 run completed 24 cells on unchanged source: 20 passed; the four dev-05 cells stopped before any model call because the driver changed the executable graph without changing its native file JSON. The matching native source was corrected; the focused r3 run passed all four dev-05 cells on unchanged source. Thus the latest development evidence covers **24/24 passing cells** (6 tasks × 2 models × 2 Prompt variants), across those two explicitly recorded runs. The eight edit/test cells also have successful ExecuteCommand receipts with exit_code=0; their objective tests were independently run. A tool attempt alone does not count as a completed test.

Earlier failures remain preserved: incorrect decision-string casing in the first driver, a model-appended Windows echo command, and relinking a Windows executable while the initial evaluator still held it open. The driver now uses the actual AllowOnce/DenyOnce protocol, approves exact synthetic commands only, cancels its own RPC on errors and waits for workspace release. Runs now execute owned binary copies. Runtime contract nx-m4-2 was changed from the development command failure; it does not grant any extra tools or approval scope.

Evidence root: `D:/BaiduNetdiskDownload/metteur-a7-work/tmp/local-acceptance-20261020/`.

- First inventory pilot: `eval-development-first-dev01` (2 passing cells).
- Initial exploratory attempt: `eval-development-two-models-first` (incomplete; not a passing dataset run).
- Development r2: `eval-development-paired-r2`, wrapper `deepseek-development-paired-r2-mussl7_4`.
- Launch correction r3: `eval-development-launch-r3`, wrapper `deepseek-development-launch-r3-mnq__spa`.
- Native final-boundary checks: `final-boundaries-effective-config-0f920lk6` (2 passed).
- Native model/authority checks: `addon-model-boundaries-2a15o00o` (10 selected tests across crates passed).

No held-out result is claimed here. Model aliases are floating provider aliases; the gateway does not provide an immutable upstream build. Reported usage/cache fields are gateway observations, and peak-rate estimates remain distinct from an actual account bill. Small samples do not establish statistical significance, speed improvement or better upstream cache reuse.
