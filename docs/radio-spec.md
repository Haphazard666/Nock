# Nock Radio: spec v1.6

Changed in v1.6 (after T4b): restores criterion 14 (lost by mistake in v1.5), one automatic restart after a failed network recovery.

Changed in v1.5 (after T4): criterion 12 split into a local network drop (event driven, "No network") and a station drop (Buffering timer); a silent station with no event is a known v1 gap.

Changed in v1.4 (during T4): criterion 12 counts from when the audio stops, live streams show no duration in the Media card.

Changed in v1.3 (after T3): criterion 7 measures the radio's own cost, stall timeout after a stream drops, Stop must not block the UI, one search box for name and genre (as built).

Changed in v1.2 (after T2): favorites keep the HLS flag, first-open time loosened to 4 s with an instant loading state, country search hint, single-mirror risk noted.

Changed in v1.1 (after T1): title via MediaPlaybackItem, 10 s connect timeout, Buffering is not an error, HLS flag, criteria 6 and 7 reworded, clippy lint fix allowed.

Owner: PM. Engineer: read this, never edit it. Report problems instead.

## Goal

A Radio tab in the Nock panel that finds and plays internet radio stations from any country, like TuneIn, while keeping Nock tiny, native and idle at zero CPU.

## Scope v1

- New `Page::Radio` tab, placed right after `Page::Media`.
- Three lists in the tab: **Top** (most popular in the user's country), **Search** (by name, country or genre tag), **Favorites**.
- Each row: station name, country code, codec and bitrate, a star to favorite, click to play.
- A now-playing strip at the top of the tab: station name, play/stop, an error line when a stream fails.
- Favorites survive restarts.

Out of scope for v1: song titles from the stream (ICY metadata), recently played, station logos, sleep timer, Linux port, a separate volume control (system volume works).

## Station directory: Radio Browser

- Free, no key, community run: https://api.radio-browser.info/
- Pick a server from the server list (`https://all.api.radio-browser.info/json/servers`) at random. Do not hard-code one mirror. Try the next server on failure.
- Endpoints:
  - Top in country: `/json/stations/search?countrycode=XX&order=clickcount&reverse=true&hidebroken=true&limit=50`
  - Search: `/json/stations/search?name=...` or `&tag=...` or `&countrycode=...`, same order and limit
  - On play, call `/json/url/<stationuuid>` once (their click-count etiquette)
- Send a `User-Agent: Nock/<version>` header.
- Play `url_resolved`, not `url`.
- Search by country takes a two-letter code. The search box hint says so (for example "Name, genre, or country code like DE").
- Known risk: Radio Browser currently lists one mirror (de1). If it is down, every fetch fails and the tab shows "Station list unavailable, try again later". Accepted for v1.
- Default country: the Windows user region (for example `GetUserGeoID` / `GetUserDefaultGeoName`), falling back to no filter.

### How to fetch

- Reuse the `sys::powershell` pattern from `cal.rs` (`Invoke-WebRequest`), run on a short-lived thread that exits when done, then post one window message to the UI.
- Have PowerShell convert the JSON to one line per station so Rust needs no JSON crate, in this order: `uuid|name|countrycode|codec|bitrate|hls|tags|url_resolved`. Strip `|` and newlines from fields. Radio Browser can mislabel HLS as AAC and report bitrate 0: show the bitrate only when it is above 0, and show `HLS` when `hls` is 1.
- **Security:** search text is user input. Never splice it raw into the PowerShell script. URL-encode it in Rust and pass it so a `'`, `;` or `$(...)` can't run code. Add a unit test with hostile input.
- Network only on user action: opening the tab (Top list), a search, or pressing play. Nothing in the background, nothing at start-up.

## Playback

- WinRT `Windows.Media.Playback.MediaPlayer`. Add the `windows` features `Media`, `Media_Core`, `Media_Playback` to the product build. No new crates.
- Create the player on first play and drop it on stop. No player object exists while radio is stopped.
- Wrap the source in a `MediaPlaybackItem` and set its display properties (title = station name, artist = country) before playing. Do not use `DisplayUpdater` before `MediaOpened`: T1 showed it gets wiped.
- Event handlers run on worker threads. They only post a window message (new `WM_APP + n`) to the UI thread. No polling.
- **Connect timeout:** `MediaFailed` doesn't fire for a host that silently drops the connection. On Play, start a one-shot 10 s window timer. Kill it on `MediaOpened`. If it fires, stop the player and show "Station not responding". This is a one-shot timer, not polling.
- `MediaFailed` gives only a code. Nock writes its own short error text ("Station offline", "Format not supported", "Network error").
- **Stall timeout:** if the stream was playing and then sits in Buffering for 15 s, stop the player and show "Connection lost". One-shot timer started on entering Buffering after MediaOpened, killed on Playing.
- Stop must not block the UI thread. Release the player off the UI thread (for example on a short-lived thread that exits when done). Audio must still go silent at once.
- Buffering is not an error. Playback state flips between Buffering and Playing while audio keeps going. Show "Buffering..." in the strip, never an error.
- Media session: T1 confirmed that the stream shows in Windows media sessions and in Nock's Media card with no AUMID setup.

## Storage

- `%APPDATA%\Nock\stations.txt`, one favorite per line: `name|url_resolved|uuid|countrycode|codec|bitrate|hls`. The 7th field `hls` (0/1) is optional on load, so 6-field lines still load as hls=0. Same style as `apps.txt`. Tolerate bad lines.
- `config.ini`: `radio=1` (tab shown, default) / `0` (tab hidden), with a comment line like the other keys.

## Code layout

- New module `src/tuner.rs` (the name `radio.rs` is taken by Wi-Fi/Bluetooth). Windows only, left out of Linux builds in `main.rs`.
- Drawing in `ui.rs`, state and actions in `app.rs`, store in `store.rs`, same as the other tabs.
- The Linux build must still pass `cargo test`.

## Acceptance criteria v1

1. Opening the Radio tab shows a loading state at once, and the Top list for the user's country within about 4 s on the first open of a session, about 2 s after that.
2. A search for `jazz` (tag) and for `BBC` (name) returns results.
3. Clicking a station plays audio within about 5 s for a typical MP3 or AAC stream. Stop silences it right away.
4. A dead stream shows an error line and does not freeze or crash Nock.
5. Starring a station adds it to Favorites, and it is still there after restarting Nock.
6. With radio stopped: Nock's CPU is 0%, no player object exists, and no thread created by Nock code is alive. Threads and memory that Media Foundation keeps after a first play are accepted (T1: about 26 threads).
7. The radio's own cost while playing, measured inside Nock with `eq_animation=0`: under 2% of one core, and under 20 MB extra **private bytes** versus Nock before the first play. (Nock's existing equaliser animation runs for any media, Spotify included; its cost is not the radio's.)
8. Hostile search input (`'; Remove-Item x; '`, `$(calc)`) runs nothing.
9. `cargo build --release`, `cargo test`, `cargo clippy` pass. The preview test renders the Radio tab. (`cargo clippy` fails on upstream main at `src/games.rs:554`, `6.28` as approx_constant. The Engineer may fix that one line with `std::f32::consts::TAU` or similar, in its own commit.)
10. `radio=0` hides the tab.
11. A station that never answers shows "Station not responding" within about 10 s.
12. **Network drop.** (a) When Windows reports the PC has lost its network while radio plays, the strip shows "No network" (amber, with a shape) within about 5 s of that report. The player is kept alive. When the network returns and audio resumes on its own, the strip goes back to "Playing". If audio has not resumed 15 s after the network returns, restart the same station once automatically (strip: "Reconnecting..."). Only if that restart also fails, by the normal play rules (connect timeout, MediaFailed), show "Connection lost". Never more than one automatic restart per drop. Use a Windows network-change event (reuse one Nock already listens to if there is one), never polling. (b) **Station drop.** A stream that enters Buffering after it opened shows "Connection lost" 15 s later, or at once if the server hangs up (T4 proved this with a local relay). (c) Known v1 gap: a station that goes silent with the network fine and the player still reporting Playing is not detected.
13. Pressing Stop never freezes the panel (no visible hitch).
14. Nock's Media card shows no duration or progress for a live radio stream. Treat a duration over 24 h, or no duration, as live. Tracks with a real length still show their time.
