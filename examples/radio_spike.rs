//! T1 throwaway playback spike for Nock Radio. Not part of the app.
//!
//! Plays internet radio through WinRT `Windows.Media.Playback.MediaPlayer` and
//! measures what the real `tuner.rs` will need to know.
//!
//! ```text
//! cargo run --release --example radio_spike                 # fetch MP3, AAC, HLS + dead URL, play each
//! cargo run --release --example radio_spike -- <url> [name] [country]
//! flags: --secs N   play time per stream (default 60)
//!        --hold N   seconds to stay alive after the last stop, for the "stopped" numbers (default 10)
//!        --wait N   seconds to wait for first sound or MediaFailed (default 20)
//!        --sta      initialise the main thread as STA (Nock's UI thread) instead of MTA; no message pump
//!        --pump     with --sta: pump window messages (like Nock's UI thread) while waiting
//!        --aumid    call SetCurrentProcessExplicitAppUserModelID before creating the player
//!        --display early|opened|item
//!                   when/how the title is set: through the SMTC DisplayUpdater before Play (early,
//!                   default) or after MediaOpened (opened), or through MediaPlaybackItem display
//!                   properties (item)
//! ```
//!
//! "First sound" is measured from the process's own audio session peak meter
//! (IAudioMeterInformation), so it is objective rather than by ear. The meter is
//! only polled while waiting for first sound and while waiting for silence after
//! stop; the 60 s measurement window does no polling.

#[cfg(not(windows))]
fn main() {
    eprintln!("radio_spike is Windows only");
}

