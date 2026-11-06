//! Dump raw frames from the status channel, with the extended status payload
//! annotated byte by byte, to verify the decoder's field offsets.
//!
//! Run: cargo run --example dump

use std::time::{Duration, Instant};

use galaxy_buds_control::link::{parse_mac, Link};
use galaxy_buds_control::paths::Config;
use galaxy_buds_control::proto::ids;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load();
    let mac = parse_mac(&config.address).ok_or("bad MAC")?;
    let mut link = Link::open(mac, config.channel)?;
    println!("channel {}", link.channel);

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        for frame in link.read_frames()? {
            println!(
                "\nid={} len={} payload: {}",
                frame.id,
                frame.payload.len(),
                frame
                    .payload
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            if frame.id == ids::EXTENDED_STATUS_UPDATED {
                for (i, b) in frame.payload.iter().enumerate() {
                    println!("  [{i:2}] 0x{b:02x} {b:3}");
                }
            }
        }
    }
    Ok(())
}
