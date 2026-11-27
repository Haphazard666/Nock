//! Nock Radio: the station directory client (Radio Browser, https://api.radio-browser.info/).
//!
//! Windows only. Network happens only when the UI asks (open the tab, search, play), on a
//! short-lived thread that runs one PowerShell script, stores the result and posts one
//! `WM_TUNER` to the window. Nothing runs in the background and nothing at start-up.
//!
//! PowerShell turns the JSON into one line per station so Rust needs no JSON crate:
//! `uuid|name|countrycode|codec|bitrate|hls|tags|url_resolved`.
//!
//! Playback (`Player`) uses WinRT `MediaPlayer`. Its event handlers run on WinRT worker
//! threads and only post `WM_TUNER_PLAY`; every state change happens on the UI thread.
//!
//! Security: search text is user input. It is URL-encoded here, and only the encoded form
//! (letters, digits and `-_.~%`) ever reaches the script. Server names and station uuids
//! from the network are checked against a strict character set before they are spliced in.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::store::Favorite;

pub const WM_TUNER: u32 = 0x8000 + 13; // WM_APP + 13: a directory fetch finished (WPARAM = its id)
/// WM_APP + 14: a player event (WPARAM = player generation, LPARAM = `encode_event`).
pub const WM_TUNER_PLAY: u32 = 0x8000 + 14;

const SERVER_LIST: &str = "https://all.api.radio-browser.info/json/servers";
/// How many mirrors one fetch tries before giving up.
const MAX_TRIES: usize = 4;
/// Stations per list.
const LIMIT: u32 = 50;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Station {
    pub uuid: String,
    pub name: String,
    pub countrycode: String,
    pub codec: String,
    /// kbps; 0 = unknown (Radio Browser reports 0 for many streams).
    pub bitrate: u32,
    pub hls: bool,
    pub tags: String,
    /// `url_resolved`: the URL to play.
    pub url: String,
}

impl Station {
    /// Codec column: `HLS` when the stream is HLS (Radio Browser often labels those AAC).
    pub fn codec_label(&self) -> String {
        if self.hls {
            "HLS".into()
        } else {
            self.codec.clone()
        }
    }

    /// Bitrate column: only shown when it is known.
    pub fn bitrate_label(&self) -> Option<String> {
        (self.bitrate > 0).then(|| format!("{} kbps", self.bitrate))
    }

    pub fn to_favorite(&self) -> Favorite {
        Favorite {
            name: self.name.clone(),
            url: self.url.clone(),
            uuid: self.uuid.clone(),
            countrycode: self.countrycode.clone(),
            codec: self.codec.clone(),
            bitrate: self.bitrate,
            hls: self.hls,
        }
    }

    /// Tags are not stored with a favorite, so they come back empty.
    pub fn from_favorite(f: &Favorite) -> Station {
        Station {
            uuid: f.uuid.clone(),
            name: f.name.clone(),
            countrycode: f.countrycode.clone(),
            codec: f.codec.clone(),
            bitrate: f.bitrate,
            hls: f.hls,
            tags: String::new(),
            url: f.url.clone(),
        }
    }
}

/// Favorites from stations.txt, as stations.
pub fn load_favorites() -> Vec<Station> {
    crate::store::load_favorites().iter().map(Station::from_favorite).collect()
}

pub fn save_favorites(stations: &[Station]) {
    let favs: Vec<Favorite> = stations.iter().map(Station::to_favorite).collect();
    crate::store::save_favorites(&favs);
}

/// One station line: `uuid|name|countrycode|codec|bitrate|hls|tags|url_resolved`.
/// Lines with the wrong number of fields, no uuid, no name or no http(s) URL are skipped.
pub fn parse_line(line: &str) -> Option<Station> {
    let f: Vec<&str> = line.trim_end_matches(['\r', '\n']).split('|').map(str::trim).collect();
    if f.len() != 8 {
        return None;
    }
    let url = f[7];
    if f[0].is_empty() || f[1].is_empty() || !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    Some(Station {
        uuid: f[0].into(),
        name: f[1].into(),
        countrycode: f[2].to_ascii_uppercase(),
        codec: f[3].into(),
        bitrate: f[4].parse().unwrap_or(0),
        hls: f[5] == "1" || f[5].eq_ignore_ascii_case("true"),
        tags: f[6].into(),
        url: url.into(),
    })
}

// ----------------------------------------------------------------- queries

#[derive(Clone, Debug, PartialEq)]
pub enum Query {
    /// Most popular stations, in a country (ISO 3166 two-letter code) or worldwide.
    Top(Option<String>),
    Name(String),
    Tag(String),
    Country(String),
    /// The search box: stations whose name matches, or that carry the tag (genre).
    Any(String),
}

/// What the search box asks for: a two-letter code is a country, anything else a name or genre.
pub fn query_for(text: &str) -> Option<Query> {
    let t = text.trim();
    if t.is_empty() {
        None
    } else if let Some(cc) = country_code(t) {
        Some(Query::Country(cc))
    } else {
        Some(Query::Any(t.to_string()))
    }
}

/// Percent-encode everything except the RFC 3986 unreserved characters.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A two-letter country code, upper-cased, or None.
fn country_code(s: &str) -> Option<String> {
    let s = s.trim();
    (s.len() == 2 && s.bytes().all(|b| b.is_ascii_alphabetic())).then(|| s.to_ascii_uppercase())
}

