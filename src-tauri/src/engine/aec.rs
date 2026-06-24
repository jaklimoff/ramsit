//! Acoustic echo cancellation via the pure-Rust `aec3` crate (WebRTC AEC3 port).
//! Owns the `!Send` LinearPipeline and the render-side resampling/framing. All
//! `aec3`-touching code is behind `#[cfg(feature = "aec")]`; with the feature off
//! the type is a no-op shell so call sites need no cfg.
//!
//! AEC3 frames are f32 in i16 range (±32768) — see `to_f32`/`to_i16`.
