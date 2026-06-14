use crate::net::{self, Command, Event};
use crate::proto::parse_code;
use serde_json::{json, Value};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

const DEFAULT_STUN: &str = "stun.l.google.com:19302";
const EVENT_CHANNEL: &str = "engine-event";

/// Shared state: the resolved STUN server and the live command sender (None
/// until `start` spawns the engine).
struct AppState {
    stun: SocketAddr,
    cmd_tx: Mutex<Option<Sender<Command>>>,
}

/// Map an engine `Event` to the tagged JSON the frontend listens for.
fn event_to_json(ev: &Event) -> Value {
    match ev {
        Event::Discovered(addr) => json!({ "type": "discovered", "code": addr.to_string() }),
        Event::Connected(addr) => json!({ "type": "connected", "peer": addr.to_string() }),
        Event::Incoming(s) => json!({ "type": "incoming", "text": s }),
        Event::AudioState(st) => json!({
            "type": "audioState",
            "muted": st.muted,
            "inputVol": st.input_vol,
            "outputVol": st.output_vol,
        }),
        Event::AudioUnavailable(s) => json!({ "type": "audioUnavailable", "reason": s }),
        Event::PeerLeft => json!({ "type": "peerLeft" }),
        Event::Fatal(s) => json!({ "type": "fatal", "message": s }),
    }
}

/// Spawn the engine once. Called by the frontend after it attaches its listener.
#[tauri::command]
fn start(app: AppHandle, state: State<AppState>) {
    let mut guard = state.cmd_tx.lock().unwrap();
    if guard.is_some() {
        return; // already started
    }
    let (_handle, cmd_tx, evt_rx) = net::spawn(state.stun);
    *guard = Some(cmd_tx);
    std::thread::spawn(move || {
        while let Ok(ev) = evt_rx.recv() {
            // Webview gone (reload/close race) — stop forwarding instead of spinning.
            if app.emit(EVENT_CHANNEL, event_to_json(&ev)).is_err() {
                break;
            }
        }
    });
}

fn send(state: &State<AppState>, cmd: Command) {
    if let Some(tx) = state.cmd_tx.lock().unwrap().as_ref() {
        let _ = tx.send(cmd);
    }
}

#[tauri::command]
fn submit_peer_code(code: String, state: State<AppState>) -> Result<(), String> {
    let addr = parse_code(&code).map_err(|e| e.to_string())?;
    send(&state, Command::PeerCode(addr));
    Ok(())
}

#[tauri::command]
fn send_message(text: String, state: State<AppState>) {
    send(&state, Command::Send(text));
}

#[tauri::command]
fn toggle_mute(state: State<AppState>) {
    send(&state, Command::ToggleMute);
}

#[tauri::command]
fn set_input_volume(pct: u8, state: State<AppState>) {
    send(&state, Command::SetInputVolume(pct));
}

#[tauri::command]
fn set_output_volume(pct: u8, state: State<AppState>) {
    send(&state, Command::SetOutputVolume(pct));
}

#[tauri::command]
fn quit(state: State<AppState>) {
    send(&state, Command::Quit);
}

/// Resolve a STUN host:port to its first IPv4 socket address.
/// Panics if DNS is unavailable at startup — treated as a fatal misconfiguration.
fn resolve_stun(s: &str) -> SocketAddr {
    s.to_socket_addrs()
        .ok()
        .and_then(|mut it| it.find(|a| a.is_ipv4()))
        .unwrap_or_else(|| panic!("could not resolve STUN server '{s}'"))
}

pub fn run() {
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .format_timestamp_millis()
    .format_target(false)
    .try_init();

    let stun = resolve_stun(DEFAULT_STUN);
    log::info!("stun: using server {DEFAULT_STUN} ({stun})");

    tauri::Builder::default()
        .manage(AppState {
            stun,
            cmd_tx: Mutex::new(None),
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                if let Some(state) = window.try_state::<AppState>() {
                    if let Some(tx) = state.cmd_tx.lock().unwrap().as_ref() {
                        let _ = tx.send(Command::Quit);
                    }
                }
                // Best-effort: give the worker up to 300ms to transmit a BYE.
                // Not guaranteed under load or high-latency links (same trade-off
                // the original TUI had).
                std::thread::sleep(Duration::from_millis(300));
            }
        })
        .invoke_handler(tauri::generate_handler![
            start,
            submit_peer_code,
            send_message,
            toggle_mute,
            set_input_volume,
            set_output_volume,
            quit,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::event_to_json;
    use crate::audio::AudioState;
    use crate::net::Event;

    #[test]
    fn maps_audio_state_with_camel_case_keys() {
        let v = event_to_json(&Event::AudioState(AudioState {
            muted: true,
            input_vol: 80,
            output_vol: 120,
        }));
        assert_eq!(v["type"], "audioState");
        assert_eq!(v["muted"], true);
        assert_eq!(v["inputVol"], 80);
        assert_eq!(v["outputVol"], 120);
    }

    #[test]
    fn maps_incoming_text() {
        let v = event_to_json(&Event::Incoming("hi".into()));
        assert_eq!(v["type"], "incoming");
        assert_eq!(v["text"], "hi");
    }

    #[test]
    fn maps_discovered_addr_as_string() {
        let addr = "203.0.113.5:54213".parse().unwrap();
        let v = event_to_json(&Event::Discovered(addr));
        assert_eq!(v["type"], "discovered");
        assert_eq!(v["code"], "203.0.113.5:54213");
    }
}
