//! Acoustic echo cancellation via the pure-Rust `aec3` crate (WebRTC AEC3 port).
//! Owns the `!Send` LinearPipeline and the render-side resampling/framing. All
//! `aec3`-touching code is behind `#[cfg(feature = "aec")]`; with the feature off
//! the type is a no-op shell so call sites need no cfg.
//!
//! AEC3 frames are f32 in i16 range (±32768) — see `to_f32`/`to_i16`.

#[cfg(feature = "aec")]
use crate::audio::{MonoResampler, SAMPLE_RATE};
use anyhow::Result;

#[cfg(feature = "aec")]
use aec3::nodes::audio::AudioFormat;
#[cfg(feature = "aec")]
use aec3::pipelines::linear::{self, LinearPipeline};

/// AEC3 frame length: 10 ms at 48 kHz.
#[allow(dead_code)]
pub(crate) const APM_FRAME: usize = 480;

/// Append `src` (i16) to `dst` as f32 in i16 range (±32768). AEC3 expects this
/// scaling, NOT normalized [-1.0, 1.0].
#[allow(dead_code)]
pub(crate) fn to_f32(src: &[i16], dst: &mut Vec<f32>) {
    dst.extend(src.iter().map(|&s| s as f32));
}

/// Write `src` (f32, i16-range) back to `dst` (i16), clamping. Lengths must match.
#[allow(dead_code)]
pub(crate) fn to_i16(src: &[f32], dst: &mut [i16]) {
    for (o, &s) in dst.iter_mut().zip(src.iter()) {
        *o = s.clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    }
}

/// Buffers arbitrary-length i16 input and drains exactly `APM_FRAME`-sized frames.
#[allow(dead_code)]
#[derive(Default)]
pub(crate) struct Framer {
    buf: Vec<i16>,
}

impl Framer {
    #[allow(dead_code)]
    pub(crate) fn push(&mut self, samples: &[i16]) {
        self.buf.extend_from_slice(samples);
    }
    #[allow(dead_code)]
    pub(crate) fn next_frame(&mut self) -> Option<Vec<i16>> {
        if self.buf.len() >= APM_FRAME {
            Some(self.buf.drain(..APM_FRAME).collect())
        } else {
            None
        }
    }
}

/// Owns the aec3 LinearPipeline plus the render-side resampler/framer. `!Send`
/// (the pipeline is) — construct and use only on the capture-pump thread.
pub(crate) struct Aec {
    #[cfg(feature = "aec")]
    pipeline: LinearPipeline,
    #[cfg(feature = "aec")]
    render_resampler: Option<MonoResampler>,
    #[cfg(feature = "aec")]
    render_framer: Framer,
    #[cfg(feature = "aec")]
    render_f32: Vec<f32>,
    #[cfg(feature = "aec")]
    cap_in: Vec<f32>,
    #[cfg(feature = "aec")]
    cap_out: Vec<f32>,
}

impl Aec {
    /// Build the AEC3 pipeline (echo cancellation + high-pass filter; NS/AGC2 off).
    /// `out_rate` is the output device rate; render is resampled to 48 kHz before
    /// AEC3. Err when the `aec` feature is disabled or the pipeline fails to build.
    pub(crate) fn new(out_rate: u32) -> Result<Aec> {
        #[cfg(feature = "aec")]
        {
            let fmt = AudioFormat::ten_ms(SAMPLE_RATE, 1);
            let pipeline = linear::builder(fmt, fmt)
                .enable_noise_suppression(false)
                .enable_gain_controller2(false)
                .initial_delay_ms(0)
                .build()
                .map_err(|e| anyhow::anyhow!("aec3 build: {e:?}"))?;
            Ok(Aec {
                pipeline,
                render_resampler: if out_rate != SAMPLE_RATE {
                    Some(MonoResampler::new(out_rate, SAMPLE_RATE)?)
                } else {
                    None
                },
                render_framer: Framer::default(),
                render_f32: Vec::with_capacity(APM_FRAME),
                cap_in: Vec::with_capacity(APM_FRAME),
                cap_out: vec![0.0; APM_FRAME],
            })
        }
        #[cfg(not(feature = "aec"))]
        {
            let _ = out_rate;
            Err(anyhow::anyhow!("aec feature disabled"))
        }
    }

