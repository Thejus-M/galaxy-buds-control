//! The daemon. It owns the single SPP link to the buds, keeps the state, and
//! serves clients over a unix socket plus a state file the bar widget watches.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use galaxy_buds_control::bluez::Bluez;
use galaxy_buds_control::ipc::{Action, Request, Response, Setting};
use galaxy_buds_control::link::{parse_mac, Link};
use galaxy_buds_control::paths::{self, Config};
use galaxy_buds_control::proto::{ids, NoiseMode, Raw, Request as Req, Side, TouchFlags};
use galaxy_buds_control::state::BudsState;

const READ_TIMEOUT: Duration = Duration::from_millis(300);
const BACKOFF_MIN: Duration = Duration::from_secs(3);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
const HEARTBEAT: Duration = Duration::from_secs(60);
/// How often to ask BlueZ whether the buds are connected while they are not.
/// A property read costs microseconds; paging the buds ourselves costs the
/// user their audio, so this is the cheap side of the trade.
const BLUEZ_POLL: Duration = Duration::from_millis(1500);

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Frame-level logging, switched on with `BUDS_DEBUG=1`.
fn debug_frames() -> bool {
    std::env::var("BUDS_DEBUG").is_ok_and(|v| v != "0")
}

/// A unit of work for the link thread, with a channel to report the outcome.
struct Job {
    kind: JobKind,
    reply: Sender<Result<(), String>>,
}

enum JobKind {
    Set(Setting),
    Act(Action),
}

struct Shared {
    state: Mutex<BudsState>,
    subscribers: Mutex<Vec<Sender<String>>>,
    jobs: Sender<Job>,
}

impl Shared {
    fn snapshot(&self) -> BudsState {
        self.state.lock().unwrap().clone()
    }

    /// Write the state file and push to every subscriber.
    fn publish(&self) {
        let state = self.snapshot();
        let Ok(mut value) = serde_json::to_value(&state) else {
            return;
        };

        // A daemon with no buds in range publishes nothing for minutes on end,
        // so readers need a liveness mark that is not the last state change.
        if let Some(object) = value.as_object_mut() {
            object.insert("heartbeat".into(), now_seconds().into());
        }
        let Ok(json) = serde_json::to_string(&value) else {
            return;
        };

        let path = paths::state_path();
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, format!("{json}\n")).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }

        let line = serde_json::to_string(&Response::ok(Some(state))).unwrap_or_default();
        let mut subs = self.subscribers.lock().unwrap();
        subs.retain(|tx| tx.send(line.clone()).is_ok());
    }
}

fn main() {
    let mut config = Config::load();
    let Some(mac) = parse_mac(&config.address) else {
        eprintln!("budsd: bad address in config: {}", config.address);
        std::process::exit(1);
    };

    if let Err(e) = std::fs::create_dir_all(paths::runtime_dir()) {
        eprintln!("budsd: cannot create runtime dir: {e}");
        std::process::exit(1);
    }

    // Refuse to start twice: a live socket means another daemon owns the link.
    let socket_path = paths::socket_path();
    if UnixStream::connect(&socket_path).is_ok() {
        eprintln!("budsd: already running at {}", socket_path.display());
        std::process::exit(1);
    }
    let _ = std::fs::remove_file(&socket_path);

    let listener = match UnixListener::bind(&socket_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("budsd: cannot bind {}: {e}", socket_path.display());
            std::process::exit(1);
        }
    };

    let (job_tx, job_rx) = channel::<Job>();
    let shared = Arc::new(Shared {
        state: Mutex::new(BudsState {
            address: config.address.clone(),
            ..BudsState::default()
        }),
        subscribers: Mutex::new(Vec::new()),
        jobs: job_tx,
    });
    shared.publish();

    {
        let shared = Arc::clone(&shared);
        thread::spawn(move || link_thread(shared, mac, &mut config, job_rx));
    }

    // Heartbeat: refresh the state file even when nothing changes, so clients
    // can tell "daemon idle, buds away" from "daemon dead".
    {
        let shared = Arc::clone(&shared);
        thread::spawn(move || loop {
            thread::sleep(HEARTBEAT);
            shared.publish();
        });
    }

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let shared = Arc::clone(&shared);
                thread::spawn(move || serve_client(shared, stream));
            }
            Err(e) => eprintln!("budsd: accept failed: {e}"),
        }
    }
}

