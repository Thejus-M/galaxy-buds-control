//! Line-delimited JSON protocol between the daemon and its clients
//! (CLI, TUI, Omarchy bar widget).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::proto::{Equalizer, HoldModes, NoiseMode, Side, TouchAction};
use crate::state::BudsState;

/// One settable value on the buds.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "key", content = "value", rename_all = "kebab-case")]
pub enum Setting {
    NoiseMode(NoiseMode),
    /// 0..=2 on Buds2.
    AmbientVolume(u8),
    Equalizer(Equalizer),
    Touch { side: Side, action: TouchAction },
    HoldModes(HoldModes),
    TouchLock(bool),
    Sidetone(bool),
    GameMode(bool),
    SpatialAudio(bool),
    /// true = call audio handled on the buds.
    CallPathOnBuds(bool),
    ExtraHighAmbient(bool),
    /// Allow noise control while only one earbud is worn.
    NoiseControlOneEarbud(bool),
}

/// A one-shot thing to do that is not a setting.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    FindStart,
    FindStop,
    /// Advance the noise control mode: off -> anc -> ambient -> off.
    CycleNoiseMode,
    /// Re-request the full state from the buds.
    Refresh,
    /// Ask BlueZ to connect the buds to this machine, audio profiles and all.
    Connect,
    /// Ask BlueZ to drop them again.
    Disconnect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum Request {
    Get,
    /// Stream a state object on every change until the client disconnects.
    Subscribe,
    Set(Setting),
    Action { name: Action },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<BudsState>,
}

impl Response {
    pub fn ok(state: Option<BudsState>) -> Self {
        Response {
            ok: true,
            error: None,
            state,
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Response {
            ok: false,
            error: Some(message.into()),
            state: None,
        }
    }
}

/// A connected client of the daemon.
pub struct Client {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Client {
    pub fn connect() -> std::io::Result<Client> {
        let stream = UnixStream::connect(paths::socket_path())?;
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Client { stream, reader })
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.stream.set_read_timeout(timeout)
    }

    /// Send one request and read the single response to it.
    pub fn request(&mut self, req: &Request) -> std::io::Result<Response> {
        self.send(req)?;
        self.next_response()?
            .ok_or_else(|| std::io::Error::other("daemon closed the connection"))
    }

    pub fn send(&mut self, req: &Request) -> std::io::Result<()> {
        let mut line = serde_json::to_string(req).map_err(std::io::Error::other)?;
        line.push('\n');
        self.stream.write_all(line.as_bytes())?;
        self.stream.flush()
    }

    /// Read the next response line, or None at end of stream.
    pub fn next_response(&mut self) -> std::io::Result<Option<Response>> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        serde_json::from_str(&line)
            .map(Some)
            .map_err(std::io::Error::other)
    }
}
