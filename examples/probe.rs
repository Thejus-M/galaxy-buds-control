//! Probe every RFCOMM channel on the buds and report which one speaks the
//! status protocol, so we know which SDP record to target.
//!
//! Run: cargo run --example probe [MAC]

use std::env;
use std::io::{Read, Write};
use std::mem;
use std::os::raw::c_int;

use bluetooth_serial_port_async::{BtProtocol, BtSocket};
use galaxy_buds_rs::message::{ids, Payload};

const DEFAULT_MAC: &str = "E4:EC:E8:43:CD:07";
const MANAGER_INFO: u8 = 136;

#[repr(C)]
struct SockaddrRc {
    rc_family: u16,
    rc_bdaddr: [u8; 6],
    rc_channel: u8,
}

struct Request(u8);
impl Payload for Request {
    fn get_id(&self) -> u8 {
        self.0
    }
}

struct Raw(u8, Vec<u8>);
impl Payload for Raw {
    fn get_id(&self) -> u8 {
        self.0
    }
    fn get_data(&self) -> Vec<u8> {
        self.1.clone()
    }
}

fn parse_mac(mac: &str) -> Option<[u8; 6]> {
    let mut out = [0u8; 6];
    let parts: Vec<&str> = mac.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    for (i, p) in parts.iter().enumerate() {
        out[i] = u8::from_str_radix(p, 16).ok()?;
    }
    Some(out)
}

/// Connect to one RFCOMM channel, send the handshake, and report the message
/// ids that come back.
fn probe(mac: [u8; 6], channel: u8) -> Result<Vec<u8>, String> {
    let socket = BtSocket::new(BtProtocol::RFCOMM).map_err(|e| format!("socket: {e:?}"))?;
    let fd = socket.get_fd();

    let mut bdaddr = mac;
    bdaddr.reverse();
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
        return Err(std::io::Error::last_os_error().to_string());
    }

    let timeout = libc::timeval {
        tv_sec: 2,
        tv_usec: 0,
    };
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &timeout as *const libc::timeval as *const libc::c_void,
            mem::size_of::<libc::timeval>() as libc::socklen_t,
        ) as c_int
    };

    let mut sock = socket;
    sock.write_all(&Raw(MANAGER_INFO, vec![1, 1, 34]).to_byte_array())
        .map_err(|e| e.to_string())?;
    sock.write_all(&Request(ids::EXTENDED_STATUS_UPDATED).to_byte_array())
        .map_err(|e| e.to_string())?;

    let mut ids_seen = Vec::new();
    let mut buf = [0u8; 2048];
    for _ in 0..3 {
        match sock.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let mut i = 0;
                while i < n {
                    if buf[i] != 0xfd || i + 3 >= n {
                        i += 1;
                        continue;
                    }
                    let size = (u16::from_le_bytes([buf[i + 1], buf[i + 2]]) & 0x03ff) as usize;
                    if i + size + 4 > n {
                        break;
                    }
                    ids_seen.push(buf[i + 3]);
                    i += size + 4;
                }
            }
            Err(_) => break,
        }
    }

    Ok(ids_seen)
}

fn main() {
    let mac = env::args().nth(1).unwrap_or_else(|| DEFAULT_MAC.to_string());
    let addr = parse_mac(&mac).expect("bad MAC");

    for channel in 1..=31u8 {
        match probe(addr, channel) {
            Ok(ids_seen) if ids_seen.is_empty() => println!("ch {channel:2}: open, silent"),
            Ok(ids_seen) => println!("ch {channel:2}: open, ids {ids_seen:?}"),
            Err(e) => {
                if !e.contains("Connection refused") && !e.contains("os error 111") {
                    println!("ch {channel:2}: {e}");
                }
            }
        }
    }
}