/// The API path for a query, e.g. `/json/stations/search?tag=jazz&order=clickcount&...`.
pub fn query_path(q: &Query) -> String {
    let filter = match q {
        Query::Top(Some(cc)) => match country_code(cc) {
            Some(cc) => format!("countrycode={cc}&"),
            None => String::new(),
        },
        Query::Top(None) => String::new(),
        Query::Name(s) => format!("name={}&", url_encode(s.trim())),
        Query::Tag(s) => format!("tag={}&", url_encode(s.trim())),
        Query::Country(s) => format!("countrycode={}&", url_encode(&s.trim().to_ascii_uppercase())),
        // `Any` is two requests (see `query_paths`); this is the name one
        Query::Any(s) => format!("name={}&", url_encode(s.trim())),
    };
    format!("/json/stations/search?{filter}order=clickcount&reverse=true&hidebroken=true&limit={LIMIT}")
}

/// Every request a query needs: one, or for `Any` the name search and the tag search.
pub fn query_paths(q: &Query) -> Vec<String> {
    match q {
        Query::Any(s) => vec![query_path(&Query::Name(s.clone())), query_path(&Query::Tag(s.trim().to_lowercase()))],
        _ => vec![query_path(q)],
    }
}

/// Characters a path may contain before it is spliced into the script.
fn safe_path(p: &str) -> bool {
    !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.~%&=?/".contains(&b))
}

/// A Radio Browser mirror name such as `de1.api.radio-browser.info`.
fn safe_host(h: &str) -> bool {
    !h.is_empty() && h.len() < 100 && h.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
}

/// A station uuid: hex digits and dashes.
fn safe_uuid(u: &str) -> bool {
    !u.is_empty() && u.len() <= 64 && u.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

const PROLOGUE: &str = r#"$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.Encoding]::UTF8
[Net.ServicePointManager]::SecurityProtocol=[Net.SecurityProtocolType]::Tls12
function Get-Utf8($u){ $r=Invoke-WebRequest -UseBasicParsing -Uri $u -Headers @{'User-Agent'=$ua} -TimeoutSec 6; [Text.Encoding]::UTF8.GetString($r.RawContentStream.ToArray()) }
function C($v){ ("$v" -replace '[|\r\n\t]',' ').Trim() }
"#;

/// `$servers=@(...)`: the mirrors to try, in order. Empty = fetch the server list first.
fn servers_block(servers: &[String]) -> String {
    let list: Vec<String> = servers.iter().filter(|s| safe_host(s)).take(MAX_TRIES).map(|s| format!("'{s}'")).collect();
    format!(
        "$ua='Nock/{}'\n$servers=@({})\nif($servers.Count -eq 0){{ try {{ $servers=@((Get-Utf8 '{SERVER_LIST}' | ConvertFrom-Json) | ForEach-Object {{ $_.name }} | Sort-Object -Unique | Get-Random -Count 1000) }} catch {{ 'ERR|server list: '+(C $_.Exception.Message); exit }} }}\nforeach($s in $servers){{ \"SRV|$s\" }}\n",
        env!("CARGO_PKG_VERSION"),
        list.join(",")
    )
}

/// The whole PowerShell script for one directory query. None if the path is not safe.
#[cfg(test)]
pub fn search_script(path: &str, servers: &[String]) -> Option<String> {
    search_script_multi(&[path.to_string()], servers)
}

/// One script for several requests to the same mirror; each request's stations follow a
/// `PART|i` line. None if there are no paths or any path is not safe.
pub fn search_script_multi(paths: &[String], servers: &[String]) -> Option<String> {
    if paths.is_empty() || !paths.iter().all(|p| safe_path(p)) {
        return None;
    }
    let list: Vec<String> = paths.iter().map(|p| format!("'{p}'")).collect();
    Some(format!(
        "{PROLOGUE}{}$n=0\nforeach($s in $servers){{\n  if($n -ge {MAX_TRIES}){{ break }}; $n++\n  try {{\n    $parts=@()\n    foreach($p in @({})){{ $parts += ,(Get-Utf8 ('https://'+$s+$p) | ConvertFrom-Json) }}\n    \"OK|$s\"\n    $i=0\n    foreach($j in $parts){{\n      \"PART|$i\"; $i++\n      foreach($x in $j){{ 'ST|'+(C $x.stationuuid)+'|'+(C $x.name)+'|'+(C $x.countrycode)+'|'+(C $x.codec)+'|'+(C $x.bitrate)+'|'+(C $x.hls)+'|'+(C $x.tags)+'|'+(C $x.url_resolved) }}\n    }}\n    exit\n  }} catch {{ \"FAIL|$s|\"+(C $_.Exception.Message) }}\n}}\n'ERR|no server answered'\n",
        servers_block(servers),
        list.join(",")
    ))
}

/// The click-count call made once when a station starts playing. None if the uuid is odd.
pub fn click_script(uuid: &str, servers: &[String]) -> Option<String> {
    if !safe_uuid(uuid) {
        return None;
    }
    Some(format!(
        "{PROLOGUE}{}$n=0\nforeach($s in $servers){{\n  if($n -ge {MAX_TRIES}){{ break }}; $n++\n  try {{ $null=Get-Utf8 ('https://'+$s+'/json/url/{uuid}'); \"OK|$s\"; exit }} catch {{ \"FAIL|$s|\"+(C $_.Exception.Message) }}\n}}\n",
        servers_block(servers)
    ))
}

// ------------------------------------------------------------ script output

#[derive(Debug, Default, PartialEq)]
pub struct Output {
    /// Every mirror the script listed (`SRV|`), in the order it tried them.
    pub servers: Vec<String>,
    /// The mirror that answered (`OK|`).
    pub used: Option<String>,
    /// Mirrors that failed (`FAIL|`).
    pub failed: Vec<String>,
    /// All stations: the parts interleaved, repeats (same uuid) dropped, at most `LIMIT`.
    pub stations: Vec<Station>,
    /// Stations per request (`PART|i`). Lines before any `PART` count as part 0.
    pub parts: Vec<Vec<Station>>,
    /// Why nothing came back (`ERR|`), if so.
    pub error: Option<String>,
}

/// Read the script's output. Unknown or broken lines are ignored.
pub fn parse_output(text: &str) -> Output {
    let mut o = Output::default();
    let mut part = 0usize;
    for line in text.lines() {
        let Some((tag, rest)) = line.split_once('|') else { continue };
        match tag {
            "SRV" if safe_host(rest.trim()) => o.servers.push(rest.trim().into()),
            "OK" => o.used = Some(rest.trim().into()),
            "FAIL" => o.failed.push(rest.split('|').next().unwrap_or("").trim().into()),
            "ERR" => o.error = Some(rest.trim().into()),
            "PART" => part = rest.trim().parse::<usize>().unwrap_or(0).min(7),
            "ST" => {
                if let Some(st) = parse_line(rest) {
                    if o.parts.len() <= part {
                        o.parts.resize(part + 1, Vec::new());
                    }
                    o.parts[part].push(st);
                }
            }
            _ => {}
        }
    }
    o.stations = interleave(&o.parts);
    o
}

/// One from each part in turn (so name matches and genre matches both show), without
/// repeating a station, at most `LIMIT`.
pub fn interleave(parts: &[Vec<Station>]) -> Vec<Station> {
    let mut out: Vec<Station> = Vec::new();
    let longest = parts.iter().map(Vec::len).max().unwrap_or(0);
    for i in 0..longest {
        for st in parts.iter().filter_map(|p| p.get(i)) {
            if out.len() < LIMIT as usize && !out.iter().any(|o| o.uuid == st.uuid) {
                out.push(st.clone());
            }
        }
    }
    out
}

// ------------------------------------------------------------ mirror order

/// Mirrors that answered before, kept for this run of Nock only.
static SERVERS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Shuffle with a small xorshift so each fetch starts at a random mirror.
pub fn shuffle(v: &mut [String], mut seed: u64) {
    seed |= 1;
    for i in (1..v.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        v.swap(i, (seed % (i as u64 + 1)) as usize);
    }
}

fn seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7)
}

