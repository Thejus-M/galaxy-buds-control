//! `buds` — command line and TUI client for the Galaxy Buds2 daemon.

mod tui;

use clap::{Parser, Subcommand, ValueEnum};

use galaxy_buds_control::ipc::{Action, Client, Request, Response, Setting};
use galaxy_buds_control::proto::{Equalizer, HoldModes, NoiseMode, Side, TouchAction};
use galaxy_buds_control::state::BudsState;

#[derive(Parser)]
#[command(name = "buds", about = "Control Samsung Galaxy Buds2 from Linux")]
struct Cli {
    /// Print the resulting state as JSON instead of a summary line.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Show the current state (default when no command is given is the TUI).
    Status,
    /// Live full-screen control surface.
    Tui,
    /// Noise control: off, anc, ambient, or cycle to the next mode.
    Anc { mode: AncArg },
    /// Ambient sound level, 0 to 2.
    Ambient { level: u8 },
    /// Equalizer preset.
    Eq { preset: EqArg },
    /// Touch-and-hold action for one earbud.
    Touch { side: SideArg, action: TouchArg },
    /// Which modes the long press cycles through (give one or more).
    HoldAnc { modes: Vec<AncCycleArg> },
    /// Lock or unlock the touchpads.
    TouchpadLock { on: OnOff },
    /// Sidetone (hear your own voice on calls).
    Sidetone { on: OnOff },
    /// Game mode (low latency).
    GameMode { on: OnOff },
    /// Spatial audio.
    Spatial { on: OnOff },
    /// Where call audio goes.
    CallPath { to: CallPathArg },
    /// Extra high ambient volume.
    ExtraHighAmbient { on: OnOff },
    /// Allow noise control with only one earbud in.
    OneEarbudAnc { on: OnOff },
    /// Make the buds beep so you can find them.
    Find { what: FindArg },
    /// Connect the buds to this machine through BlueZ, audio included.
    Connect,
    /// Disconnect the buds from this machine.
    Disconnect,
    /// Print a JSON state line on every change.
    Watch,
}

#[derive(Clone, Copy, ValueEnum)]
enum AncArg {
    Off,
    Anc,
    Ambient,
    Cycle,
}

#[derive(Clone, Copy, ValueEnum)]
enum AncCycleArg {
    Off,
    Anc,
    Ambient,
}

#[derive(Clone, Copy, ValueEnum)]
enum EqArg {
    Normal,
    BassBoost,
    Soft,
    Dynamic,
    Clear,
    TrebleBoost,
}

#[derive(Clone, Copy, ValueEnum)]
enum SideArg {
    Left,
    Right,
}

#[derive(Clone, Copy, ValueEnum)]
enum TouchArg {
    VoiceAssistant,
    NoiseControl,
    Volume,
    Spotify,
    Other,
}

#[derive(Clone, Copy, ValueEnum)]
enum OnOff {
    On,
    Off,
}

#[derive(Clone, Copy, ValueEnum)]
enum CallPathArg {
    Buds,
    Phone,
}

#[derive(Clone, Copy, ValueEnum)]
enum FindArg {
    Start,
    Stop,
}

impl From<OnOff> for bool {
    fn from(v: OnOff) -> bool {
        matches!(v, OnOff::On)
    }
}

impl From<EqArg> for Equalizer {
    fn from(v: EqArg) -> Equalizer {
        match v {
            EqArg::Normal => Equalizer::Normal,
            EqArg::BassBoost => Equalizer::BassBoost,
            EqArg::Soft => Equalizer::Soft,
            EqArg::Dynamic => Equalizer::Dynamic,
            EqArg::Clear => Equalizer::Clear,
            EqArg::TrebleBoost => Equalizer::TrebleBoost,
        }
    }
}

impl From<SideArg> for Side {
    fn from(v: SideArg) -> Side {
        match v {
            SideArg::Left => Side::Left,
            SideArg::Right => Side::Right,
        }
    }
}

