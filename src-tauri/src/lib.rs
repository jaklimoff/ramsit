mod bridge;
mod engine;

pub use engine::{audio, meter, net, proto, punch, tone};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    bridge::run();
}