/// Known mirrors in random order (empty the first time: the script fetches the list).
fn mirrors() -> Vec<String> {
    let mut v = SERVERS.lock().unwrap().clone();
    shuffle(&mut v, seed());
    v
}

/// Remember the mirror list the script saw, minus those that failed.
fn learn(o: &Output) {
    let mut known = SERVERS.lock().unwrap();
    if known.is_empty() {
        *known = o.servers.clone();
        known.sort();
        known.dedup();
    }
    known.retain(|s| !o.failed.contains(s));
}

// ------------------------------------------------------------------ fetch

/// What a finished fetch left for the UI.
#[derive(Debug)]
pub struct Fetched {
    pub id: usize,
    pub query: Query,
    /// The stations, or a short error for the UI.
    pub result: Result<Vec<Station>, String>,
}

static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
/// Finished fetches the UI has not taken yet. Two can finish close together (the Top list
/// and a search), so each is kept under its own id; only the newest few are kept.
static DONE: Mutex<Vec<Fetched>> = Mutex::new(Vec::new());
const KEEP: usize = 4;

/// Run the query on a short-lived thread. When it ends, the result is waiting in
/// [`take`] and `WM_TUNER` is posted to `hwnd` with WPARAM = the returned id.
pub fn fetch(hwnd: isize, query: Query) -> usize {
    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    trace(&format!("fetch {id} start {query:?}"));
    std::thread::spawn(move || {
        let result = run_query(&query);
        trace(&format!("fetch {id} done {}", result.as_ref().map_or_else(|e| format!("error {e}"), |v| format!("{} stations", v.len()))));
        {
            let mut d = DONE.lock().unwrap();
            d.push(Fetched { id, query, result });
            let n = d.len();
            if n > KEEP {
                d.drain(..n - KEEP);
            }
        }
        unsafe {
            use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
            let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_TUNER, WPARAM(id), LPARAM(0));
        }
    });
    id
}

/// The finished fetch with this id, if it is still kept.
pub fn take(id: usize) -> Option<Fetched> {
    let mut d = DONE.lock().unwrap();
    let i = d.iter().position(|f| f.id == id)?;
    Some(d.remove(i))
}

