# Nock Radio: tasks

Owner: PM. The Engineer does one task per prompt, in order, and stops.

| ID | Task | Pass | Status |
|---|---|---|---|
| T0 | Read-only machine check: Windows version, `git`, `rustup`/`cargo`/`rustc` (MSVC toolchain?), Visual Studio Build Tools C++ workload, PowerShell version. Install nothing. | `docs/results/T0.md` lists every version or "missing" | Next |
| T1 | Playback spike: play one hard-coded stream through WinRT `MediaPlayer` from a debug-only path, stop it, and check if it shows in Windows media controls and Nock's Media card. | Audio plays and stops; SMTC result reported; CPU/RAM while playing measured | Done (spike code kept on branch `eng/T1-playback-spike`, not merged) |
| T2 | Directory client and favorites store in `src/tuner.rs` and `store.rs`, with unit tests (line parsing, store round trip, hostile input). Also the clippy lint fix. No UI yet. | Spec criteria 8, 9 for this code | Done |
| T3 | Radio tab UI and wiring: Top, Search, Favorites, now-playing strip, `radio` config key, favorites `hls` field (spec v1.2). | Spec criteria 1 to 7, 9 to 11 | Done (criterion 7 redefined in v1.3) |
| T4 | Stall timeout, non-blocking Stop, tests T3 left open (AAC and HLS through the tab, pause from the flyout and the Media card, display scaling), criterion 7 re-measured, CHANGELOG and README entries. Live-stream duration in the Media card (v1.4). | All criteria 1 to 14 pass on the machine | Done except 12 (real drop) and 14 |
| T4b | Criterion 12a ("No network" from a network-change event) and criterion 14 (live stream shows no duration in the Media card), then install the build as Yuval's daily Nock with a backup. | Criteria 12 and 14 pass on the machine; full suite still green | Next |
| T5 | Upstream-ready PR to Akhilesh-Varute/Nock (clean history, no fork docs). Only if Yuval decides to submit. | | |
