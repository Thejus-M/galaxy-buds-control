//! The ratatui control surface: one focusable row per setting, live state from
//! the daemon's subscribe stream, writes through a worker so the UI never
//! blocks on the buds.

use std::io;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use galaxy_buds_control::ipc::{Action, Client, Request, Setting};
use galaxy_buds_control::proto::{Equalizer, HoldModes, NoiseMode, Side, TouchAction};
use galaxy_buds_control::state::BudsState;

/// One line in the control list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    NoiseControl,
    AmbientVolume,
    Equalizer,
    TouchLeft,
    TouchRight,
    HoldModes,
    TouchpadLock,
    GameMode,
    Sidetone,
    SpatialAudio,
    ExtraHighAmbient,
    CallPath,
    OneEarbudNoiseControl,
}

const ROWS: [Row; 13] = [
    Row::NoiseControl,
    Row::AmbientVolume,
    Row::Equalizer,
    Row::TouchLeft,
    Row::TouchRight,
    Row::HoldModes,
    Row::TouchpadLock,
    Row::GameMode,
    Row::Sidetone,
    Row::SpatialAudio,
    Row::ExtraHighAmbient,
    Row::CallPath,
    Row::OneEarbudNoiseControl,
];

impl Row {
    fn label(self) -> &'static str {
        match self {
            Row::NoiseControl => "Noise control",
            Row::AmbientVolume => "Ambient level",
            Row::Equalizer => "Equalizer",
            Row::TouchLeft => "Touch & hold L",
            Row::TouchRight => "Touch & hold R",
            Row::HoldModes => "Long-press cycle",
            Row::TouchpadLock => "Touchpad",
            Row::GameMode => "Game mode",
            Row::Sidetone => "Sidetone",
            Row::SpatialAudio => "Spatial audio",
            Row::ExtraHighAmbient => "Extra high ambient",
            Row::CallPath => "Call audio",
            Row::OneEarbudNoiseControl => "Noise control, one bud",
        }
    }

    /// Minimum firmware revision; rows above the buds' revision are greyed out.
    fn min_revision(self) -> u8 {
        match self {
            Row::Sidetone => 6,
            Row::CallPath => 7,
            Row::SpatialAudio => 8,
            Row::OneEarbudNoiseControl => 3,
            _ => 0,
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Row::NoiseControl => "← → cycle   o off   n anc   m ambient",
            Row::AmbientVolume => "← → or + -",
            Row::Equalizer => "← → cycle",
            Row::TouchLeft | Row::TouchRight => "← → cycle",
            Row::HoldModes => "1 off   2 anc   3 ambient",
            _ => "space toggles",
        }
    }
}

fn eq_name(eq: Equalizer) -> &'static str {
    match eq {
        Equalizer::Normal => "Normal",
        Equalizer::BassBoost => "Bass boost",
        Equalizer::Soft => "Soft",
        Equalizer::Dynamic => "Dynamic",
        Equalizer::Clear => "Clear",
        Equalizer::TrebleBoost => "Treble boost",
    }
}

fn touch_name(action: Option<TouchAction>) -> &'static str {
    match action {
        Some(TouchAction::VoiceAssistant) => "Voice assistant",
        Some(TouchAction::NoiseControl) => "Noise control",
        Some(TouchAction::Volume) => "Volume",
        Some(TouchAction::Spotify) => "Spotify",
        Some(TouchAction::Other) => "Other",
        None => "unset",
    }
}

fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

/// What the worker thread should send to the daemon.
enum Command {
    Set(Setting),
    Act(Action),
}

struct App {
    state: BudsState,
    cursor: usize,
    message: String,
    commands: Sender<Command>,
    updates: Receiver<BudsState>,
    results: Receiver<String>,
    quit: bool,
}

impl App {
    fn row(&self) -> Row {
        ROWS[self.cursor]
    }

    fn supported(&self, row: Row) -> bool {
        self.state.revision >= row.min_revision()
    }

