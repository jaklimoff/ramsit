//! The long-lived audio engine actor. Owns the cpal streams on its own thread and
//! services `AudioCmd`s. Capture peaks and playback peaks are reported at ~30 Hz via
//! a UI-agnostic `AudioEvent` callback (the bridge maps it to a Tauri event). The
//! engine is the single owner of audio for both the solo self-test and live calls.

use crate::audio::{
    build_input, build_output, clamp_vol, prefers_48k, AudioState, AudioStreams, Controls,
    MonoResampler, FRAME_SAMPLES, SAMPLE_RATE,
};
use crate::meter::Meters;
use crate::proto::AUDIO_PREFIX;
use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, StreamConfig};
use serde::Serialize;
use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// UI-agnostic events the engine reports; the bridge maps these to the frontend
/// `engine-event` JSON. Keeping this Tauri-free preserves audio.rs's "no UI deps" rule.
pub enum AudioEvent {
    Levels { input: f32, output: f32 },
    State(AudioState),
    Unavailable(String),
}

/// Snapshot of available devices + the current selection, sent to the UI.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceList {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub current_input: Option<String>,
    pub current_output: Option<String>,
}

/// Commands to the engine thread. Mute/volume are NOT here — they are plain atomic
/// stores done directly on `Shared` by the handle (see `AudioEngineHandle`).
pub enum AudioCmd {
    StartTest,
    StopTest,
    Tone(bool),
    StartCall { sock: UdpSocket, peer: SocketAddr },
    EndCall,
    SetInputDevice(Option<String>),
    SetOutputDevice(Option<String>),
    ListDevices(Sender<DeviceList>),
    Shutdown,
}

/// Shared, `Send + Sync` audio state. Held by the engine thread, the realtime
/// callbacks (via the Arc fields), the capture pump, and the network worker (`play`).
pub(crate) struct Shared {
    pub controls: Arc<Controls>,
    pub meters: Arc<Meters>,
    pub jitter: Arc<Mutex<VecDeque<i16>>>,
    pub decoder: Mutex<opus::Decoder>,
    pub out_resampler: Mutex<Option<MonoResampler>>,
    pub tone_active: Arc<AtomicBool>,
    pub active: AtomicBool,
}

impl Shared {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            controls: Arc::new(Controls {
                muted: AtomicBool::new(false),
                input_vol: std::sync::atomic::AtomicU32::new(100),
                output_vol: std::sync::atomic::AtomicU32::new(100),
            }),
            meters: Arc::new(Meters::default()),
            jitter: Arc::new(Mutex::new(VecDeque::new())),
            decoder: Mutex::new(opus::Decoder::new(SAMPLE_RATE, opus::Channels::Mono)?),
            out_resampler: Mutex::new(None),
            tone_active: Arc::new(AtomicBool::new(false)),
            active: AtomicBool::new(false),
        })
    }
}

/// Per-call encoder: resamples to 48 kHz, frames, applies input gain, encodes Opus,
/// and hands the bytes to `send`. Protocol-agnostic. Lives in the capture-pump thread.
pub(crate) struct CallSink {
    enc: opus::Encoder,
    resampler: Option<MonoResampler>,
    controls: Arc<Controls>,
    buf: Vec<i16>,
    enc_buf: [u8; 4000],
    send: Box<dyn FnMut(&[u8]) + Send>,
}

impl CallSink {
    pub(crate) fn new(
        in_rate: u32,
        controls: Arc<Controls>,
        send: Box<dyn FnMut(&[u8]) + Send>,
    ) -> Result<Self> {
        let enc = opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip)
            .map_err(|e| anyhow!("encoder init: {e}"))?;
        let resampler = if in_rate != SAMPLE_RATE {
            Some(MonoResampler::new(in_rate, SAMPLE_RATE)?)
        } else {
            None
        };
        Ok(Self {
            enc,
            resampler,
            controls,
            buf: Vec::with_capacity(FRAME_SAMPLES * 4),
            enc_buf: [0u8; 4000],
            send,
        })
    }

    pub(crate) fn process(&mut self, chunk: &[i16]) {
        match self.resampler.as_mut() {
            Some(r) => self.buf.extend_from_slice(&r.process(chunk)),
            None => self.buf.extend_from_slice(chunk),
        }
        while self.buf.len() >= FRAME_SAMPLES {
            let mut frame: Vec<i16> = self.buf.drain(..FRAME_SAMPLES).collect();
            if self.controls.muted.load(Ordering::Relaxed) {
                continue;
            }
            crate::audio::apply_gain(&mut frame, self.controls.input_vol.load(Ordering::Relaxed));
            match self.enc.encode(&frame, &mut self.enc_buf) {
                Ok(n) => (self.send)(&self.enc_buf[..n]),
                Err(e) => log::warn!("audio: encode failed: {e}"),
            }
        }
    }
}