    /// Feed the far-end reference (resample OUT_RATE→48k, frame to 480, push to AEC3).
    pub(crate) fn feed_render(&mut self, samples_out_rate: &[i16]) {
        #[cfg(feature = "aec")]
        {
            let at_48k = match self.render_resampler.as_mut() {
                Some(r) => r.process(samples_out_rate),
                None => samples_out_rate.to_vec(),
            };
            self.render_framer.push(&at_48k);
            while let Some(frame) = self.render_framer.next_frame() {
                self.render_f32.clear();
                to_f32(&frame, &mut self.render_f32);
                let _ = self.pipeline.handle_render_frame(&self.render_f32);
            }
        }
        #[cfg(not(feature = "aec"))]
        let _ = samples_out_rate;
    }

    /// Cancel echo in place on a 960-sample mic frame (split into 2×480).
    pub(crate) fn process_capture(&mut self, frame: &mut [i16]) {
        #[cfg(feature = "aec")]
        {
            for chunk in frame.chunks_mut(APM_FRAME) {
                if chunk.len() != APM_FRAME {
                    break; // 960 splits evenly into 2×480; ignore any partial tail
                }
                self.cap_in.clear();
                to_f32(chunk, &mut self.cap_in);
                if let Ok(true) = self.pipeline.process_capture_frame(&self.cap_in, &mut self.cap_out) {
                    to_i16(&self.cap_out, chunk); // warming up / Err → leave frame unchanged
                }
            }
        }
        #[cfg(not(feature = "aec"))]
        let _ = frame;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_roundtrip_is_exact_in_i16_range() {
        let src = [0i16, 16384, -16384, i16::MAX, i16::MIN];
        let mut f = Vec::new();
        to_f32(&src, &mut f);
        assert_eq!(f[1], 16384.0, "i16 maps to itself as f32 (no normalization)");
        let mut back = [0i16; 5];
        to_i16(&f, &mut back);
        assert_eq!(src, back);
    }

    #[test]
    fn to_i16_clamps_out_of_range() {
        let mut back = [0i16; 2];
        to_i16(&[40000.0, -40000.0], &mut back);
        assert_eq!(back, [i16::MAX, i16::MIN]);
    }

    #[test]
    fn framer_drains_480_and_retains_remainder() {
        let mut fr = Framer::default();
        fr.push(&vec![1i16; APM_FRAME + 10]);
        assert_eq!(fr.next_frame().expect("full frame").len(), APM_FRAME);
        assert!(fr.next_frame().is_none(), "10 leftover < APM_FRAME");
        fr.push(&vec![2i16; APM_FRAME - 10]);
        let second = fr.next_frame().expect("second frame");
        assert_eq!(second.len(), APM_FRAME);
        assert_eq!(second[0], 1, "leftover leads");
        assert_eq!(second[10], 2, "then new samples");
    }

    #[cfg(feature = "aec")]
    #[test]
    fn aec_cancels_pure_echo() {
        // Render = i16-range noise; near-end = the same (pure echo). Steady-state
        // output energy must collapse vs input (spike measured ~124 dB).
        let mut aec = Aec::new(SAMPLE_RATE).expect("aec");
        let mut st = 0x1234_5678_9abc_def0u64;
        let mut render = vec![0i16; APM_FRAME * 2]; // one 960-style frame
        let (mut in_sum, mut out_sum) = (0f64, 0f64);
        for i in 0..600 {
            for s in render.iter_mut() {
                st = st.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                *s = (((st >> 40) as i32 & 0x7fff) - 16384) as i16;
            }
            aec.feed_render(&render);
            let mut near = render.clone();
            aec.process_capture(&mut near);
            if i >= 400 {
                in_sum += render.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>();
                out_sum += near.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>();
            }
        }
        assert!(out_sum < in_sum * 0.25, "expected >6dB cancellation: in={in_sum} out={out_sum}");
    }

    #[cfg(feature = "aec")]
    #[test]
    fn capture_is_processed_every_frame() {
        // process_capture must mutate the frame regardless of any mute decision
        // (mute lives in CallSink, not Aec). Feed matched render+echo and assert
        // the frame changes — proving the canceller runs unconditionally.
        let mut aec = Aec::new(SAMPLE_RATE).unwrap();
        let mut st = 0x0fed_cba9_8765_4321u64;
        let mut render = vec![0i16; APM_FRAME * 2];
        let mut changed = false;
        for _ in 0..100 {
            for s in render.iter_mut() {
                st = st.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                *s = (((st >> 40) as i32 & 0x7fff) - 16384) as i16;
            }
            aec.feed_render(&render);
            let mut near = render.clone();
            aec.process_capture(&mut near);
            if near != render {
                changed = true;
            }
        }
        assert!(changed, "process_capture must alter the frame in place");
    }
}