#[cfg(windows)]
fn main() {
    if let Err(e) = win::run() {
        eprintln!("spike error: {e:?}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod win {
    use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
    use std::time::{Duration, Instant};

    use windows::core::{Interface, Result, HSTRING};
    use windows::Foundation::{TypedEventHandler, Uri};
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager as Gsmtc;
    use windows::Media::Core::MediaSource;
    use windows::Media::MediaPlaybackType;
    use windows::Media::Playback::{IMediaPlaybackSource, MediaPlaybackItem, MediaPlaybackState, MediaPlayer};
    use windows::Win32::Foundation::{CloseHandle, FILETIME};
    use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
        MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, GetProcessTimes,
    };

    const UA: &str = "Nock/0.13.0 (T1 radio spike)";

    struct Opts {
        secs: u64,
        hold: u64,
        wait: u64,
        sta: bool,
        pump: bool,
        aumid: bool,
        display: String,
        url: Option<(String, String, String)>,
    }

    fn opts() -> Opts {
        let mut o = Opts { secs: 60, hold: 10, wait: 20, sta: false, pump: false, aumid: false, display: "early".into(), url: None };
        let mut free = Vec::new();
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--secs" => o.secs = it.next().and_then(|v| v.parse().ok()).unwrap_or(60),
                "--hold" => o.hold = it.next().and_then(|v| v.parse().ok()).unwrap_or(10),
                "--wait" => o.wait = it.next().and_then(|v| v.parse().ok()).unwrap_or(20),
                "--sta" => o.sta = true,
                "--pump" => o.pump = true,
                "--aumid" => o.aumid = true,
                "--display" => o.display = it.next().unwrap_or_else(|| "early".into()),
                _ => free.push(a),
            }
        }
        if let Some(u) = free.first() {
            let name = free.get(1).cloned().unwrap_or_else(|| "Spike station".into());
            let country = free.get(2).cloned().unwrap_or_else(|| "Nowhere".into());
            o.url = Some((u.clone(), name, country));
        }
        o
    }

    struct Station {
        label: String,
        name: String,
        country: String,
        codec: String,
        bitrate: String,
        url: String,
    }

    pub fn run() -> Result<()> {
        let o = opts();
        unsafe {
            CoInitializeEx(None, if o.sta { COINIT_APARTMENTTHREADED } else { COINIT_MULTITHREADED }).ok()?;
            if o.aumid {
                windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(windows::core::w!(
                    "Nock.RadioSpike"
                ))?;
            }
        }
        println!(
            "apartment={} pump={} aumid={} display={} main_thread={} pid={} cpus={}",
            if o.sta { "STA" } else { "MTA" },
            o.pump,
            o.aumid,
            o.display,
            unsafe { GetCurrentThreadId() },
            unsafe { GetCurrentProcessId() },
            ncpu()
        );
        let base = Sample::now();
        println!(
            "baseline before any player: ws={:.1} MB private={:.1} MB threads={}",
            base.ws_mb,
            base.priv_mb,
            thread_count()
        );

        let stations = match o.url.clone() {
            Some((url, name, country)) => vec![Station {
                label: "ARG".into(),
                name,
                country,
                codec: "?".into(),
                bitrate: "?".into(),
                url,
            }],
            None => {
                let mut v = fetch_stations();
                v.push(Station {
                    label: "DEAD".into(),
                    name: "Dead URL".into(),
                    country: "None".into(),
                    codec: "MP3".into(),
                    bitrate: "0".into(),
                    url: "http://127.0.0.1:9/none.mp3".into(),
                });
                v
            }
        };

        for s in &stations {
            play_one(s, &o)?;
        }

        println!("\n== stopped: holding {} s with no player ==", o.hold);
        let a = Sample::now();
        std::thread::sleep(Duration::from_secs(o.hold));
        let b = Sample::now();
        println!(
            "stopped: cpu={:.3}% of machine ({:.3}% of one core) ws={:.1} MB private={:.1} MB threads={}",
            cpu_pct(&a, &b) / ncpu() as f64,
            cpu_pct(&a, &b),
            b.ws_mb,
            b.priv_mb,
            thread_count()
        );
        Ok(())
    }

    // ---------- directory (spike only: PowerShell, like cal.rs) ----------

    fn fetch_stations() -> Vec<Station> {
        let script = format!(
            r#"$ErrorActionPreference='Stop'
$h=@{{'User-Agent'='{UA}'}}
function clean($v){{ "$v" -replace '[|\r\n]',' ' }}
function line($lab,$x){{ "ST|$lab|$(clean $x.name)|$(clean $x.country)|$(clean $x.codec)|$(clean $x.bitrate)|$(clean $x.url_resolved)" }}
$servers = Invoke-RestMethod -Uri 'https://all.api.radio-browser.info/json/servers' -Headers $h -TimeoutSec 10
$names = $servers | ForEach-Object {{ $_.name }} | Sort-Object -Unique | Get-Random -Count 100
foreach($srv in $names){{
  try {{
    "SERVER|$srv"
    foreach($c in 'MP3','AAC'){{
      $r = Invoke-RestMethod -Uri "https://$srv/json/stations/search?codec=$c&order=clickcount&reverse=true&hidebroken=true&limit=5" -Headers $h -TimeoutSec 10
      line $c $r[0]
      foreach($y in $r[1..4]){{ "ALT|$c|$(clean $y.name)|$(clean $y.url_resolved)" }}
    }}
    $r = Invoke-RestMethod -Uri "https://$srv/json/stations/search?order=clickcount&reverse=true&hidebroken=true&limit=2000" -Headers $h -TimeoutSec 20
    $hls = @($r | Where-Object {{ $_.hls -eq 1 -or $_.url_resolved -like '*.m3u8*' }})
    if($hls.Count -gt 0){{ line 'HLS' $hls[0]; foreach($y in $hls[1..4]){{ "ALT|HLS|$(clean $y.name)|$(clean $y.url_resolved)" }} }} else {{ "NOHLS" }}
    break
  }} catch {{ "FAIL|$srv|$($_.Exception.Message)" }}
}}"#
        );
        let t = Instant::now();
        let out = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .expect("powershell");
        let text = String::from_utf8_lossy(&out.stdout);
        println!("directory fetch took {} ms", t.elapsed().as_millis());
        let mut v = Vec::new();
        for l in text.lines() {
            let f: Vec<&str> = l.split('|').collect();
            match f.first() {
                Some(&"ST") if f.len() >= 7 => v.push(Station {
                    label: f[1].into(),
                    name: f[2].trim().into(),
                    country: f[3].trim().into(),
                    codec: f[4].into(),
                    bitrate: f[5].into(),
                    url: f[6].trim().into(),
                }),
                _ => println!("  {l}"),
            }
        }
        if !out.stderr.is_empty() {
            println!("  stderr: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        v
    }

    // ---------- playback ----------

    #[derive(Debug)]
    enum Ev {
        Opened(Instant, u32),
        Failed(Instant, u32, String),
        State(Instant, u32, MediaPlaybackState),
    }

    fn play_one(s: &Station, o: &Opts) -> Result<()> {
        println!(
            "\n== {}: {} ({}) codec={} bitrate={} ==\n   {}",
            s.label, s.name, s.country, s.codec, s.bitrate, s.url
        );
        let (tx, rx) = channel::<Ev>();
        let t_create = Instant::now();
        let player = MediaPlayer::new()?;
        hook(&player, &tx)?;

        let src = MediaSource::CreateFromUri(&Uri::CreateUri(&HSTRING::from(&s.url))?)?;
        if o.display == "item" {
            let item = MediaPlaybackItem::Create(&src)?;
            let p = item.GetDisplayProperties()?;
            p.SetType(MediaPlaybackType::Music)?;
            p.MusicProperties()?.SetTitle(&HSTRING::from(&s.name))?;
            p.MusicProperties()?.SetArtist(&HSTRING::from(&s.country))?;
            item.ApplyDisplayProperties(&p)?;
            player.SetSource(&item)?;
            println!("   display set on MediaPlaybackItem");
        } else {
            if o.display == "early" {
                set_display(&player, s)?;
            }
            player.SetSource(&src)?;
        }
        let t_play = Instant::now();
        player.Play()?;
        println!("   create->Play() {} ms", t_play.duration_since(t_create).as_millis());

        // Wait for first sound, a failure, or `--wait` seconds.
        let meter = Meter::new();
        let mut first_sound: Option<u128> = None;
        let mut failed = false;
        while t_play.elapsed() < Duration::from_secs(o.wait) {
            failed |= drain(&rx, t_play);
            if failed {
                break;
            }
            if meter.peak().unwrap_or(0.0) > 0.0005 {
                first_sound = Some(t_play.elapsed().as_millis());
                break;
            }
            nap(o, 20);
        }
        match (first_sound, failed) {
            (Some(ms), _) => println!("   FIRST SOUND at +{ms} ms (session peak meter)"),
            (None, true) => println!("   failed before any sound (+{} ms)", t_play.elapsed().as_millis()),
            (None, false) => println!("   NO SOUND and no failure within {} s", o.wait),
        }
        if failed || first_sound.is_none() {
            let t = Instant::now();
            player.Close()?;
            drop(player);
            println!("   close after failure took {} ms (no hang)", t.elapsed().as_millis());
            return Ok(());
        }

        if o.display == "opened" {
            set_display(&player, s)?;
        }
        // Give SMTC a moment, then see what Windows lists.
        nap(o, 1500);
        drain(&rx, t_play);
        list_sessions("while playing");

        // Measurement window: no polling, just wait on the event channel.
        println!("   measuring {} s ...", o.secs);
        let a = Sample::now();
        let mut ws_sum = 0.0;
        let mut ws_max: f64 = 0.0;
        let mut n = 0;
        let end = Instant::now() + Duration::from_secs(o.secs);
        while Instant::now() < end {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(e) => print_ev(&e, t_play),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            let ws = Sample::now().ws_mb;
            ws_sum += ws;
            ws_max = ws_max.max(ws);
            n += 1;
        }
        let b = Sample::now();
        let still = meter.peak().unwrap_or(0.0);
        println!(
            "   playing: cpu={:.3}% of machine ({:.3}% of one core) ws avg={:.1} MB max={:.1} MB private={:.1} MB threads={} peak_now={still:.3}",
            cpu_pct(&a, &b) / ncpu() as f64,
            cpu_pct(&a, &b),
            ws_sum / n.max(1) as f64,
            ws_max,
            b.priv_mb,
            thread_count()
        );

        // Stop: pause, clear source, close, drop.
        let t_stop = Instant::now();
        player.Pause()?;
        player.SetSource(None::<&IMediaPlaybackSource>)?;
        player.Close()?;
        drop(player);
        drop(src);
        let api_ms = t_stop.elapsed().as_millis();
        let mut silent_at = None;
        let mut quiet = 0;
        while t_stop.elapsed() < Duration::from_secs(5) {
            if meter.peak().unwrap_or(0.0) <= 0.0005 {
                quiet += 1;
                if quiet == 1 {
                    silent_at = Some(t_stop.elapsed().as_millis());
                }
                if quiet >= 10 {
                    break;
                }
            } else {
                quiet = 0;
                silent_at = None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        drain(&rx, t_play);
        println!(
            "   STOP: calls took {api_ms} ms, meter silent at +{} ms after stop",
            silent_at.map(|m| m.to_string()).unwrap_or_else(|| "NEVER (5 s)".into())
        );
        std::thread::sleep(Duration::from_millis(1500));
        list_sessions("after stop");
        Ok(())
    }

    /// Sleep, dispatching window messages meanwhile when `--pump` is set.
    fn nap(o: &Opts, ms: u64) {
        use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};
        if !o.pump {
            std::thread::sleep(Duration::from_millis(ms));
            return;
        }
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            unsafe {
                let mut m = MSG::default();
                while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&m);
                    DispatchMessageW(&m);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Media session display info through the player's SMTC DisplayUpdater.
    fn set_display(player: &MediaPlayer, s: &Station) -> Result<()> {
        let smtc = player.SystemMediaTransportControls()?;
        let du = smtc.DisplayUpdater()?;
        du.SetType(MediaPlaybackType::Music)?;
        let mp = du.MusicProperties()?;
        mp.SetTitle(&HSTRING::from(&s.name))?;
        mp.SetArtist(&HSTRING::from(&s.country))?;
        du.Update()?;
        println!("   smtc enabled={} display set via DisplayUpdater", smtc.IsEnabled()?);
        Ok(())
    }

    fn hook(player: &MediaPlayer, tx: &Sender<Ev>) -> Result<()> {
        let t = tx.clone();
        player.MediaOpened(&TypedEventHandler::new(move |_, _| {
            let _ = t.send(Ev::Opened(Instant::now(), unsafe { GetCurrentThreadId() }));
            Ok(())
        }))?;
        let t = tx.clone();
        player.MediaFailed(&TypedEventHandler::new(
            move |_, args: windows::core::Ref<windows::Media::Playback::MediaPlayerFailedEventArgs>| {
                let msg = match args.ok() {
                    Ok(a) => format!(
                        "error={:?} hr={:?} msg={}",
                        a.Error().ok(),
                        a.ExtendedErrorCode().ok(),
                        a.ErrorMessage().map(|m| m.to_string()).unwrap_or_default()
                    ),
                    Err(_) => "no args".into(),
                };
                let _ = t.send(Ev::Failed(Instant::now(), unsafe { GetCurrentThreadId() }, msg));
                Ok(())
            },
        ))?;
        let t = tx.clone();
        player.PlaybackSession()?.PlaybackStateChanged(&TypedEventHandler::new(
            move |s: windows::core::Ref<windows::Media::Playback::MediaPlaybackSession>, _| {
                let st = s.ok().and_then(|s| s.PlaybackState()).unwrap_or(MediaPlaybackState::None);
                let _ = t.send(Ev::State(Instant::now(), unsafe { GetCurrentThreadId() }, st));
                Ok(())
            },
        ))?;
        Ok(())
    }

    /// Print pending events; true if a failure arrived.
    fn drain(rx: &Receiver<Ev>, t0: Instant) -> bool {
        let mut failed = false;
        while let Ok(e) = rx.try_recv() {
            failed |= matches!(e, Ev::Failed(..));
            print_ev(&e, t0);
        }
        failed
    }

    fn print_ev(e: &Ev, t0: Instant) {
        let state = |s: &MediaPlaybackState| match *s {
            MediaPlaybackState::None => "None",
            MediaPlaybackState::Opening => "Opening",
            MediaPlaybackState::Buffering => "Buffering",
            MediaPlaybackState::Playing => "Playing",
            MediaPlaybackState::Paused => "Paused",
            _ => "?",
        };
        // Times are when the event fired, relative to Play().
        let ms = |t: &Instant| t.saturating_duration_since(t0).as_millis();
        match e {
            Ev::Opened(t, tid) => println!("   +{} ms MediaOpened (thread {tid})", ms(t)),
            Ev::Failed(t, tid, m) => println!("   +{} ms MediaFailed (thread {tid}) {m}", ms(t)),
            Ev::State(t, tid, s) => println!("   +{} ms PlaybackState={} (thread {tid})", ms(t), state(s)),
        }
    }

    /// What Windows' global media session list (the flyout's and Nock's source) shows.
    fn list_sessions(when: &str) {
        let r = (|| -> Result<()> {
            let mgr = Gsmtc::RequestAsync()?.join()?;
            let cur = mgr
                .GetCurrentSession()
                .ok()
                .and_then(|c| c.SourceAppUserModelId().ok())
                .map(|h| h.to_string())
                .unwrap_or_else(|| "(none)".into());
            let list = mgr.GetSessions()?;
            println!("   GSMTC {when}: {} session(s), current = {cur}", list.Size()?);
            for s in list {
                let id = s.SourceAppUserModelId()?.to_string();
                let (title, artist) = match s.TryGetMediaPropertiesAsync().and_then(|o| o.join()) {
                    Ok(p) => (p.Title()?.to_string(), p.Artist()?.to_string()),
                    Err(e) => (format!("<{e:?}>"), String::new()),
                };
                let status = s.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()).map(|v| v.0).unwrap_or(-1);
                println!("     - app={id} title={title:?} artist={artist:?} status={status}");
            }
            Ok(())
        })();
        if let Err(e) = r {
            println!("   GSMTC {when}: error {e:?}");
        }
    }

    // ---------- per-process audio meter ----------

    struct Meter {
        mgr: Option<IAudioSessionManager2>,
        pid: u32,
    }

    impl Meter {
        fn new() -> Self {
            let mgr = unsafe {
                CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .and_then(|en| en.GetDefaultAudioEndpoint(eRender, eConsole))
                    .and_then(|d| d.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None))
                    .ok()
            };
            Meter { mgr, pid: unsafe { GetCurrentProcessId() } }
        }

        /// Highest peak over this process's sessions on the default render device.
        fn peak(&self) -> Option<f32> {
            let mgr = self.mgr.as_ref()?;
            unsafe {
                let list = mgr.GetSessionEnumerator().ok()?;
                let mut best: Option<f32> = None;
                for i in 0..list.GetCount().ok()? {
                    let Ok(c) = list.GetSession(i) else { continue };
                    let Ok(c2) = c.cast::<IAudioSessionControl2>() else { continue };
                    if c2.GetProcessId().ok() != Some(self.pid) {
                        continue;
                    }
                    if let Ok(m) = c.cast::<IAudioMeterInformation>() {
                        if let Ok(p) = m.GetPeakValue() {
                            best = Some(best.unwrap_or(0.0).max(p));
                        }
                    }
                }
                best
            }
        }
    }

    // ---------- process numbers ----------

    struct Sample {
        t: Instant,
        cpu_100ns: u64,
        ws_mb: f64,
        priv_mb: f64,
    }

    impl Sample {
        fn now() -> Self {
            let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
            let (mut c, mut e, mut k, mut u) = Default::default();
            let mut pmc = PROCESS_MEMORY_COUNTERS {
                cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
                ..Default::default()
            };
            unsafe {
                let _ = GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
                let _ = K32GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb);
            }
            Sample {
                t: Instant::now(),
                cpu_100ns: ft(k) + ft(u),
                ws_mb: pmc.WorkingSetSize as f64 / (1024.0 * 1024.0),
                priv_mb: pmc.PagefileUsage as f64 / (1024.0 * 1024.0),
            }
        }
    }

    /// CPU time as a percentage of one core over the interval.
    fn cpu_pct(a: &Sample, b: &Sample) -> f64 {
        let wall = b.t.duration_since(a.t).as_secs_f64();
        let cpu = (b.cpu_100ns - a.cpu_100ns) as f64 / 1e7;
        100.0 * cpu / wall.max(1e-9)
    }

    fn ncpu() -> usize {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
    }

    fn thread_count() -> usize {
        unsafe {
            let pid = GetCurrentProcessId();
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) else { return 0 };
            let mut te = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
            let mut n = 0;
            if Thread32First(snap, &mut te).is_ok() {
                loop {
                    if te.th32OwnerProcessID == pid {
                        n += 1;
                    }
                    if Thread32Next(snap, &mut te).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
            n
        }
    }
}
