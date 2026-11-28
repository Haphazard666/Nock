# T4b: "No network" from a network event, live streams in the Media card, install

Date: 2026-11-27/28. Branch `eng/T4b-network-live`, from `main` at 4575a0d (spec v1.5, checked). Windows 11 (build 26300), Rust release build.

**Summary: criteria 12a and 14 pass on the machine. The new build is installed as Yuval's daily Nock, with 0.13 backed up.** The full gate is green: build has 0 warnings, 75 tests pass, and clippy shows the same count as `main`. One product finding for the PM is below: after a long drop the player stays stuck, so the listener gets "Connection lost" and has to press Play.

## Changes

| File | What | Why |
|---|---|---|
| `src/tuner.rs` | While a `Player` exists, it registers WinRT `NetworkInformation.NetworkStatusChanged`. The handler runs on a WinRT worker thread, reads `GetInternetConnectionProfile().GetNetworkConnectivityLevel()` and only posts `WM_TUNER_NET` (**WM_APP + 15**, WPARAM = player gen, LPARAM = 1 up / 0 down). Unregistered in `Player::drop`. Up means internet or constrained internet. No profile, none or local-only means down (`level_up`, unit tested). New `Player::state()` and `Player::position()` (one property read each). | Criterion 12a. Network event, no polling, no thread of ours |
| `src/app.rs` | New `RStatus::NoNetwork`. Pure `radio_net()` handles network changes. `radio_step()` takes `net_down`: while it is set, the strip stays "No network" whatever the player claims, and no stall timer starts. A pause still shows, and a failure still drops the player. Network back: the strip shows the player's real state, and a one-shot 15 s timer **`T_RNET = 26`** starts. When it fires, if `Position` has not moved at least 1 s (`resumed()`), the player is dropped with "Connection lost". If the stream never opened, it gets its 10 s connect timer again instead. Timer handling is shared in `radio_apply()`. 6 new tests in `radio_tests`, `T_RNET` added to the timer-id test, and a `radio_no_network` preview render. | Criterion 12a |
| `src/ui.rs` | Strip: "No network" in amber with its own shape, a ring with a slash. It is not the warning triangle, which marks errors. Media card: no progress bar and no times when `is_live()`. | Criteria 12a and 14 |
| `src/media.rs` | `MediaState::is_live()`: the duration is missing or over 24 h. `progress()` gives duration 0 for a live source, so seeking is off too. Unit test `a_live_stream_has_no_duration`. | Criterion 14 |
| `src/win_main.rs` | Dispatches `WM_TUNER_NET`. Added to the window-message uniqueness test. | |
| `Cargo.toml` | `windows` feature `Networking_Connectivity`. This adds no crate. | WinRT network API |

**Existing listener?** I checked first. Nock listens to no network-change event. The top bar's Wi-Fi state (`radio.rs`) is the radio on/off switch, refreshed only when the worker gets a request, and `sys.rs` scans Wi-Fi only on demand. So there was nothing to reuse. The new listener lives only as long as a player, so with radio stopped Nock still listens to nothing (criterion 6).

## Gate

| Check | Result |
|---|---|
| `cargo build --release` | Pass, 0 warnings |
| `cargo test` | **75 passed**, 0 failed, 7 ignored (T4: 67) |
| `cargo clippy` | 25 warnings, the same count as `main`. None in new code |
| `preview::render_pages` (ignored test) | Pass. New renders `radio_no_network` and `media_live` checked by eye |

`cargo fmt --check` reports differences, as it already did before this task. Not part of the gate.

## Criterion 12a: live, real Wi-Fi drop

Test build with `NOCK_RADIO_TRACE` and a scratch copy of the profile. Glglz (MP3) playing. Yuval switched Wi-Fi off, then on again after about 48 s. The PC has no other uplink. Up during the test: Tailscale and the WSL vEthernet, neither with internet.