    fn send(&mut self, command: Command) {
        let row = self.row();
        if !self.supported(row) {
            self.message = format!(
                "{} needs firmware revision {} (buds report {})",
                row.label(),
                row.min_revision(),
                self.state.revision
            );
            return;
        }
        if self.commands.send(command).is_err() {
            self.message = "daemon connection lost".into();
        }
    }

    /// Cycle the focused row forwards (`step` 1) or backwards (`step` -1).
    fn cycle(&mut self, step: i8) {
        let state = self.state.clone();
        match self.row() {
            Row::NoiseControl => {
                let modes = NoiseMode::all();
                let at = modes.iter().position(|m| *m == state.noise_mode).unwrap_or(0);
                let next = wrap(at, step, modes.len());
                self.send(Command::Set(Setting::NoiseMode(modes[next])));
            }
            Row::AmbientVolume => {
                let level = (state.ambient_volume as i8 + step).clamp(0, 2) as u8;
                self.send(Command::Set(Setting::AmbientVolume(level)));
            }
            Row::Equalizer => {
                let all = Equalizer::all();
                let at = all.iter().position(|e| *e == state.equalizer).unwrap_or(0);
                let next = wrap(at, step, all.len());
                self.send(Command::Set(Setting::Equalizer(all[next])));
            }
            Row::TouchLeft | Row::TouchRight => {
                let side = if self.row() == Row::TouchLeft {
                    Side::Left
                } else {
                    Side::Right
                };
                let current = if side == Side::Left {
                    state.touch_left
                } else {
                    state.touch_right
                };
                let all = TouchAction::all();
                let at = current
                    .and_then(|c| all.iter().position(|a| *a == c))
                    .unwrap_or(0);
                let next = wrap(at, step, all.len());
                self.send(Command::Set(Setting::Touch {
                    side,
                    action: all[next],
                }));
            }
            _ => self.toggle(),
        }
    }

    fn toggle(&mut self) {
        let state = self.state.clone();
        let setting = match self.row() {
            Row::TouchpadLock => Setting::TouchLock(!state.touch_flags.locked),
            Row::GameMode => Setting::GameMode(!state.game_mode),
            Row::Sidetone => Setting::Sidetone(!state.sidetone),
            Row::SpatialAudio => Setting::SpatialAudio(!state.spatial_audio),
            Row::ExtraHighAmbient => Setting::ExtraHighAmbient(!state.extra_high_ambient),
            Row::CallPath => Setting::CallPathOnBuds(!state.call_path_on_buds),
            Row::OneEarbudNoiseControl => {
                Setting::NoiseControlOneEarbud(!state.noise_controls_one_earbud)
            }
            Row::NoiseControl | Row::AmbientVolume | Row::Equalizer => {
                self.cycle(1);
                return;
            }
            Row::TouchLeft | Row::TouchRight | Row::HoldModes => {
                self.message = self.row().hint().into();
                return;
            }
        };
        self.send(Command::Set(setting));
    }

    /// Flip one member of the long-press cycle, keeping at least one enabled.
    fn toggle_hold(&mut self, which: u8) {
        let mut modes = self.state.hold_modes;
        match which {
            1 => modes.off = !modes.off,
            2 => modes.anc = !modes.anc,
            _ => modes.ambient = !modes.ambient,
        }
        if !(modes.off || modes.anc || modes.ambient) {
            self.message = "at least one long-press mode must stay enabled".into();
            return;
        }
        self.send(Command::Set(Setting::HoldModes(modes)));
    }