/// `Send + Sync` handle to the engine. Mute/volume act directly on the shared atomics;
/// everything else is a non-blocking command.
#[derive(Clone)]
pub struct AudioEngineHandle {
    pub(crate) cmd_tx: Sender<AudioCmd>,
    pub(crate) shared: Arc<Shared>,
}

impl AudioEngineHandle {
    pub fn start_test(&self) {
        let _ = self.cmd_tx.send(AudioCmd::StartTest);
    }
    pub fn stop_test(&self) {
        let _ = self.cmd_tx.send(AudioCmd::StopTest);
    }
    pub fn set_tone(&self, on: bool) {
        let _ = self.cmd_tx.send(AudioCmd::Tone(on));
    }
    pub fn start_call(&self, sock: UdpSocket, peer: SocketAddr) {
        let _ = self.cmd_tx.send(AudioCmd::StartCall { sock, peer });
    }
    pub fn end_call(&self) {
        let _ = self.cmd_tx.send(AudioCmd::EndCall);
    }
    pub fn shutdown(&self) {
        let _ = self.cmd_tx.send(AudioCmd::Shutdown);
    }
    pub fn set_input_device(&self, name: Option<String>) {
        let _ = self.cmd_tx.send(AudioCmd::SetInputDevice(name));
    }
    pub fn set_output_device(&self, name: Option<String>) {
        let _ = self.cmd_tx.send(AudioCmd::SetOutputDevice(name));
    }
    /// Enumerate devices on the engine thread (serializes cpal access). Blocks up to 2s.
    pub fn list_devices(&self) -> DeviceList {
        let (tx, rx) = channel();
        if self.cmd_tx.send(AudioCmd::ListDevices(tx)).is_err() {
            return DeviceList::default();
        }
        rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default()
    }

    pub fn toggle_mute(&self) -> AudioState {
        let m = !self.shared.controls.muted.load(Ordering::Relaxed);
        self.shared.controls.muted.store(m, Ordering::Relaxed);
        self.shared.controls.snapshot()
    }
    pub fn set_input_volume(&self, pct: u8) -> AudioState {
        self.shared
            .controls
            .input_vol
            .store(clamp_vol(pct as i32), Ordering::Relaxed);
        self.shared.controls.snapshot()
    }
    pub fn set_output_volume(&self, pct: u8) -> AudioState {
        self.shared
            .controls
            .output_vol
            .store(clamp_vol(pct as i32), Ordering::Relaxed);
        self.shared.controls.snapshot()
    }
    pub fn state(&self) -> AudioState {
        self.shared.controls.snapshot()
    }

    /// Decode a received Opus payload and enqueue it for playback (called by the net
    /// worker on inbound audio).
    pub fn play(&self, payload: &[u8]) {
        let mut dec = self.shared.decoder.lock().unwrap();
        let mut out = [0i16; FRAME_SAMPLES];
        match dec.decode(payload, &mut out, false) {
            Ok(n) => {
                let samples = match self.shared.out_resampler.lock().unwrap().as_mut() {
                    Some(r) => r.process(&out[..n]),
                    None => out[..n].to_vec(),
                };
                let mut jb = self.shared.jitter.lock().unwrap();
                jb.extend(samples);
                while jb.len() > FRAME_SAMPLES * 25 {
                    jb.pop_front();
                }
            }
            Err(e) => log::warn!("audio: decode failed: {e}"),
        }
    }
}

/// Spawn the engine (boots Idle) and the level-emitter thread. `on_event` is invoked
/// off the realtime path for levels/state/errors.
pub fn spawn(
    on_event: impl Fn(AudioEvent) + Send + Sync + 'static,
    input_device: Option<String>,
    output_device: Option<String>,
) -> Result<AudioEngineHandle> {
    let shared = Arc::new(Shared::new()?);
    let (cmd_tx, cmd_rx) = channel::<AudioCmd>();
    let handle = AudioEngineHandle {
        cmd_tx,
        shared: shared.clone(),
    };
    let on_event: Arc<dyn Fn(AudioEvent) + Send + Sync> = Arc::new(on_event);

    {
        let shared = shared.clone();
        let on_event = on_event.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(33));
            if shared.active.load(Ordering::Relaxed) {
                let (input, output) = shared.meters.take();
                on_event(AudioEvent::Levels { input, output });
            }
        });
    }

    {
        let shared = shared.clone();
        thread::spawn(move || engine_loop(cmd_rx, shared, on_event, input_device, output_device));
    }

    Ok(handle)
}