/// Blocking: run one query now. `fetch` calls this on its own thread.
pub fn run_query(q: &Query) -> Result<Vec<Station>, String> {
    let script = search_script_multi(&query_paths(q), &mirrors()).ok_or("bad query")?;
    let text = crate::sys::powershell(&script).ok_or("Network error")?;
    let o = parse_output(&text);
    learn(&o);
    match (&o.used, o.error) {
        (Some(_), _) => Ok(o.stations),
        (None, Some(e)) => Err(e),
        (None, None) => Err("Network error".into()),
    }
}

/// Tell Radio Browser a station was played (their click-count etiquette). Fire and forget.
pub fn click(uuid: &str) {
    let Some(script) = click_script(uuid, &mirrors()) else { return };
    trace(&format!("click {uuid}"));
    std::thread::spawn(move || {
        if let Some(text) = crate::sys::powershell(&script) {
            learn(&parse_output(&text));
        }
    });
}

/// The user's Windows region as a two-letter code (`GetUserDefaultGeoName`), or None
/// (for example "001", the world) to mean no country filter.
pub fn default_country() -> Option<String> {
    use windows::Win32::Globalization::GetUserDefaultGeoName;
    let mut buf = [0u16; 16];
    let n = unsafe { GetUserDefaultGeoName(&mut buf) };
    if n <= 1 {
        return None;
    }
    country_code(&String::from_utf16_lossy(&buf[..(n as usize - 1).min(buf.len())]))
}

// --------------------------------------------------------------- playback

/// What a player event means for the UI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlayEv {
    Opened,
    Failed(Fail),
    State(PlayState),
    /// `MediaEnded`: a live stream has no end, so the station closed the connection.
    Ended,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlayState {
    Opening,
    Buffering,
    Playing,
    Paused,
    Other,
}

/// Why a stream failed. `MediaFailed` gives only a code, so Nock words it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fail {
    Offline,
    Format,
    Network,
}

impl Fail {
    pub fn text(self) -> &'static str {
        match self {
            Fail::Offline => "Station offline",
            Fail::Format => "Format not supported",
            Fail::Network => "Network error",
        }
    }

    /// From `MediaPlayerError` (0 unknown, 1 aborted, 2 network, 3 decoding, 4 source not
    /// supported) and the extended HRESULT.
    pub fn classify(error: i32, hr: u32) -> Fail {
        // unsupported byte stream / service, codec not found, invalid format
        const FORMAT_HR: [u32; 4] = [0xC00D_36C4, 0xC00D_36B4, 0xC00D_5212, 0xC00D_36EF];
        // WinINet: name not resolved, cannot connect, timed out, connection aborted
        const NETWORK_HR: [u32; 4] = [0x8007_2EE7, 0x8007_2EFD, 0x8007_2EE2, 0x8007_2EFE];
        if error == 3 || FORMAT_HR.contains(&hr) {
            Fail::Format
        } else if error == 2 || NETWORK_HR.contains(&hr) {
            Fail::Network
        } else {
            Fail::Offline
        }
    }
}

/// LPARAM of `WM_TUNER_PLAY`: low byte = kind, next byte = detail.
pub fn encode_event(e: PlayEv) -> isize {
    let (kind, v) = match e {
        PlayEv::Opened => (1, 0),
        PlayEv::Failed(f) => (2, f as isize),
        PlayEv::State(s) => (3, s as isize),
        PlayEv::Ended => (4, 0),
    };
    kind | (v << 8)
}

pub fn decode_event(lp: isize) -> Option<PlayEv> {
    let v = ((lp >> 8) & 0xff) as usize;
    Some(match lp & 0xff {
        1 => PlayEv::Opened,
        2 => PlayEv::Failed([Fail::Offline, Fail::Format, Fail::Network].get(v).copied().unwrap_or(Fail::Offline)),
        3 => PlayEv::State([PlayState::Opening, PlayState::Buffering, PlayState::Playing, PlayState::Paused, PlayState::Other].get(v).copied().unwrap_or(PlayState::Other)),
        4 => PlayEv::Ended,
        _ => return None,
    })
}

/// One playing station. Created on play and dropped on stop, so no player exists while
/// radio is stopped. Dropping it pauses and closes the WinRT player.
pub struct Player {
    mp: windows::Media::Playback::MediaPlayer,
    pub gen: usize,
}