impl From<TouchArg> for TouchAction {
    fn from(v: TouchArg) -> TouchAction {
        match v {
            TouchArg::VoiceAssistant => TouchAction::VoiceAssistant,
            TouchArg::NoiseControl => TouchAction::NoiseControl,
            TouchArg::Volume => TouchAction::Volume,
            TouchArg::Spotify => TouchAction::Spotify,
            TouchArg::Other => TouchAction::Other,
        }
    }
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command.unwrap_or(Command::Tui) {
        Command::Tui => tui::run().map_err(|e| e.to_string()).map(|_| None),
        Command::Watch => watch().map(|_| None),
        Command::Status => request(Request::Get),
        Command::Anc { mode } => match mode {
            AncArg::Cycle => request(Request::Action {
                name: Action::CycleNoiseMode,
            }),
            AncArg::Off => set(Setting::NoiseMode(NoiseMode::Off)),
            AncArg::Anc => set(Setting::NoiseMode(NoiseMode::Anc)),
            AncArg::Ambient => set(Setting::NoiseMode(NoiseMode::Ambient)),
        },
        Command::Ambient { level } => set(Setting::AmbientVolume(level)),
        Command::Eq { preset } => set(Setting::Equalizer(preset.into())),
        Command::Touch { side, action } => set(Setting::Touch {
            side: side.into(),
            action: action.into(),
        }),
        Command::HoldAnc { modes } => {
            let mut hold = HoldModes::default();
            for m in &modes {
                match m {
                    AncCycleArg::Off => hold.off = true,
                    AncCycleArg::Anc => hold.anc = true,
                    AncCycleArg::Ambient => hold.ambient = true,
                }
            }
            set(Setting::HoldModes(hold))
        }
        Command::TouchpadLock { on } => set(Setting::TouchLock(on.into())),
        Command::Sidetone { on } => set(Setting::Sidetone(on.into())),
        Command::GameMode { on } => set(Setting::GameMode(on.into())),
        Command::Spatial { on } => set(Setting::SpatialAudio(on.into())),
        Command::CallPath { to } => {
            set(Setting::CallPathOnBuds(matches!(to, CallPathArg::Buds)))
        }
        Command::ExtraHighAmbient { on } => set(Setting::ExtraHighAmbient(on.into())),
        Command::OneEarbudAnc { on } => set(Setting::NoiseControlOneEarbud(on.into())),
        Command::Find { what } => request(Request::Action {
            name: match what {
                FindArg::Start => Action::FindStart,
                FindArg::Stop => Action::FindStop,
            },
        }),
        Command::Connect => request(Request::Action {
            name: Action::Connect,
        }),
        Command::Disconnect => request(Request::Action {
            name: Action::Disconnect,
        }),
    };

    match result {
        Ok(Some(state)) => {
            if cli.json {
                println!("{}", serde_json::to_string(&state).unwrap_or_default());
            } else {
                println!("{}", summary(&state));
            }
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("buds: {e}");
            std::process::exit(1);
        }
    }
}

fn connect() -> Result<Client, String> {
    Client::connect().map_err(|e| {
        format!(
            "cannot reach the daemon ({e}). Start it with: systemctl --user start galaxybuds"
        )
    })
}

fn request(req: Request) -> Result<Option<BudsState>, String> {
    let mut client = connect()?;
    unwrap_response(client.request(&req).map_err(|e| e.to_string())?)
}

fn set(setting: Setting) -> Result<Option<BudsState>, String> {
    request(Request::Set(setting))
}

fn unwrap_response(response: Response) -> Result<Option<BudsState>, String> {
    if response.ok {
        Ok(response.state)
    } else {
        Err(response.error.unwrap_or_else(|| "request failed".into()))
    }
}

/// Print a JSON state line on every change.
fn watch() -> Result<(), String> {
    let mut client = connect()?;
    client.send(&Request::Subscribe).map_err(|e| e.to_string())?;
    while let Some(response) = client.next_response().map_err(|e| e.to_string())? {
        if let Some(state) = response.state {
            println!("{}", serde_json::to_string(&state).unwrap_or_default());
        }
    }
    Ok(())
}

/// One-line summary for humans.
fn summary(s: &BudsState) -> String {
    if !s.connected {
        return "buds disconnected".into();
    }

    let noise = match s.noise_mode {
        NoiseMode::Off => "noise control off".to_string(),
        NoiseMode::Anc => "ANC".to_string(),
        NoiseMode::Ambient => format!("ambient {}/2", s.ambient_volume),
    };
    let touch = |a: Option<TouchAction>| match a {
        Some(TouchAction::VoiceAssistant) => "assistant",
        Some(TouchAction::NoiseControl) => "noise",
        Some(TouchAction::Volume) => "volume",
        Some(TouchAction::Spotify) => "spotify",
        Some(TouchAction::Other) => "other",
        None => "-",
    };

    format!(
        "L {}% R {}% case {}% | {noise} | eq {:?} | hold L:{} R:{} | {}",
        s.battery_left,
        s.battery_right,
        s.battery_case,
        s.equalizer,
        touch(s.touch_left),
        touch(s.touch_right),
        if s.touch_flags.locked {
            "touchpad locked"
        } else {
            "touchpad unlocked"
        }
    )
}
