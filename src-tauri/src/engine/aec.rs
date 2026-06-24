//! Acoustic echo cancellation via the pure-Rust `aec3` crate (WebRTC AEC3 port).
//! Owns the `!Send` LinearPipeline and the render-side resampling/framing. All
//! `aec3`-touching code is behind `#[cfg(feature = "aec")]`; with the feature off
//! the type is a no-op shell so call sites need no cfg.
//!
//! AEC3 frames are f32 in i16 range (±32768) — see `to_f32`/`to_i16`.

/// AEC3 frame length: 10 ms at 48 kHz.
pub(crate) const APM_FRAME: usize = 480;

/// Append `src` (i16) to `dst` as f32 in i16 range (±32768). AEC3 expects this
/// scaling, NOT normalized [-1.0, 1.0].
pub(crate) fn to_f32(src: &[i16], dst: &mut Vec<f32>) {
    dst.extend(src.iter().map(|&s| s as f32));
}

/// Write `src` (f32, i16-range) back to `dst` (i16), clamping. Lengths must match.
pub(crate) fn to_i16(src: &[f32], dst: &mut [i16]) {
    for (o, &s) in dst.iter_mut().zip(src.iter()) {
        *o = s.clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    }
}

/// Buffers arbitrary-length i16 input and drains exactly `APM_FRAME`-sized frames.
#[derive(Default)]
pub(crate) struct Framer {
    buf: Vec<i16>,
}

impl Framer {
    pub(crate) fn push(&mut self, samples: &[i16]) {
        self.buf.extend_from_slice(samples);
    }
    pub(crate) fn next_frame(&mut self) -> Option<Vec<i16>> {
        if self.buf.len() >= APM_FRAME {
            Some(self.buf.drain(..APM_FRAME).collect())
        } else {
            None
        }
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
}