    fn on_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.quit = true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = (self.cursor + 1) % ROWS.len();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.cursor = (self.cursor + ROWS.len() - 1) % ROWS.len();
            }
            KeyCode::Right | KeyCode::Char('l') => self.cycle(1),
            KeyCode::Left | KeyCode::Char('h') => self.cycle(-1),
            KeyCode::Char('+') => {
                self.cursor = index_of(Row::AmbientVolume);
                self.cycle(1);
            }
            KeyCode::Char('-') => {
                self.cursor = index_of(Row::AmbientVolume);
                self.cycle(-1);
            }
            KeyCode::Char(' ') | KeyCode::Enter => self.toggle(),
            KeyCode::Char('o') => {
                self.cursor = index_of(Row::NoiseControl);
                self.send(Command::Set(Setting::NoiseMode(NoiseMode::Off)));
            }
            KeyCode::Char('n') => {
                self.cursor = index_of(Row::NoiseControl);
                self.send(Command::Set(Setting::NoiseMode(NoiseMode::Anc)));
            }
            KeyCode::Char('m') => {
                self.cursor = index_of(Row::NoiseControl);
                self.send(Command::Set(Setting::NoiseMode(NoiseMode::Ambient)));
            }
            KeyCode::Char('1') => self.toggle_hold(1),
            KeyCode::Char('2') => self.toggle_hold(2),
            KeyCode::Char('3') => self.toggle_hold(3),
            KeyCode::Char('f') => self.send(Command::Act(Action::FindStart)),
            KeyCode::Char('F') => self.send(Command::Act(Action::FindStop)),
            _ => {}
        }
    }

    fn drain(&mut self) {
        loop {
            match self.updates.try_recv() {
                Ok(state) => self.state = state,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.message = "daemon stopped streaming".into();
                    break;
                }
            }
        }
        while let Ok(msg) = self.results.try_recv() {
            self.message = msg;
        }
    }

    fn value(&self, row: Row) -> String {
        let s = &self.state;
        match row {
            Row::NoiseControl => match s.noise_mode {
                NoiseMode::Off => "off".into(),
                NoiseMode::Anc => "ANC".into(),
                NoiseMode::Ambient => "ambient".into(),
            },
            Row::AmbientVolume => {
                let filled = s.ambient_volume.min(2) as usize;
                format!("{}{}  {}/2", "▮".repeat(filled), "▯".repeat(2 - filled), s.ambient_volume)
            }
            Row::Equalizer => eq_name(s.equalizer).into(),
            Row::TouchLeft => touch_name(s.touch_left).into(),
            Row::TouchRight => touch_name(s.touch_right).into(),
            Row::HoldModes => hold_summary(s.hold_modes),
            Row::TouchpadLock => if s.touch_flags.locked { "locked" } else { "unlocked" }.into(),
            Row::GameMode => on_off(s.game_mode).into(),
            Row::Sidetone => on_off(s.sidetone).into(),
            Row::SpatialAudio => on_off(s.spatial_audio).into(),
            Row::ExtraHighAmbient => on_off(s.extra_high_ambient).into(),
            Row::CallPath => if s.call_path_on_buds { "on buds" } else { "on phone" }.into(),
            Row::OneEarbudNoiseControl => on_off(s.noise_controls_one_earbud).into(),
        }
    }
}

fn hold_summary(modes: HoldModes) -> String {
    let mut parts = Vec::new();
    if modes.off {
        parts.push("off");
    }
    if modes.anc {
        parts.push("ANC");
    }
    if modes.ambient {
        parts.push("ambient");
    }
    if parts.is_empty() {
        "none".into()
    } else {
        parts.join(" → ")
    }
}

fn index_of(row: Row) -> usize {
    ROWS.iter().position(|r| *r == row).unwrap_or(0)
}

fn wrap(at: usize, step: i8, len: usize) -> usize {
    let len_i = len as i32;
    (((at as i32 + step as i32) % len_i + len_i) % len_i) as usize
}