impl Player {
    /// Start playing `st` (its `url_resolved`). Events from this player carry `gen`.
    pub fn start(hwnd: isize, st: &Station, gen: usize) -> windows::core::Result<Player> {
        use windows::core::{Ref, HSTRING};
        use windows::Foundation::{TypedEventHandler, Uri};
        use windows::Media::Core::MediaSource;
        use windows::Media::MediaPlaybackType;
        use windows::Media::Playback::{MediaPlaybackItem, MediaPlaybackSession, MediaPlaybackState, MediaPlayer, MediaPlayerFailedEventArgs};

        let post = move |e: PlayEv| unsafe {
            use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
            let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_TUNER_PLAY, WPARAM(gen), LPARAM(encode_event(e)));
        };
        let mp = MediaPlayer::new()?;
        // The handlers run on WinRT worker threads: they only post a message.
        mp.MediaOpened(&TypedEventHandler::new(move |_, _| {
            post(PlayEv::Opened);
            Ok(())
        }))?;
        mp.MediaFailed(&TypedEventHandler::new(move |_, args: Ref<MediaPlayerFailedEventArgs>| {
            let (err, hr) = match args.ok() {
                Ok(a) => (a.Error().map(|e| e.0).unwrap_or(0), a.ExtendedErrorCode().map(|h| h.0 as u32).unwrap_or(0)),
                Err(_) => (0, 0),
            };
            post(PlayEv::Failed(Fail::classify(err, hr)));
            Ok(())
        }))?;
        mp.MediaEnded(&TypedEventHandler::new(move |_, _| {
            post(PlayEv::Ended);
            Ok(())
        }))?;
        mp.PlaybackSession()?.PlaybackStateChanged(&TypedEventHandler::new(move |s: Ref<MediaPlaybackSession>, _| {
            let st = match s.ok().and_then(|s| s.PlaybackState()) {
                Ok(MediaPlaybackState::Opening) => PlayState::Opening,
                Ok(MediaPlaybackState::Buffering) => PlayState::Buffering,
                Ok(MediaPlaybackState::Playing) => PlayState::Playing,
                Ok(MediaPlaybackState::Paused) => PlayState::Paused,
                _ => PlayState::Other,
            };
            post(PlayEv::State(st));
            Ok(())
        }))?;
        // Title and artist go on the playback item before playing: nothing wipes them (T1).
        let src = MediaSource::CreateFromUri(&Uri::CreateUri(&HSTRING::from(&st.url))?)?;
        let item = MediaPlaybackItem::Create(&src)?;
        let props = item.GetDisplayProperties()?;
        props.SetType(MediaPlaybackType::Music)?;
        let music = props.MusicProperties()?;
        music.SetTitle(&HSTRING::from(&st.name))?;
        music.SetArtist(&HSTRING::from(country_name(&st.countrycode)))?;
        item.ApplyDisplayProperties(&props)?;
        mp.SetSource(&item)?;
        mp.Play()?;
        trace(&format!("player {gen} play {}", st.name));
        Ok(Player { mp, gen })
    }

    /// Play again after a pause from the Windows media flyout or Nock's Media card.
    pub fn resume(&self) {
        let _ = self.mp.Play();
    }
}

impl Drop for Player {
    /// Silence at once on the calling (UI) thread, then let a short-lived thread do the
    /// slow part: `Close` takes about 0.3 s and would freeze the panel. The thread exits
    /// as soon as the player is closed and released.
    fn drop(&mut self) {
        let t = std::time::Instant::now();
        let _ = self.mp.SetIsMuted(true);
        let _ = self.mp.Pause();
        let gen = self.gen;
        trace(&format!("player {gen} silent in {} ms", t.elapsed().as_millis()));
        let mp = self.mp.clone();
        let spawned = std::thread::Builder::new().name("nock-radio-close".into()).spawn(move || {
            use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
            let t = std::time::Instant::now();
            let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
            let _ = mp.Close();
            drop(mp);
            if com {
                unsafe { CoUninitialize() };
            }
            trace(&format!("player {gen} closed off the UI thread in {} ms", t.elapsed().as_millis()));
        });
        if spawned.is_err() {
            // no thread to be had: close here rather than leave the player open
            let _ = self.mp.Close();
        }
    }
}

/// The artist line in the media session: the country's name when Windows knows the code.
fn country_name(cc: &str) -> String {
    use windows::Win32::Globalization::{GetGeoInfoEx, GEO_FRIENDLYNAME};
    let Some(code) = country_code(cc) else { return cc.to_string() };
    let mut buf = [0u16; 128];
    let n = unsafe { GetGeoInfoEx(&windows::core::HSTRING::from(&code), GEO_FRIENDLYNAME, Some(&mut buf)) };
    if n > 1 {
        String::from_utf16_lossy(&buf[..(n as usize - 1).min(buf.len())])
    } else {
        code
    }
}

