# Nock Radio: tasks

Owner: PM. The Engineer does one task per prompt, in order, and stops.

| ID | Task | Pass | Status |
|---|---|---|---|
| T0 | Read-only machine check: Windows version, `git`, `rustup`/`cargo`/`rustc` (MSVC toolchain?), Visual Studio Build Tools C++ workload, PowerShell version. Install nothing. | `docs/results/T0.md` lists every version or "missing" | Next |
| T1 | Playback spike: play one hard-coded stream through WinRT `MediaPlayer` from a debug-only path, stop it, and check if it shows in Windows media controls and Nock's Media card. | Audio plays and stops; SMTC result reported; CPU/RAM while playing measured | |
| T2 | Directory client and favorites store in `src/tuner.rs` and `store.rs`, with unit tests (line parsing, store round trip, hostile input). No UI yet. | Spec criteria 8, 9 for this code | |
| T3 | Radio tab UI and wiring: Top, Search, Favorites, now-playing strip, `radio` config key. | Spec criteria 1 to 7, 9, 10 | |
| T4 | Polish and measurements, CHANGELOG and README entries, a clean upstream-ready PR. | All acceptance criteria pass on the machine | |