/// Keep a link to the buds up, pump frames into the state, run queued jobs.
fn link_thread(shared: Arc<Shared>, mac: [u8; 6], config: &mut Config, jobs: Receiver<Job>) {
    let mut backoff = BACKOFF_MIN;

    // Without the gate, opening the SPP socket pages the buds and brings up a
    // bare BR/EDR link: connected to BlueZ, no audio profile, no sound. Only
    // attach once the system itself holds the device.
    let mut bluez = match Bluez::new(&config.address) {
        Ok(bluez) => Some(bluez),
        Err(e) => {
            eprintln!("budsd: no system bus ({e}); attaching without the BlueZ gate");
            None
        }
    };

    loop {
        if let Some(bluez) = bluez.as_mut()
            && !bluez.connected()
        {
            if state_connected(&shared) {
                set_connected(&shared, false);
            }
            reject_pending(&jobs, "buds not connected");
            thread::sleep(BLUEZ_POLL);
            continue;
        }

        match acquire(mac, config) {
            Ok(mut link) => {
                eprintln!("budsd: connected on RFCOMM channel {}", link.channel);
                // Short reads keep queued jobs responsive.
                let _ = link.set_read_timeout(READ_TIMEOUT);
                // try_channel already consumed the first status frame while
                // probing, so ask again and pick up the firmware version too.
                let _ = link.send(&Req(ids::EXTENDED_STATUS_UPDATED));
                let _ = link.send(&Req(ids::VERSION_INFO));
                backoff = BACKOFF_MIN;
                set_connected(&shared, true);

                let reason = pump(&shared, &mut link, &jobs);
                eprintln!("budsd: link lost: {reason}");
                set_connected(&shared, false);
            }
            Err(e) => {
                eprintln!("budsd: cannot connect: {e}");
                reject_pending(&jobs, "buds not connected");
                thread::sleep(backoff);
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
}

fn state_connected(shared: &Arc<Shared>) -> bool {
    shared.state.lock().unwrap().connected
}

fn set_connected(shared: &Arc<Shared>, connected: bool) {
    {
        let mut state = shared.state.lock().unwrap();
        state.connected = connected;
    }
    shared.publish();
}

/// Answer jobs that arrived while the buds were away, rather than leaving the
/// client waiting for a link that is not coming.
fn reject_pending(jobs: &Receiver<Job>, reason: &str) {
    while let Ok(job) = jobs.try_recv() {
        let _ = job.reply.send(Err(reason.to_string()));
    }
}

/// Open the status channel, preferring the cached one and only scanning when
/// the buds are reachable but that channel is wrong.
fn acquire(mac: [u8; 6], config: &mut Config) -> std::io::Result<Link> {
    let preferred = config.channel.unwrap_or(galaxy_buds_control::link::DEFAULT_CHANNEL);

    match Link::try_channel(mac, preferred) {
        Ok(Some(link)) => {
            if config.channel != Some(link.channel) {
                config.channel = Some(link.channel);
                let _ = config.save();
            }
            return Ok(link);
        }
        Ok(None) => {}
        Err(e) if e.raw_os_error() == Some(libc::ECONNREFUSED) => {}
        // Host down or paging timeout: the buds are off, do not scan 30 channels.
        Err(e) => return Err(e),
    }

    let link = Link::open(mac, config.channel)?;
    config.channel = Some(link.channel);
    let _ = config.save();
    Ok(link)
}

/// Run the link until it breaks; returns why it stopped.
fn pump(shared: &Arc<Shared>, link: &mut Link, jobs: &Receiver<Job>) -> String {
    loop {
        while let Ok(job) = jobs.try_recv() {
            let snapshot = shared.snapshot();
            let result = run_job(link, &job.kind, &snapshot);
            let failed = result.as_ref().err().cloned();

            // The buds do not echo changes back to the session that made them,
            // so mirror the change locally the way the phone app does.
            if result.is_ok() {
                let mut state = shared.state.lock().unwrap();
                match &job.kind {
                    JobKind::Set(setting) => apply_optimistic(&mut state, setting),
                    JobKind::Act(Action::CycleNoiseMode) => {
                        state.noise_mode = state.noise_mode.next();
                    }
                    JobKind::Act(_) => {}
                }
                drop(state);
                shared.publish();
            }

            let _ = job.reply.send(result);
            if let Some(e) = failed {
                return e;
            }
        }

        match link.read_frames() {
            Ok(frames) => {
                let mut changed = false;
                for frame in frames {
                    if debug_frames() {
                        eprintln!(
                            "budsd: <- id={} payload={}",
                            frame.id,
                            frame
                                .payload
                                .iter()
                                .map(|b| format!("{b:02x}"))
                                .collect::<Vec<_>>()
                                .join(" ")
                        );
                    }
                    let mut state = shared.state.lock().unwrap();
                    if state.apply(frame.id, &frame.payload) {
                        changed = true;
                    }
                }
                if changed {
                    shared.publish();
                }
            }
            Err(e) => return e.to_string(),
        }
    }
}

/// Translate one job into SPP messages and send them.
fn run_job(link: &mut Link, kind: &JobKind, state: &BudsState) -> Result<(), String> {
    let messages: Vec<Raw> = match kind {
        JobKind::Set(setting) => encode_setting(setting, state)?,
        JobKind::Act(action) => match action {
            Action::FindStart => vec![Raw(ids::FIND_MY_EARBUDS_START, vec![])],
            Action::FindStop => vec![Raw(ids::FIND_MY_EARBUDS_STOP, vec![])],
            Action::CycleNoiseMode => {
                vec![Raw::byte(ids::NOISE_CONTROLS, state.noise_mode.next().encode())]
            }
            Action::Refresh => vec![],
            // Handled in serve_client, before a job is ever queued: these go
            // to BlueZ, and the link does not exist yet when they are asked.
            Action::Connect | Action::Disconnect => {
                return Err("connecting is BlueZ's job, not the link's".into());
            }
        },
    };

    for msg in &messages {
        if debug_frames() {
            eprintln!("budsd: -> id={} payload={:02x?}", msg.0, msg.1);
        }
        link.send(msg).map_err(|e| e.to_string())?;
    }

    // Always resync: the buds answer settings changes with a fresh full state.
    link.send(&Req(ids::EXTENDED_STATUS_UPDATED))
        .map_err(|e| e.to_string())
}

/// Reflect a setting we just sent in the local state.
fn apply_optimistic(state: &mut BudsState, setting: &Setting) {
    match setting {
        Setting::NoiseMode(mode) => state.noise_mode = *mode,
        Setting::AmbientVolume(level) => {
            state.noise_mode = NoiseMode::Ambient;
            state.ambient_volume = *level;
        }
        Setting::Equalizer(eq) => state.equalizer = *eq,
        Setting::Touch { side, action } => match side {
            Side::Left => state.touch_left = Some(*action),
            Side::Right => state.touch_right = Some(*action),
        },
        Setting::HoldModes(modes) => state.hold_modes = *modes,
        Setting::TouchLock(locked) => state.touch_flags.locked = *locked,
        Setting::Sidetone(on) => state.sidetone = *on,
        Setting::GameMode(on) => state.game_mode = *on,
        Setting::SpatialAudio(on) => state.spatial_audio = *on,
        Setting::CallPathOnBuds(on) => state.call_path_on_buds = *on,
        Setting::ExtraHighAmbient(on) => state.extra_high_ambient = *on,
        Setting::NoiseControlOneEarbud(on) => state.noise_controls_one_earbud = *on,
    }
}

fn encode_setting(setting: &Setting, state: &BudsState) -> Result<Vec<Raw>, String> {
    Ok(match setting {
        Setting::NoiseMode(mode) => vec![Raw::byte(ids::NOISE_CONTROLS, mode.encode())],
        Setting::AmbientVolume(level) => {
            if *level > 2 {
                return Err("ambient volume is 0..2 on Buds2".into());
            }
            // Ambient volume only applies in ambient mode; switch if needed.
            let mut out = Vec::new();
            if state.noise_mode != NoiseMode::Ambient {
                out.push(Raw::byte(ids::NOISE_CONTROLS, NoiseMode::Ambient.encode()));
            }
            out.push(Raw::byte(ids::AMBIENT_VOLUME, *level));
            out
        }
        Setting::Equalizer(eq) => vec![Raw::byte(ids::EQUALIZER, eq.encode())],
        Setting::Touch { side, action } => {
            // The message carries both sides, so keep the other one as it is.
            let (left, right) = match side {
                Side::Left => (
                    action.encode(Side::Left),
                    state
                        .touch_right
                        .map(|a| a.encode(Side::Right))
                        .unwrap_or(2),
                ),
                Side::Right => (
                    state.touch_left.map(|a| a.encode(Side::Left)).unwrap_or(2),
                    action.encode(Side::Right),
                ),
            };
            vec![Raw(ids::SET_TOUCHPAD_OPTION, vec![left, right])]
        }
        Setting::HoldModes(modes) => {
            if !(modes.off || modes.anc || modes.ambient) {
                return Err("at least one long-press mode must stay enabled".into());
            }
            vec![Raw::byte(
                ids::SET_TOUCH_AND_HOLD_NOISE_CONTROLS,
                modes.encode(),
            )]
        }
        Setting::TouchLock(locked) => {
            let mut flags = if state.touch_flags == TouchFlags::default() {
                TouchFlags::all_on(*locked)
            } else {
                state.touch_flags
            };
            flags.locked = *locked;
            vec![Raw::byte(ids::LOCK_TOUCHPAD, flags.encode())]
        }
        Setting::Sidetone(on) => vec![Raw::flag(ids::SET_SIDETONE, *on)],
        Setting::GameMode(on) => vec![Raw::flag(ids::GAME_MODE, *on)],
        Setting::SpatialAudio(on) => vec![Raw::flag(ids::SET_SPATIAL_AUDIO, *on)],
        // The buds use 0 for "call audio on the buds".
        Setting::CallPathOnBuds(on) => {
            vec![Raw::byte(ids::SET_CALL_PATH_CONTROL, u8::from(!*on))]
        }
        Setting::ExtraHighAmbient(on) => vec![Raw::flag(ids::EXTRA_HIGH_AMBIENT, *on)],
        Setting::NoiseControlOneEarbud(on) => {
            vec![Raw::flag(ids::SET_ANC_WITH_ONE_EARBUD, *on)]
        }
    })
}

fn serve_client(shared: Arc<Shared>, stream: UnixStream) {
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    let reader = BufReader::new(stream);
    let out = Arc::new(Mutex::new(write_half));

    for line in reader.lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Request>(&line) {
            Err(e) => Response::err(format!("bad request: {e}")),
            Ok(Request::Get) => Response::ok(Some(shared.snapshot())),
            Ok(Request::Subscribe) => {
                let (tx, rx) = channel::<String>();
                shared.subscribers.lock().unwrap().push(tx);
                let out = Arc::clone(&out);
                thread::spawn(move || {
                    for line in rx {
                        let mut sink = out.lock().unwrap();
                        if writeln!(sink, "{line}").is_err() || sink.flush().is_err() {
                            return;
                        }
                    }
                });
                Response::ok(Some(shared.snapshot()))
            }
            Ok(Request::Set(setting)) => dispatch(&shared, JobKind::Set(setting)),
            // Connect and disconnect are the system's business, not the
            // link's: they go to BlueZ so the audio profiles come with them.
            Ok(Request::Action {
                name: name @ (Action::Connect | Action::Disconnect),
            }) => bluez_action(&shared, matches!(name, Action::Connect)),
            Ok(Request::Action { name }) => dispatch(&shared, JobKind::Act(name)),
        };

        let Ok(mut text) = serde_json::to_string(&response) else {
            return;
        };
        text.push('\n');
        let mut sink = out.lock().unwrap();
        if sink.write_all(text.as_bytes()).is_err() || sink.flush().is_err() {
            return;
        }
    }
}

/// Connect or disconnect through BlueZ. An RFCOMM socket would make a bare
/// link with no sink behind it; BlueZ negotiates the profiles.
fn bluez_action(shared: &Arc<Shared>, connect: bool) -> Response {
    let address = shared.snapshot().address;
    let mut bluez = match Bluez::new(&address) {
        Ok(bluez) => bluez,
        Err(e) => return Response::err(format!("no system bus: {e}")),
    };

    let result = if connect {
        bluez.connect()
    } else {
        bluez.disconnect()
    };

    match result {
        // The link thread polls BlueZ, so give it a moment to attach before
        // answering with a state that still says disconnected.
        Ok(()) => {
            thread::sleep(Duration::from_millis(2500));
            Response::ok(Some(shared.snapshot()))
        }
        Err(e) => Response::err(e),
    }
}

/// Hand a job to the link thread and wait for its verdict.
fn dispatch(shared: &Arc<Shared>, kind: JobKind) -> Response {
    let (tx, rx) = channel();
    if shared.jobs.send(Job { kind, reply: tx }).is_err() {
        return Response::err("link thread is gone");
    }

    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => {
            // Give the buds a moment to answer with the new state.
            thread::sleep(Duration::from_millis(400));
            Response::ok(Some(shared.snapshot()))
        }
        Ok(Err(e)) => Response::err(e),
        Err(_) => Response::err("timed out waiting for the buds"),
    }
}