pub fn run() -> io::Result<()> {
    let mut client = Client::connect().map_err(|e| {
        io::Error::other(format!(
            "cannot reach the daemon ({e}). Start it with: systemctl --user start galaxybuds"
        ))
    })?;
    let initial = client
        .request(&Request::Get)
        .ok()
        .and_then(|r| r.state)
        .unwrap_or_default();

    let (update_tx, update_rx) = channel();
    let (command_tx, command_rx) = channel::<Command>();
    let (result_tx, result_rx) = channel();

    // Stream state changes.
    thread::spawn(move || {
        let Ok(mut sub) = Client::connect() else { return };
        if sub.send(&Request::Subscribe).is_err() {
            return;
        }
        while let Ok(Some(response)) = sub.next_response() {
            if let Some(state) = response.state {
                if update_tx.send(state).is_err() {
                    return;
                }
            }
        }
    });

    // Apply changes without blocking the UI thread.
    let worker_results = result_tx.clone();
    thread::spawn(move || {
        for command in command_rx {
            let request = match command {
                Command::Set(setting) => Request::Set(setting),
                Command::Act(action) => Request::Action { name: action },
            };
            let outcome = match Client::connect().and_then(|mut c| c.request(&request)) {
                Ok(response) if response.ok => String::new(),
                Ok(response) => response.error.unwrap_or_else(|| "request failed".into()),
                Err(e) => e.to_string(),
            };
            if worker_results.send(outcome).is_err() {
                return;
            }
        }
    });

    let mut app = App {
        state: initial,
        cursor: 0,
        message: String::new(),
        commands: command_tx,
        updates: update_rx,
        results: result_rx,
        quit: false,
    };

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    while !app.quit {
        app.drain();
        terminal.draw(|frame| draw(frame, app))?;

        if event::poll(Duration::from_millis(150))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.on_key(key);
                }
            }
        }
    }
    Ok(())
}

fn draw(frame: &mut Frame, app: &App) {
    let areas = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        Constraint::Length(3),
    ])
    .split(frame.area());

    draw_header(frame, areas[0], app);
    draw_rows(frame, areas[1], app);
    draw_footer(frame, areas[2], app);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let s = &app.state;
    let battery = |label: &str, value: u8, charging: bool| {
        let suffix = if charging { "+" } else { "" };
        format!("{label} {value}%{suffix}")
    };

    let line = if s.connected {
        Line::from(vec![
            Span::styled(
                format!("Galaxy {}  ", s.model),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "{}  rev {}  ",
                s.firmware.clone().unwrap_or_else(|| "fw ?".into()),
                s.revision
            )),
            Span::raw(format!(
                "{}  {}  {}",
                battery("L", s.battery_left, s.charging_left),
                battery("R", s.battery_right, s.charging_right),
                battery("case", s.battery_case, s.charging_case),
            )),
        ])
    } else {
        Line::from(Span::styled(
            "buds disconnected — waiting for them to come back",
            Style::default().fg(Color::Yellow),
        ))
    };

    frame.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL).title(" buds ")),
        area,
    );
}

fn draw_rows(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = Vec::with_capacity(ROWS.len());

    for (i, row) in ROWS.iter().enumerate() {
        let focused = i == app.cursor;
        let supported = app.supported(*row);

        let mut style = Style::default();
        if !supported {
            style = style.fg(Color::DarkGray);
        } else if focused {
            style = style.add_modifier(Modifier::BOLD);
        }

        let marker = if focused { "›" } else { " " };
        let value = if supported {
            app.value(*row)
        } else {
            format!("needs rev {}", row.min_revision())
        };

        let mut spans = vec![
            Span::styled(format!("{marker} {:<24}", row.label()), style),
            Span::styled(
                format!("{value:<18}"),
                if supported && focused {
                    style.fg(Color::Cyan)
                } else {
                    style
                },
            ),
        ];
        if focused && supported {
            spans.push(Span::styled(
                row.hint(),
                Style::default().fg(Color::DarkGray),
            ));
        }
        lines.push(Line::from(spans));
    }

    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" controls ")),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let hints = "j/k move   ←/→ change   space toggle   f find   q quit";
    let line = if app.message.is_empty() {
        Line::from(Span::styled(hints, Style::default().fg(Color::DarkGray)))
    } else {
        Line::from(Span::styled(
            app.message.clone(),
            Style::default().fg(Color::Yellow),
        ))
    };
    frame.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
        area,
    );
}