| Time (trace) | Event |
|---|---|
| 69.1 s | Windows: network down (player still reports `Playing`, position 42.7 s). Strip shows **"No network"** in the same millisecond. **Yuval saw it** |
| 69.1 to ~78.5 s | The buffer played out. Position froze at 52.1 s, so the audio went silent about 9 s after the cut |
| 117.0 s | Windows: network up. Strip shows "Playing" (the player's own state) and the 15 s check starts. A second "up" event 0.9 s later changed nothing |
| 132.0 s | Check: position 52.1 s, unchanged. The player is dropped and the strip shows **"Connection lost"**. The player never recovered by itself and kept claiming `Playing` |
| 133.3 s | Yuval pressed Play. A new player was opened and `Playing` 1.4 s later |

Result: **pass as written.** "No network" showed at once, the player was kept alive, the strip went back to the player's real state when the network returned, and "Connection lost" came 15 s later because the audio had not resumed. Not seen live: the branch where the audio resumes by itself within 15 s. The unit tests cover it (`network_back_shows_the_real_state_and_starts_the_resume_check`). This drop also confirmed that `Position` stops while the player is starved and still says `Playing`, so the check does not give a false "resumed".

### Finding for the PM

Yuval: "after wifi back on - I had to press play for it to start again." The behavior is what the spec says, but it is not a good experience. Here the player did not recover within 15 s. In T4 it took about 44 s, after a 110 s drop. A possible v1.6 rule: when the 15 s check fails, restart the same station once (a new player) instead of showing "Connection lost", and show "Connection lost" only if that restart fails. That costs one extra connect, only after a real drop. I did not build it, because it would be a spec change.

## Criterion 14: Media card

| Source | Media card | Who checked |
|---|---|---|
| Radio, KAN Gimel (installed test build) | Title, "Israel", "Nock". **No progress bar, no times** | Yuval (screenshot) |
| YouTube in Chrome (Avengers: Doomsday trailer) | Progress bar, **0:05** and **3:15** | Yuval (screenshot) |

Result: **pass.** Spotify was not tried. It reports a normal finite duration, which takes the same path as YouTube.

Note: spec v1.5 lists criteria only up to 13. "Criterion 14" is taken from the task list (T4b) and the v1.4 change note ("live streams show no duration in the Media card"). The PM may want to add it to the numbered list.

## Criteria 1 to 13

Unchanged code paths for 1 to 11 and 13. Their unit tests all pass (75 in total). Live on this build: play (criterion 3) worked from the strip after the drop and again on the installed build, and Stop silenced it (Yuval). Criterion 6: the network listener exists only while a player exists. It is removed in `Player::drop`, and its callbacks run on WinRT pool threads, not ours. CPU and memory for criteria 6 and 7 were not re-measured. The only additions while playing are one event registration and a timer after a network change.

## Install

- Quit the running 0.13. Backed up `%LOCALAPPDATA%\Programs\Nock\nock.exe` to **`nock.exe.bak-0.13`** (1,224,704 bytes, SHA-256 starts 95F312D24B4A).
- Copied the release build over `nock.exe` (1,314,816 bytes, SHA-256 starts 3EA18EC44486, matching `target\release\nock.exe`).
- Started Nock from that path with Yuval's own profile (normal `APPDATA`, no trace). `config.ini` is byte-identical to the copy taken before testing. The profile has no `stations.txt`, since test favorites stayed in the scratch profile.
- **Yuval confirmed on the installed Nock:** the Radio tab loads, a station plays, and Stop silences it.

### Rollback

In PowerShell:

```powershell
Stop-Process -Name nock -Force
Copy-Item "$env:LOCALAPPDATA\Programs\Nock\nock.exe.bak-0.13" "$env:LOCALAPPDATA\Programs\Nock\nock.exe" -Force
Start-Process "$env:LOCALAPPDATA\Programs\Nock\nock.exe"
```

`config.ini` needs no change. 0.13 ignores the `radio=` key. A `stations.txt` saved later can stay, because 0.13 does not read it.

## Untested

- Audio resuming by itself within 15 s of the network's return (unit tested only).
- Network loss while still connecting (unit tested only).
- Ethernet unplug, VPN-only, captive portal ("constrained" counts as up), and a network that switches without a gap.
- Criteria 6 and 7 re-measured with the listener in place.
- Linux `cargo test` (no Linux machine; all new code is in Windows-only modules).