/// Find the named input device, or the system default if `name` is None / not found.
fn pick_input(host: &cpal::Host, name: &Option<String>) -> Result<cpal::Device> {
    if let Some(n) = name {
        if let Ok(mut devs) = host.input_devices() {
            if let Some(d) = devs.find(|d| d.to_string() == *n) {
                return Ok(d);
            }
        }
        log::warn!("audio: input device '{n}' not found; using default");
    }
    host.default_input_device()
        .ok_or_else(|| anyhow!("no default input device"))
}

/// Find the named output device, or the system default if `name` is None / not found.
fn pick_output(host: &cpal::Host, name: &Option<String>) -> Result<cpal::Device> {
    if let Some(n) = name {
        if let Ok(mut devs) = host.output_devices() {
            if let Some(d) = devs.find(|d| d.to_string() == *n) {
                return Ok(d);
            }
        }
        log::warn!("audio: output device '{n}' not found; using default");
    }
    host.default_output_device()
        .ok_or_else(|| anyhow!("no default output device"))
}

/// Enumerate all device names plus the current selection. Runs on the engine thread.
fn enumerate_devices(in_dev: &Option<String>, out_dev: &Option<String>) -> DeviceList {
    let host = cpal::default_host();
    let inputs = host
        .input_devices()
        .map(|it| it.map(|d| d.to_string()).collect())
        .unwrap_or_default();
    let outputs = host
        .output_devices()
        .map(|it| it.map(|d| d.to_string()).collect())
        .unwrap_or_default();
    DeviceList {
        inputs,
        outputs,
        current_input: in_dev.clone(),
        current_output: out_dev.clone(),
    }
}

/// Open the selected input+output streams and a capture pump. Returns the stream guard
/// and the negotiated input rate.
fn open_streams(
    shared: &Arc<Shared>,
    sink_slot: &Arc<Mutex<Option<CallSink>>>,
    in_dev: &Option<String>,
    out_dev: &Option<String>,
) -> Result<(AudioStreams, u32)> {
    let host = cpal::default_host();
    let in_dev = pick_input(&host, in_dev)?;
    let out_dev = pick_output(&host, out_dev)?;

    let in_cfg = in_dev.default_input_config()?;
    // NOTE: in cpal 0.18.1 `sample_rate` is a plain `u32`.
    let in_rate = if prefers_48k(in_dev.supported_input_configs().ok()) {
        SAMPLE_RATE
    } else {
        in_cfg.sample_rate()
    };
    let in_channels = in_cfg.channels().max(1) as usize;
    let in_sc = StreamConfig {
        channels: in_cfg.channels(),
        sample_rate: in_rate,
        buffer_size: BufferSize::Default,
    };
    let (pcm_tx, pcm_rx) = channel::<Vec<i16>>();
    let input = build_input(
        &in_dev,
        in_sc,
        in_channels,
        in_cfg.sample_format(),
        shared.meters.clone(),
        pcm_tx,
    )?;

    let out_cfg = out_dev.default_output_config()?;
    let out_rate = if prefers_48k(out_dev.supported_output_configs().ok()) {
        SAMPLE_RATE
    } else {
        out_cfg.sample_rate()
    };
    let out_channels = out_cfg.channels().max(1) as usize;
    let out_sc = StreamConfig {
        channels: out_cfg.channels(),
        sample_rate: out_rate,
        buffer_size: BufferSize::Default,
    };
    let output = build_output(
        &out_dev,
        out_sc,
        out_channels,
        out_cfg.sample_format(),
        out_rate,
        shared.jitter.clone(),
        shared.controls.clone(),
        shared.meters.clone(),
        shared.tone_active.clone(),
    )?;

    *shared.out_resampler.lock().unwrap() = if out_rate != SAMPLE_RATE {
        Some(MonoResampler::new(SAMPLE_RATE, out_rate)?)
    } else {
        None
    };

    input.play()?;
    output.play()?;

    {
        let sink_slot = sink_slot.clone();
        thread::spawn(move || {
            while let Ok(chunk) = pcm_rx.recv() {
                if let Some(sink) = sink_slot.lock().unwrap().as_mut() {
                    sink.process(&chunk);
                }
            }
        });
    }

    log::info!("audio: streams open — in {in_rate}Hz, out {out_rate}Hz");
    Ok((
        AudioStreams {
            _input: input,
            _output: output,
        },
        in_rate,
    ))
}

