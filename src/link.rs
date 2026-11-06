//! The RFCOMM/SPP link to the buds.
//!
//! The buds publish several RFCOMM records; only one of them speaks the status
//! protocol (channel 20 on Buds2 firmware seen so far). BlueZ's SDP attribute
//! query does not hand back the protocol descriptors for these records, so the
//! channel is found by probing and then cached in the config.

use std::io::{self, Read, Write};
use std::mem;
use std::time::{Duration, Instant};

use bluetooth_serial_port_async::{BtProtocol, BtSocket};
use galaxy_buds_rs::message::{Message, Payload};
use galaxy_buds_rs::model::Model;

use crate::proto::{ids, manager_info, Request};

/// Channel used by every Buds2 unit seen so far; tried before a full scan.
pub const DEFAULT_CHANNEL: u8 = 20;
const SCAN_RANGE: std::ops::RangeInclusive<u8> = 1..=30;

#[repr(C)]
struct SockaddrRc {
    rc_family: u16,
    rc_bdaddr: [u8; 6],
    rc_channel: u8,
}

/// One decoded SPP frame.
#[derive(Debug, Clone)]
pub struct Frame {
    pub id: u8,
    pub payload: Vec<u8>,
}

pub struct Link {
    socket: BtSocket,
    buffer: Vec<u8>,
    /// Frames read before the caller took over (during channel probing).
    pending: Vec<Frame>,
    read_timeout: Duration,
    pub channel: u8,
}

pub fn parse_mac(mac: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = mac.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut out = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        out[i] = u8::from_str_radix(p, 16).ok()?;
    }
    Some(out)
}

impl Link {
    /// Connect to one RFCOMM channel without any SDP lookup.
    pub fn connect(mac: [u8; 6], channel: u8, read_timeout: Duration) -> io::Result<Link> {
        let socket = BtSocket::new(BtProtocol::RFCOMM)
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        let fd = socket.get_fd();

        let mut bdaddr = mac;
        bdaddr.reverse(); // bdaddr_t is little-endian
        let addr = SockaddrRc {
            rc_family: libc::AF_BLUETOOTH as u16,
            rc_bdaddr: bdaddr,
            rc_channel: channel,
        };

        let rc = unsafe {
            libc::connect(
                fd,
                &addr as *const SockaddrRc as *const libc::sockaddr,
                mem::size_of::<SockaddrRc>() as libc::socklen_t,
            )
        };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }

        let mut link = Link {
            socket,
            buffer: Vec::with_capacity(4096),
            pending: Vec::new(),
            read_timeout,
            channel,
        };
        link.set_read_timeout(read_timeout)?;
        Ok(link)
    }

    pub fn set_read_timeout(&mut self, timeout: Duration) -> io::Result<()> {
        self.read_timeout = timeout;
        let tv = libc::timeval {
            tv_sec: timeout.as_secs() as libc::time_t,
            tv_usec: timeout.subsec_micros() as libc::suseconds_t,
        };
        let rc = unsafe {
            libc::setsockopt(
                self.socket.get_fd(),
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                &tv as *const libc::timeval as *const libc::c_void,
                mem::size_of::<libc::timeval>() as libc::socklen_t,
            )
        };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Send the host handshake and ask for the full state.
    pub fn handshake(&mut self) -> io::Result<()> {
        self.send(&manager_info())?;
        self.send(&Request(ids::EXTENDED_STATUS_UPDATED))
    }

    pub fn send<P: Payload>(&mut self, msg: &P) -> io::Result<()> {
        self.socket.write_all(&msg.to_byte_array())
    }

    /// Wait for readable data, and notice right away when the buds drop the
    /// link: a plain timed-out read would hide the hangup until the socket
    /// eventually errors, leaving stale state behind for seconds.
    fn wait_readable(&self) -> io::Result<bool> {
        let mut fds = libc::pollfd {
            fd: self.socket.get_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = self.read_timeout.as_millis().min(i32::MAX as u128) as i32;

        let rc = unsafe { libc::poll(&mut fds, 1, millis) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                return Ok(false);
            }
            return Err(err);
        }
        if fds.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "buds dropped the link",
            ));
        }
        Ok(fds.revents & libc::POLLIN != 0)
    }

    /// Read whatever is available and return the complete frames in it.
    /// A read timeout surfaces as an empty vector, not an error.
    pub fn read_frames(&mut self) -> io::Result<Vec<Frame>> {
        if !self.pending.is_empty() {
            return Ok(std::mem::take(&mut self.pending));
        }
        if !self.wait_readable()? {
            return Ok(Vec::new());
        }

        let mut chunk = [0u8; 2048];
        match self.socket.read(&mut chunk) {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "buds closed the link",
            )),
            Ok(n) => {
                self.buffer.extend_from_slice(&chunk[..n]);
                Ok(self.drain_frames())
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
                ) =>
            {
                Ok(Vec::new())
            }
            Err(e) => Err(e),
        }
    }

    /// Pull every complete frame out of the accumulation buffer.
    fn drain_frames(&mut self) -> Vec<Frame> {
        let mut out = Vec::new();
        let mut consumed = 0;

        loop {
            let buf = &self.buffer[consumed..];
            // Skip anything before the next start-of-message byte.
            let Some(start) = buf.iter().position(|&b| b == galaxy_buds_rs::message::BOM) else {
                consumed = self.buffer.len();
                break;
            };
            let buf = &buf[start..];
            if buf.len() < 6 {
                consumed += start;
                break;
            }
            let size = (u16::from_le_bytes([buf[1], buf[2]]) & 0x03ff) as usize;
            let total = size + 4; // som + header(2) + id/payload/crc + eom
            if buf.len() < total {
                consumed += start;
                break;
            }

            let frame = &buf[..total];
            let msg = Message::new(frame.to_vec(), Model::Buds2);
            if msg.check_crc() {
                out.push(Frame {
                    id: frame[3],
                    payload: frame[4..total - 3].to_vec(),
                });
            }
            consumed += start + total;
        }

        self.buffer.drain(..consumed);
        out
    }

    /// Try one channel and report whether the buds answer the status request.
    pub fn try_channel(mac: [u8; 6], channel: u8) -> io::Result<Option<Link>> {
        let mut link = Link::connect(mac, channel, Duration::from_millis(1200))?;
        link.handshake()?;

        // Keep what the buds send during the probe: the extended status only
        // arrives once per connection, so the caller must not lose it.
        let mut seen_status = false;
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            let frames = link.read_frames()?;
            for frame in frames {
                seen_status |= frame.id == ids::EXTENDED_STATUS_UPDATED;
                link.pending.push(frame);
            }
            if seen_status {
                return Ok(Some(link));
            }
        }
        Ok(None)
    }

    /// Open the status channel, trying `preferred` first, then the default, then
    /// a scan. Returns the live link; the caller should persist `link.channel`.
    pub fn open(mac: [u8; 6], preferred: Option<u8>) -> io::Result<Link> {
        let mut tried = Vec::new();
        let mut candidates = Vec::new();
        if let Some(c) = preferred {
            candidates.push(c);
        }
        candidates.push(DEFAULT_CHANNEL);
        candidates.extend(SCAN_RANGE);

        let mut last_err = None;
        for channel in candidates {
            if tried.contains(&channel) {
                continue;
            }
            tried.push(channel);

            match Link::try_channel(mac, channel) {
                Ok(Some(link)) => return Ok(link),
                Ok(None) => {}
                Err(e) => {
                    // Refused channels are expected during a scan.
                    if e.raw_os_error() != Some(libc::ECONNREFUSED) {
                        last_err = Some(e);
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "no RFCOMM channel answered the status request",
            )
        }))
    }
}
