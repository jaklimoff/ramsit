use crate::audio::AudioState;
use crate::audio_engine::{self, AudioEngineHandle, AudioEvent, DeviceList};
use crate::net::{self, Command, Event};
use crate::proto::parse_code;
use crate::settings::Settings;
use serde_json::{json, Value};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
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
    audio: AudioEngineHandle,
    settings: Mutex<Settings>,
    config_dir: PathBuf,
}

/// Map an engine `Event` to the tagged JSON the frontend listens for.
fn event_to_json(ev: &Event) -> Value {
    match ev {
        Event::Discovered { public, local } => json!({
            "type": "discovered",
            "code": public.to_string(),
            "localCode": local.map(|a| a.to_string()),
        }),
        Event::Connected(addr) => json!({ "type": "connected", "peer": addr.to_string() }),
        Event::Incoming(s) => json!({ "type": "incoming", "text": s }),
        Event::PeerLeft => json!({ "type": "peerLeft" }),
        Event::Fatal(s) => json!({ "type": "fatal", "message": s }),
    }
}

fn audio_state_json(st: &AudioState) -> Value {
    json!({
        "type": "audioState",
        "muted": st.muted,
        "inputVol": st.input_vol,
        "outputVol": st.output_vol,
    })
}

fn audio_event_json(ev: &AudioEvent) -> Value {
    match ev {
        AudioEvent::Levels { input, output } => {
            json!({ "type": "levels", "input": input, "output": output })
        }
        AudioEvent::State(st) => audio_state_json(st),
        AudioEvent::Unavailable(reason) => json!({ "type": "audioUnavailable", "reason": reason }),
    }
}

/// Spawn the engine once. Called by the frontend after it attaches its listener.
#[tauri::command]
fn start(app: AppHandle, state: State<AppState>) {
    let mut guard = state.cmd_tx.lock().unwrap();
    if guard.is_some() {
        return; // already started
    }
    let (_handle, cmd_tx, evt_rx) = net::spawn(state.stun, state.audio.clone());
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
fn toggle_mute(app: AppHandle, state: State<AppState>) {
    let st = state.audio.toggle_mute();
    let _ = app.emit(EVENT_CHANNEL, audio_state_json(&st));
}

#[tauri::command]
fn set_input_volume(app: AppHandle, pct: u8, state: State<AppState>) {
    let st = state.audio.set_input_volume(pct);
    let _ = app.emit(EVENT_CHANNEL, audio_state_json(&st));
}

#[tauri::command]
fn set_output_volume(app: AppHandle, pct: u8, state: State<AppState>) {
    let st = state.audio.set_output_volume(pct);
    let _ = app.emit(EVENT_CHANNEL, audio_state_json(&st));
}

#[tauri::command]
fn start_audio_test(state: State<AppState>) {
    state.audio.start_test();
}

#[tauri::command]
fn stop_audio_test(state: State<AppState>) {
    state.audio.stop_test();
}

#[tauri::command]
fn play_test_tone(on: bool, state: State<AppState>) {
    state.audio.set_tone(on);
}

#[tauri::command]
fn list_audio_devices(state: State<AppState>) -> DeviceList {
    state.audio.list_devices()
}

#[tauri::command]
fn set_input_device(name: Option<String>, state: State<AppState>) {
    {
        let mut s = state.settings.lock().unwrap();
        s.input_device = name.clone();
        if let Err(e) = s.save(&state.config_dir) {
            log::warn!("settings: save failed: {e}");
        }
    }
    state.audio.set_input_device(name);
}

#[tauri::command]
fn set_output_device(name: Option<String>, state: State<AppState>) {
    {
        let mut s = state.settings.lock().unwrap();
        s.output_device = name.clone();
        if let Err(e) = s.save(&state.config_dir) {
            log::warn!("settings: save failed: {e}");
        }
    }
    state.audio.set_output_device(name);
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
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            let app_handle = app.handle().clone();
            let config_dir = app.path().app_config_dir().expect("no app config dir");
            let settings = Settings::load(&config_dir);
            let audio = audio_engine::spawn(
                move |ev: AudioEvent| {
                    let _ = app_handle.emit(EVENT_CHANNEL, audio_event_json(&ev));
                },
                settings.input_device.clone(),
                settings.output_device.clone(),
            )
            .expect("failed to start audio engine");
            app.manage(AppState {
                stun,
                cmd_tx: Mutex::new(None),
                audio,
                settings: Mutex::new(settings),
                config_dir,
            });
            #[cfg(target_os = "macos")]
            {
                use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial};
                if let Some(window) = app.get_webview_window("main") {
                    if let Err(e) =
                        apply_vibrancy(&window, NSVisualEffectMaterial::HudWindow, None, None)
                    {
                        log::warn!("window vibrancy unavailable: {e}");
                    }
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                if let Some(state) = window.try_state::<AppState>() {
                    if let Some(tx) = state.cmd_tx.lock().unwrap().as_ref() {
                        let _ = tx.send(Command::Quit);
                    }
                    state.audio.shutdown();
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
            start_audio_test,
            stop_audio_test,
            play_test_tone,
            list_audio_devices,
            set_input_device,
            set_output_device,
            quit,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::{audio_state_json, event_to_json};
    use crate::audio::AudioState;
    use crate::net::Event;

    #[test]
    fn maps_audio_state_with_camel_case_keys() {
        let v = audio_state_json(&AudioState {
            muted: true,
            input_vol: 80,
            output_vol: 120,
        });
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
        let v = event_to_json(&Event::Discovered {
            public: "203.0.113.5:54213".parse().unwrap(),
            local: Some("192.168.1.42:54213".parse().unwrap()),
        });
        assert_eq!(v["type"], "discovered");
        assert_eq!(v["code"], "203.0.113.5:54213");
        assert_eq!(v["localCode"], "192.168.1.42:54213");
    }

    #[test]
    fn maps_discovered_without_local_as_null() {
        let v = event_to_json(&Event::Discovered {
            public: "203.0.113.5:54213".parse().unwrap(),
            local: None,
        });
        assert_eq!(v["localCode"], serde_json::Value::Null);
    }
}
