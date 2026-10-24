# Workspace Reliability and Interaction Rework

The prior acceptance covered bounded contracts and missed core everyday editor workflows. Its completion claim does not establish reliability of the reported workflows. This rework starts from concrete regressions and changes ownership and event flow rather than adding retry loops to each view.

The owner requested all 15 issues be addressed and explicitly required fixes at the underlying layer. Work is isolated on `codex/workspace-reliability-20261021` from `fb3bb9f`. PR #8 was subsequently merged upstream; this rework requires a new draft; existing archives and evidence are immutable historical records.

| Issues | Root-level change | Required verification |
|---|---|---|
| 10, 12, 15 | Workspace-scoped document buffers own contents, disk baseline, deleted state and saving snapshots. Views and Monaco models attach to documents. Explicit close confirms unsaved changes. | Edit A, switch B/A and surfaces; split synchronization; save races; external modification; delete/restore; workspace isolation. |
| 11, 14 | File operations publish state only after successful RPC. One inline Explorer name editor handles focus, Enter/Escape and validation. Rename carries buffers. | Actual Windows file RPC plus UI create/delete/rename in root/nested folders, failures and cancellation. |
| 13, 15 | Atomic workspace registry open; watcher shutdown joins before lock release; selecting another workspace detaches the window without closing a shared daemon workspace. | Parallel open, competing process, crash, close/reopen, failed switch, two windows. |
| 5, 15 | Shared push invalidation stream after committed changes. Frontend readers serialize/coalesce refreshes. Host continuation dispatcher waits for changes or its nearest deadline. | No idle resource polling; update without focus/reload; reconnect/abort; pending-event and timer/job recovery. |
| 1–4, 6 | Six primary activity entries. Tasks owns Schedules, Addon Pages belongs to Settings, prompt files open in Explorer's editor with resource revision checks. One page spacing/title system, Title Case. | Primary navigation, legacy deep links, theme/width and keyboard accessibility, scoped prompt writes. |
| 7, 8, 9 | Assistant segments close at tool boundaries; readable context capacity display; valid default selection reacts to effective workspace config. | Ordered streaming and persisted transcript, unknown/zero/near-full context, delayed and changed model config. |

At investigation time, the old `http://127.0.0.1:1801` backend was no longer running. A new owned synthetic service on port 9179 passed actual create/read/delete/directory/Unicode nested-file operations. This establishes that the core file RPC works while connected; it does not excuse UI lifecycle and error-reporting defects.

Implementation and technical verification are complete. See [the full fresh audit](QUALITY_REAUDIT_20261021.md) and [45-task/88-scenario evidence](QUALITY_REAUDIT_20261021.json). Windows 908, Linux 921 and browser 138 tests passed. Owner visual signoff remains pending; existing warnings and historical paid evidence remain explicitly limited. Publication is recorded separately after the authorized fork pushes. The final actual service is on port 1801; port 9179 was an earlier intermediate candidate.