/// Timing notes for measuring, written only when the `NOCK_RADIO_TRACE` environment
/// variable names a file. Off otherwise.
pub fn trace(what: &str) {
    use std::io::Write;
    static PATH: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    let Some(p) = PATH.get_or_init(|| std::env::var_os("NOCK_RADIO_TRACE").map(Into::into)) else { return };
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "{ms} {what}");
    }
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "96062a7b-0601-11e8-ae97-52543be04c81|Radio Paradise|us|AAC|320|0|eclectic,rock|http://stream-uk1.radioparadise.com/aac-320";

    #[test]
    fn good_line() {
        let s = parse_line(GOOD).unwrap();
        assert_eq!(s.uuid, "96062a7b-0601-11e8-ae97-52543be04c81");
        assert_eq!(s.name, "Radio Paradise");
        assert_eq!(s.countrycode, "US");
        assert_eq!(s.codec, "AAC");
        assert_eq!(s.bitrate, 320);
        assert!(!s.hls);
        assert_eq!(s.tags, "eclectic,rock");
        assert_eq!(s.url, "http://stream-uk1.radioparadise.com/aac-320");
        assert_eq!(s.bitrate_label().as_deref(), Some("320 kbps"));
        assert_eq!(s.codec_label(), "AAC");
    }

    #[test]
    fn short_and_long_lines_are_skipped() {
        assert!(parse_line("abc|Name|US|MP3|128|0|http://x").is_none()); // 7 fields
        assert!(parse_line(&format!("{GOOD}|extra")).is_none());
        assert!(parse_line("").is_none());
        assert!(parse_line("garbage").is_none());
    }

    #[test]
    fn empty_fields() {
        let s = parse_line("u1|Name||||||https://x.example/stream").unwrap();
        assert_eq!((s.countrycode.as_str(), s.codec.as_str(), s.bitrate, s.hls, s.tags.as_str()), ("", "", 0, false, ""));
        assert!(parse_line("|Name|US|MP3|128|0||https://x").is_none(), "no uuid");
        assert!(parse_line("u1||US|MP3|128|0||https://x").is_none(), "no name");
        assert!(parse_line("u1|Name|US|MP3|128|0||").is_none(), "no url");
        assert!(parse_line("u1|Name|US|MP3|128|0||ftp://x").is_none(), "not http");
    }

    #[test]
    fn bitrate_zero_and_junk() {
        let s = parse_line("u1|Armisa|IT|MP3|0|0||https://onair.armisa.it/radio.mp3").unwrap();
        assert_eq!(s.bitrate, 0);
        assert_eq!(s.bitrate_label(), None);
        assert_eq!(parse_line("u1|A|IT|MP3|lots|0||https://a").unwrap().bitrate, 0);
    }

    #[test]
    fn hls_flag() {
        let s = parse_line("u1|RTL|FR|AAC|64|1|news|https://live.example/index.m3u8").unwrap();
        assert!(s.hls);
        assert_eq!(s.codec_label(), "HLS");
    }

    #[test]
    fn script_output() {
        let text = format!("SRV|de1.api.radio-browser.info\r\nSRV|nl1.api.radio-browser.info\nSRV|bad host!\nFAIL|de1.api.radio-browser.info|timed out\nOK|nl1.api.radio-browser.info\nST|{GOOD}\nST|broken|line\nnoise\n");
        let o = parse_output(&text);
        assert_eq!(o.servers, ["de1.api.radio-browser.info", "nl1.api.radio-browser.info"]);
        assert_eq!(o.failed, ["de1.api.radio-browser.info"]);
        assert_eq!(o.used.as_deref(), Some("nl1.api.radio-browser.info"));
        assert_eq!(o.stations.len(), 1);
        assert_eq!(o.error, None);
        assert_eq!(parse_output("ERR|no server answered").error.as_deref(), Some("no server answered"));
    }

    #[test]
    fn query_paths() {
        let tail = "order=clickcount&reverse=true&hidebroken=true&limit=50";
        assert_eq!(query_path(&Query::Top(Some("il".into()))), format!("/json/stations/search?countrycode=IL&{tail}"));
        assert_eq!(query_path(&Query::Top(None)), format!("/json/stations/search?{tail}"));
        assert_eq!(query_path(&Query::Top(Some("001".into()))), format!("/json/stations/search?{tail}"));
        assert_eq!(query_path(&Query::Name("BBC Radio 4".into())), format!("/json/stations/search?name=BBC%20Radio%204&{tail}"));
        assert_eq!(query_path(&Query::Tag(" jazz ".into())), format!("/json/stations/search?tag=jazz&{tail}"));
        assert_eq!(query_path(&Query::Country("de".into())), format!("/json/stations/search?countrycode=DE&{tail}"));
        assert_eq!(query_path(&Query::Name("Café & co".into())), format!("/json/stations/search?name=Caf%C3%A9%20%26%20co&{tail}"));
    }

    const HOSTILE: [&str; 6] = [
        "'; Remove-Item x; '",
        "$(calc)",
        "`calc`; \"quoted\"",
        "a\nRemove-Item x\r\nb",
        "\"; Start-Process calc; \"",
        "' + (Remove-Item x) + '",
    ];

    fn decode(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn hostile_search_only_reaches_the_script_encoded() {
        let servers = vec!["de1.api.radio-browser.info".to_string()];
        let benign = search_script(&query_path(&Query::Name("x".into())), &servers).unwrap();
        for evil in HOSTILE {
            for q in [Query::Name(evil.into()), Query::Tag(evil.into()), Query::Country(evil.into())] {
                let path = query_path(&q);
                let script = search_script(&path, &servers).expect("encoded path is always safe");
                // Only safe characters, and they decode back to exactly what was typed.
                assert!(safe_path(&path), "{path}");
                let enc = path.split('=').nth(1).unwrap().split('&').next().unwrap();
                let typed = match &q {
                    Query::Country(s) => s.trim().to_ascii_uppercase(),
                    Query::Name(s) | Query::Tag(s) => s.trim().to_string(),
                    Query::Top(_) | Query::Any(_) => unreachable!(),
                };
                assert_eq!(decode(enc), typed);
                // No PowerShell syntax character survives encoding, and the raw input is not in the script.
                for c in ['\'', '"', ';', '$', '`', '(', ')', '+', ' ', '\n', '\r', '&', '|'] {
                    assert!(!enc.contains(c), "{c:?} left raw in {enc} for {evil:?}");
                }
                assert!(!script.contains(evil));
                for raw in ["$(calc)", "`calc`", "'; Remove", "\"; Start"] {
                    assert!(!script.contains(raw), "{raw:?} reached the script raw for {evil:?}");
                }
                // The input changes nothing in the script except the encoded value.
                let key = path.split('=').next().unwrap().rsplit('?').next().unwrap();
                let normalise = |s: &str, v: &str| s.replace(&format!("{key}={v}&"), "QUERY&");
                assert_eq!(
                    normalise(&script, enc),
                    normalise(&search_script(&query_path(&match &q {
                        Query::Name(_) => Query::Name("x".into()),
                        Query::Tag(_) => Query::Tag("x".into()),
                        _ => Query::Country("x".into()),
                    }), &servers).unwrap(), if key == "countrycode" { "X" } else { "x" })
                );
            }
        }
        assert!(benign.contains("name=x&"));
    }

    #[test]
    fn unsafe_pieces_are_refused() {
        assert!(search_script("/json/x?name='; calc", &[]).is_none());
        assert!(click_script("abc'; calc", &[]).is_none());
        assert!(click_script("96062a7b-0601-11e8-ae97-52543be04c81", &[]).is_some());
        // A mirror name from the network with odd characters is left out of the script.
        let s = search_script("/json/stations/search?limit=50", &["evil'; calc; '".into(), "nl1.api.radio-browser.info".into()]).unwrap();
        assert!(!s.contains("calc"));
        assert!(s.contains("'nl1.api.radio-browser.info'"));
        let o = parse_output("SRV|evil'; calc\nSRV|ok.example");
        assert_eq!(o.servers, ["ok.example"]);
    }

    #[test]
    fn script_has_user_agent_and_tries_a_few_mirrors() {
        let s = search_script(&query_path(&Query::Top(None)), &[]).unwrap();
        assert!(s.contains(&format!("$ua='Nock/{}'", env!("CARGO_PKG_VERSION"))));
        assert!(s.contains(SERVER_LIST));
        assert!(s.contains("url_resolved"));
        let many: Vec<String> = (0..10).map(|i| format!("s{i}.example")).collect();
        let s = search_script("/json/x", &many).unwrap();
        assert!(s.contains("'s3.example'") && !s.contains("'s4.example'"));
    }

    #[test]
    fn shuffle_keeps_every_mirror() {
        let orig: Vec<String> = (0..8).map(|i| format!("s{i}")).collect();
        let mut v = orig.clone();
        shuffle(&mut v, 12345);
        let mut sorted = v.clone();
        sorted.sort();
        assert_eq!(sorted, orig);
        let mut w = orig.clone();
        shuffle(&mut w, 999);
        assert_ne!(v, w, "different seeds give different orders");
    }

    #[test]
    fn favorite_round_trip() {
        let s = parse_line(GOOD).unwrap();
        let back = Station::from_favorite(&s.to_favorite());
        assert_eq!(back, Station { tags: String::new(), ..s });
    }

    #[test]
    fn hls_favorite_round_trip() {
        let s = parse_line("u1|RTL|FR|AAC|64|1|news|https://live.example/index.m3u8").unwrap();
        let back = Station::from_favorite(&s.to_favorite());
        assert!(back.hls);
        assert_eq!(back.codec_label(), "HLS");
    }

    #[test]
    fn message_number() {
        assert_eq!(WM_TUNER, 0x8000 + 13);
        assert_eq!(WM_TUNER_PLAY, 0x8000 + 14);
    }

    #[test]
    fn search_box_text() {
        assert_eq!(query_for("  "), None);
        assert_eq!(query_for("de"), Some(Query::Country("DE".into())));
        assert_eq!(query_for(" GB "), Some(Query::Country("GB".into())));
        assert_eq!(query_for("jazz"), Some(Query::Any("jazz".into())));
        assert_eq!(query_for("BBC"), Some(Query::Any("BBC".into())));
        assert_eq!(query_for("d3"), Some(Query::Any("d3".into())));
        let tail = "order=clickcount&reverse=true&hidebroken=true&limit=50";
        assert_eq!(
            super::query_paths(&Query::Any("BBC Radio".into())),
            [format!("/json/stations/search?name=BBC%20Radio&{tail}"), format!("/json/stations/search?tag=bbc%20radio&{tail}")]
        );
        assert_eq!(super::query_paths(&Query::Country("de".into())), [format!("/json/stations/search?countrycode=DE&{tail}")]);
    }

    #[test]
    fn hostile_search_box_text_reaches_the_script_encoded() {
        let servers = vec!["de1.api.radio-browser.info".to_string()];
        for evil in HOSTILE {
            let q = query_for(evil).unwrap();
            let paths = super::query_paths(&q);
            assert!(paths.iter().all(|p| safe_path(p)), "{paths:?}");
            let script = search_script_multi(&paths, &servers).expect("encoded paths are safe");
            assert!(!script.contains(evil));
            // letters, digits and `-_.~` may stay as typed (inside the quoted path that is
            // harmless); every quote, `;`, `$`, backtick, bracket, space and line break is encoded
            for raw in ["$(calc)", "`calc`", "'; Remove", "\"; Start", "Remove-Item x", "Start-Process calc"] {
                assert!(!script.contains(raw), "{raw:?} reached the script raw for {evil:?}");
            }
            for p in &paths {
                let value = p.split('=').nth(1).unwrap().split('&').next().unwrap();
                for c in ['\'', '"', ';', '$', '`', '(', ')', '+', ' ', '\n', '\r', '|'] {
                    assert!(!value.contains(c), "{c:?} left raw in {value} for {evil:?}");
                }
            }
        }
        // an unsafe path anywhere in the list refuses the whole script
        assert!(search_script_multi(&["/json/ok".into(), "/json/x?name=';calc".into()], &servers).is_none());
        assert!(search_script_multi(&[], &servers).is_none());
    }

    #[test]
    fn parts_are_interleaved_without_repeats() {
        let st = |u: &str| format!("ST|{u}|Station {u}|DE|MP3|128|0||https://{u}.example/s");
        let text = [
            "OK|de1.api.radio-browser.info".to_string(),
            "PART|0".into(),
            st("a1"),
            st("a2"),
            st("both"),
            "PART|1".into(),
            st("b1"),
            st("both"),
        ]
        .join("\n");
        let o = parse_output(&text);
        assert_eq!(o.parts.len(), 2);
        let ids: Vec<&str> = o.stations.iter().map(|s| s.uuid.as_str()).collect();
        assert_eq!(ids, ["a1", "b1", "a2", "both"]);
        // two full parts are capped at the list limit
        let many: Vec<Vec<Station>> = (0..2).map(|p| (0..50).map(|i| Station { uuid: format!("{p}-{i}"), ..Default::default() }).collect()).collect();
        assert_eq!(interleave(&many).len(), LIMIT as usize);
    }

    #[test]
    fn two_finished_fetches_are_both_kept() {
        let put = |id: usize| DONE.lock().unwrap().push(Fetched { id, query: Query::Top(None), result: Ok(vec![]) });
        put(1001);
        put(1002);
        assert_eq!(take(1001).map(|f| f.id), Some(1001));
        assert_eq!(take(1002).map(|f| f.id), Some(1002));
        assert!(take(1001).is_none());
    }

    #[test]
    fn player_events_round_trip() {
        let all = [
            PlayEv::Opened,
            PlayEv::Failed(Fail::Offline),
            PlayEv::Failed(Fail::Format),
            PlayEv::Failed(Fail::Network),
            PlayEv::State(PlayState::Opening),
            PlayEv::State(PlayState::Buffering),
            PlayEv::State(PlayState::Playing),
            PlayEv::State(PlayState::Paused),
            PlayEv::State(PlayState::Other),
            PlayEv::Ended,
        ];
        for e in all {
            assert_eq!(decode_event(encode_event(e)), Some(e));
        }
        assert_eq!(decode_event(0), None);
        assert_eq!(decode_event(99), None);
    }

    #[test]
    fn failure_words_are_nocks_own() {
        assert_eq!(Fail::classify(2, 0).text(), "Network error");
        assert_eq!(Fail::classify(3, 0).text(), "Format not supported");
        assert_eq!(Fail::classify(4, 0xC00D_36C4).text(), "Format not supported");
        assert_eq!(Fail::classify(4, 0xC00D_001A).text(), "Station offline");
        assert_eq!(Fail::classify(0, 0x8007_2EE7).text(), "Network error");
        assert_eq!(Fail::classify(0, 0).text(), "Station offline");
    }

    /// Live check against the real directory. Not run by `cargo test`:
    /// `cargo test --release live_fetch -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_fetch() {
        let cc = default_country();
        println!("default country: {cc:?}");
        for q in [Query::Top(cc.clone()), Query::Tag("jazz".into()), Query::Name("BBC".into())] {
            let t = std::time::Instant::now();
            let r = run_query(&q);
            let ms = t.elapsed().as_millis();
            match &r {
                Ok(v) => println!(
                    "{q:?}: {} stations in {ms} ms; first: {:?}; hls={} bitrate0={} non-ascii names={}",
                    v.len(),
                    v.first().map(|s| (&s.name, &s.countrycode, s.codec_label(), s.bitrate)),
                    v.iter().filter(|s| s.hls).count(),
                    v.iter().filter(|s| s.bitrate == 0).count(),
                    v.iter().filter(|s| !s.name.is_ascii()).count()
                ),
                Err(e) => println!("{q:?}: ERROR {e} after {ms} ms"),
            }
            if let Ok(v) = &r {
                let odd: Vec<&str> = v.iter().filter(|s| !s.name.is_ascii()).take(3).map(|s| s.name.as_str()).collect();
                println!("  sample non-ASCII names: {odd:?}");
            }
            assert!(r.is_ok_and(|v| !v.is_empty()));
        }
        println!("mirrors known now: {:?}", SERVERS.lock().unwrap());

        // The threaded path the UI uses (no window here, so the message goes nowhere).
        let t = std::time::Instant::now();
        let id = fetch(0, Query::Country("de".into()));
        let got = loop {
            if let Some(f) = take(id) {
                break f;
            }
            assert!(t.elapsed().as_secs() < 30, "fetch thread never finished");
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        println!("fetch() thread: {:?} -> {} stations in {} ms", got.query, got.result.as_ref().map_or(0, Vec::len), t.elapsed().as_millis());
        assert_eq!(got.id, id);
        assert!(got.result.is_ok_and(|v| !v.is_empty()));
    }
}
