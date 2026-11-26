# Nock Radio: spec v1

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
- Default country: the Windows user region (for example `GetUserGeoID` / `GetUserDefaultGeoName`), falling back to no filter.

### How to fetch

- Reuse the `sys::powershell` pattern from `cal.rs` (`Invoke-WebRequest`), run on a short-lived thread that exits when done, then post one window message to the UI.
- Have PowerShell convert the JSON to one line per station (`uuid|name|countrycode|codec|bitrate|tags|url_resolved`) so Rust needs no JSON crate. Strip `|` and newlines from fields.
- **Security:** search text is user input. Never splice it raw into the PowerShell script. URL-encode it in Rust and pass it so a `'`, `;` or `$(...)` can't run code. Add a unit test with hostile input.
- Network only on user action: opening the tab (Top list), a search, or pressing play. Nothing in the background, nothing at start-up.

## Playback

- WinRT `Windows.Media.Playback.MediaPlayer` (add the needed `windows` crate features, for example `Media_Playback`, `Media_Core`). No new crates.
- Create the player on first play and drop it on stop. No player object exists while radio is stopped.
- React to `MediaFailed` and playback-state events by posting a window message (new `WM_APP + n`). No polling.
- Media session: if the player shows up in Windows media controls, set its display title to the station name and artist to the country. Nock's own Media card should then show it. If an unpackaged app can't do this, report it. Then the Radio tab's now-playing strip is the only display, and that is acceptable for v1.

## Storage

- `%APPDATA%\Nock\stations.txt`, one favorite per line: `name|url_resolved|uuid|countrycode|codec|bitrate`. Same style as `apps.txt`. Tolerate bad lines.
- `config.ini`: `radio=1` (tab shown, default) / `0` (tab hidden), with a comment line like the other keys.

## Code layout

- New module `src/tuner.rs` (the name `radio.rs` is taken by Wi-Fi/Bluetooth). Windows only, left out of Linux builds in `main.rs`.
- Drawing in `ui.rs`, state and actions in `app.rs`, store in `store.rs`, same as the other tabs.
- The Linux build must still pass `cargo test`.

## Acceptance criteria v1

1. The Radio tab shows a Top list for the user's country within about 3 s of first opening it.
2. A search for `jazz` (tag) and for `BBC` (name) returns results.
3. Clicking a station plays audio within about 5 s for a typical MP3 or AAC stream. Stop silences it right away.
4. A dead stream shows an error line and does not freeze or crash Nock.
5. Starring a station adds it to Favorites, and it is still there after restarting Nock.
6. With radio stopped, Nock's idle CPU stays at 0% and no `tuner` thread or player exists.
7. While playing, Nock's CPU and memory are measured and reported (target: under 2% CPU, under 40 MB extra).
8. Hostile search input (`'; Remove-Item x; '`, `$(calc)`) runs nothing.
9. `cargo build --release`, `cargo test`, `cargo clippy` pass. The preview test renders the Radio tab.
10. `radio=0` hides the tab.