/// Ensure streams are open; report failure once. Returns true if open afterwards.
fn ensure_open(
    shared: &Arc<Shared>,
    sink_slot: &Arc<Mutex<Option<CallSink>>>,
    streams: &mut Option<AudioStreams>,
    in_rate: &mut u32,
    in_dev: &Option<String>,
    out_dev: &Option<String>,
    on_event: &Arc<dyn Fn(AudioEvent) + Send + Sync>,
) -> bool {
    if streams.is_some() {
        return true;
    }
    match open_streams(shared, sink_slot, in_dev, out_dev) {
        Ok((s, rate)) => {
            *streams = Some(s);
            *in_rate = rate;
            shared.active.store(true, Ordering::Relaxed);
            true
        }
        Err(e) => {
            log::warn!("audio: open failed: {e}");
            on_event(AudioEvent::Unavailable(e.to_string()));
            false
        }
    }
}

/// Build the per-call encoder sink whose `send` closure prepends the audio prefix and
/// transmits to `peer`. Factored out so device-change reopen can rebuild it.
fn build_call_sink(
    shared: &Arc<Shared>,
    in_rate: u32,
    sock: UdpSocket,
    peer: SocketAddr,
) -> Result<CallSink> {
    let prefix = AUDIO_PREFIX.to_vec();
    let mut pkt = Vec::with_capacity(prefix.len() + 400);
    let send = Box::new(move |bytes: &[u8]| {
        pkt.clear();
        pkt.extend_from_slice(&prefix);
        pkt.extend_from_slice(bytes);
        let _ = sock.send_to(&pkt, peer);
    });
    CallSink::new(in_rate, shared.controls.clone(), send)
}

/// Apply a device change by reopening the streams (Phase 2 is not seamless). No-op when
/// streams aren't open (selection then applies on the next StartTest/StartCall).
#[allow(clippy::too_many_arguments)]
fn reopen(
    shared: &Arc<Shared>,
    sink_slot: &Arc<Mutex<Option<CallSink>>>,
    streams: &mut Option<AudioStreams>,
    in_rate: &mut u32,
    in_dev: &Option<String>,
    out_dev: &Option<String>,
    call: &Option<(UdpSocket, SocketAddr)>,
    on_event: &Arc<dyn Fn(AudioEvent) + Send + Sync>,
) {
    if streams.is_none() {
        return;
    }
    // Tear down: clearing the sink stops sends; dropping streams drops pcm_tx so the
    // pump thread exits; clear jitter since the output rate may change.
    //
    // INVARIANT: the sink MUST be cleared before reopening. The old pump thread is not
    // joined, so it can briefly coexist with the new pump (spawned in open_streams) and
    // both share this `sink_slot`. Keeping the sink `None` until after ensure_open means
    // the draining old pump can only ever observe `None` — never the new sink. Do not
    // repopulate the sink before ensure_open returns.
    *sink_slot.lock().unwrap() = None;
    shared.active.store(false, Ordering::Relaxed);
    *streams = None;
    shared.jitter.lock().unwrap().clear();

    if !ensure_open(shared, sink_slot, streams, in_rate, in_dev, out_dev, on_event) {
        return; // open failed; Unavailable already emitted
    }

    // Rebuild the call sink for the new input rate if a call is active.
    if let Some((sock, peer)) = call.as_ref() {
        match sock.try_clone() {
            Ok(clone) => match build_call_sink(shared, *in_rate, clone, *peer) {
                Ok(sink) => *sink_slot.lock().unwrap() = Some(sink),
                Err(e) => on_event(AudioEvent::Unavailable(e.to_string())),
            },
            Err(e) => on_event(AudioEvent::Unavailable(format!("socket clone failed: {e}"))),
        }
    }
}

