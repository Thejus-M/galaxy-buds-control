//! Ask the buds' SDP server for the RFCOMM channel behind each service UUID.
//!
//! Kept as evidence for why `examples/probe.rs` exists: against this firmware
//! the search returns one record per UUID, but `sdp_get_access_protos` fails
//! with -1 on every one of them, so no channel can be read this way. The
//! daemon probes channels instead. Re-run this if a BlueZ release changes that.
//!
//! Run: cargo run --example sdp [MAC]

use std::env;
use std::ffi::c_void;
use std::os::raw::{c_int, c_void as raw_void};
use std::ptr;

const DEFAULT_MAC: &str = "E4:EC:E8:43:CD:07";

// UUIDs the Galaxy Buds publish.
const UUID_SPP_STANDARD: [u8; 16] = [
    0x00, 0x00, 0x11, 0x01, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb,
];
const UUID_SPP_NEW: [u8; 16] = [
    0x2e, 0x73, 0xa4, 0xad, 0x33, 0x2d, 0x41, 0xfc, 0x90, 0xe2, 0x16, 0xbe, 0xf0, 0x65, 0x23, 0xf2,
];
const UUID_SMEP: [u8; 16] = [
    0xf8, 0x62, 0x06, 0x74, 0xa1, 0xed, 0x41, 0xab, 0xa8, 0xb9, 0xde, 0x9a, 0xd6, 0x55, 0x72, 0x9d,
];

const SDP_RETRY_IF_BUSY: u32 = 0x01;
const SDP_ATTR_REQ_RANGE: c_int = 1;
const ALL_ATTRS: u32 = 0x0000_ffff;
const RFCOMM_UUID: c_int = 0x0003;

#[repr(C)]
#[derive(Copy, Clone)]
struct SdpList {
    next: *mut SdpList,
    data: *mut c_void,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct UuidT {
    type_: u8,
    value: [u32; 4],
}

#[link(name = "bluetooth")]
unsafe extern "C" {
    fn sdp_connect(src: *const u8, dst: *const u8, flags: u32) -> *mut raw_void;
    fn sdp_close(session: *mut raw_void) -> c_int;
    fn sdp_uuid128_create(uuid: *mut UuidT, data: *const u8) -> *mut UuidT;
    fn sdp_uuid16_create(uuid: *mut UuidT, data: u16) -> *mut UuidT;
    fn sdp_list_append(list: *mut SdpList, d: *mut c_void) -> *mut SdpList;
    fn sdp_list_free(list: *mut SdpList, free_func: *const c_void);
    fn sdp_service_search_attr_req(
        session: *mut raw_void,
        search: *const SdpList,
        reqtype: c_int,
        attrid_list: *const SdpList,
        rsp_list: *mut *mut SdpList,
    ) -> c_int;
    fn sdp_get_access_protos(rec: *const raw_void, protos: *mut *mut SdpList) -> c_int;
    fn sdp_get_proto_port(list: *const SdpList, proto: c_int) -> c_int;
    fn sdp_record_free(rec: *mut raw_void);
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

/// What to search for: a 16-bit short UUID or a full 128-bit one.
enum Search {
    Short(u16),
    Long([u8; 16]),
}

/// Query one UUID and return the RFCOMM channels of the matching records.
fn channels_for(mac: [u8; 6], uuid: Search) -> Result<Vec<u8>, String> {
    // bdaddr_t is little-endian: reverse the human-readable order.
    let mut dst = mac;
    dst.reverse();
    let any = [0u8; 6];

    let session = unsafe { sdp_connect(any.as_ptr(), dst.as_ptr(), SDP_RETRY_IF_BUSY) };
    if session.is_null() {
        return Err("sdp_connect failed".into());
    }

    let mut svc = UuidT {
        type_: 0,
        value: [0; 4],
    };
    match uuid {
        Search::Short(u) => unsafe {
            sdp_uuid16_create(&mut svc, u);
        },
        Search::Long(u) => unsafe {
            sdp_uuid128_create(&mut svc, u.as_ptr());
        },
    }

    let search = unsafe { sdp_list_append(ptr::null_mut(), &mut svc as *mut UuidT as *mut c_void) };
    let mut range = ALL_ATTRS;
    let attrs =
        unsafe { sdp_list_append(ptr::null_mut(), &mut range as *mut u32 as *mut c_void) };

    let mut rsp: *mut SdpList = ptr::null_mut();
    let rc = unsafe {
        sdp_service_search_attr_req(session, search, SDP_ATTR_REQ_RANGE, attrs, &mut rsp)
    };

    let mut found = Vec::new();
    eprintln!("    [debug] rc={rc} rsp_null={}", rsp.is_null());
    if rc == 0 {
        let mut node = rsp;
        let mut n = 0;
        while !node.is_null() {
            n += 1;
            let rec = unsafe { (*node).data } as *mut raw_void;
            let mut protos: *mut SdpList = ptr::null_mut();
            let prc = unsafe { sdp_get_access_protos(rec, &mut protos) };
            eprintln!("    [debug] record {n}: get_access_protos={prc}");
            if prc == 0 {
                let port = unsafe { sdp_get_proto_port(protos, RFCOMM_UUID) };
                if port > 0 {
                    found.push(port as u8);
                }
                unsafe { sdp_list_free(protos, ptr::null()) };
            }
            unsafe { sdp_record_free(rec) };
            node = unsafe { (*node).next };
        }
    }

    unsafe {
        if !rsp.is_null() {
            sdp_list_free(rsp, ptr::null());
        }
        sdp_list_free(search, ptr::null());
        sdp_list_free(attrs, ptr::null());
        sdp_close(session);
    }

    if rc != 0 {
        return Err(format!("sdp_service_search_attr_req failed: rc={rc}"));
    }
    Ok(found)
}

fn main() {
    let mac = env::args().nth(1).unwrap_or_else(|| DEFAULT_MAC.to_string());
    let addr = parse_mac(&mac).expect("bad MAC");

    for (name, uuid) in [
        ("SPP 0x1101 (16-bit)", Search::Short(0x1101)),
        ("SPP standard 00001101", Search::Long(UUID_SPP_STANDARD)),
        ("SPP new 2e73a4ad", Search::Long(UUID_SPP_NEW)),
        ("SMEP f8620674", Search::Long(UUID_SMEP)),
    ] {
        match channels_for(addr, uuid) {
            Ok(ch) => println!("{name:24} -> channels {ch:?}"),
            Err(e) => println!("{name:24} -> error: {e}"),
        }
    }
}
