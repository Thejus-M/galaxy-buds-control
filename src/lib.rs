//! Control the Samsung Galaxy Buds2 from Linux: protocol, RFCOMM link, daemon
//! IPC and shared state.

pub mod bluez;
pub mod ipc;
pub mod link;
pub mod paths;
pub mod proto;
pub mod state;