fn engine_loop(
    cmd_rx: Receiver<AudioCmd>,
    shared: Arc<Shared>,
    on_event: Arc<dyn Fn(AudioEvent) + Send + Sync>,
    mut in_dev: Option<String>,
    mut out_dev: Option<String>,
) {
    let mut streams: Option<AudioStreams> = None;
    let mut in_rate: u32 = SAMPLE_RATE;
    let mut call: Option<(UdpSocket, SocketAddr)> = None;
    let sink_slot: Arc<Mutex<Option<CallSink>>> = Arc::new(Mutex::new(None));

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            AudioCmd::StartTest => {
                ensure_open(
                    &shared, &sink_slot, &mut streams, &mut in_rate, &in_dev, &out_dev, &on_event,
                );
            }
            AudioCmd::StopTest => {
                *sink_slot.lock().unwrap() = None;
                shared.tone_active.store(false, Ordering::Relaxed);
                shared.active.store(false, Ordering::Relaxed);
                streams = None;
            }
            AudioCmd::Tone(on) => {
                shared.tone_active.store(on, Ordering::Relaxed);
            }
            AudioCmd::StartCall { sock, peer } => {
                shared.tone_active.store(false, Ordering::Relaxed);
                if !ensure_open(
                    &shared, &sink_slot, &mut streams, &mut in_rate, &in_dev, &out_dev, &on_event,
                ) {
                    continue;
                }
                call = sock.try_clone().ok().map(|s| (s, peer));
                match build_call_sink(&shared, in_rate, sock, peer) {
                    Ok(sink) => {
                        *sink_slot.lock().unwrap() = Some(sink);
                        on_event(AudioEvent::State(shared.controls.snapshot()));
                    }
                    Err(e) => on_event(AudioEvent::Unavailable(e.to_string())),
                }
            }
            AudioCmd::EndCall => {
                *sink_slot.lock().unwrap() = None;
                call = None;
            }
            AudioCmd::SetInputDevice(name) => {
                if name != in_dev {
                    in_dev = name;
                    reopen(
                        &shared, &sink_slot, &mut streams, &mut in_rate, &in_dev, &out_dev, &call,
                        &on_event,
                    );
                }
            }
            AudioCmd::SetOutputDevice(name) => {
                if name != out_dev {
                    out_dev = name;
                    reopen(
                        &shared, &sink_slot, &mut streams, &mut in_rate, &in_dev, &out_dev, &call,
                        &on_event,
                    );
                }
            }
            AudioCmd::ListDevices(reply) => {
                let _ = reply.send(enumerate_devices(&in_dev, &out_dev));
            }
            AudioCmd::Shutdown => {
                *sink_slot.lock().unwrap() = None;
                shared.active.store(false, Ordering::Relaxed);
                drop(streams.take());
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    fn test_controls() -> Arc<Controls> {
        Arc::new(Controls {
            muted: AtomicBool::new(false),
            input_vol: AtomicU32::new(100),
            output_vol: AtomicU32::new(100),
        })
    }

    #[test]
    fn callsink_emits_opus_for_a_full_frame_at_48k() {
        let got = Arc::new(Mutex::new(Vec::<usize>::new()));
        let sink_got = got.clone();
        let mut sink = CallSink::new(
            SAMPLE_RATE,
            test_controls(),
            Box::new(move |bytes: &[u8]| sink_got.lock().unwrap().push(bytes.len())),
        )
        .unwrap();
        sink.process(&vec![0i16; FRAME_SAMPLES * 2]);
        let lens = got.lock().unwrap();
        assert_eq!(lens.len(), 2, "expected two encoded packets");
        assert!(lens.iter().all(|&n| n > 0), "encoded packets must be non-empty");
    }

    #[test]
    fn callsink_sends_nothing_when_muted() {
        let controls = test_controls();
        controls.muted.store(true, Ordering::Relaxed);
        let got = Arc::new(Mutex::new(0usize));
        let sink_got = got.clone();
        let mut sink = CallSink::new(
            SAMPLE_RATE,
            controls,
            Box::new(move |_b: &[u8]| *sink_got.lock().unwrap() += 1),
        )
        .unwrap();
        sink.process(&vec![1234i16; FRAME_SAMPLES * 2]);
        assert_eq!(*got.lock().unwrap(), 0);
    }

    #[test]
    fn device_list_default_is_empty() {
        let d = DeviceList::default();
        assert!(d.inputs.is_empty() && d.outputs.is_empty());
        assert!(d.current_input.is_none() && d.current_output.is_none());
    }

    #[test]
    fn play_decodes_and_enqueues_into_jitter() {
        let mut enc =
            opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip).unwrap();
        let mut buf = [0u8; 4000];
        let n = enc.encode(&[0i16; FRAME_SAMPLES], &mut buf).unwrap();

        let shared = Arc::new(Shared::new().unwrap());
        let handle = AudioEngineHandle {
            cmd_tx: channel().0, // unused by play()
            shared: shared.clone(),
        };
        handle.play(&buf[..n]);
        assert_eq!(shared.jitter.lock().unwrap().len(), FRAME_SAMPLES);
    }
}
