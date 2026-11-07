//! An LTC (SMPTE 12M linear timecode) decoder for audio samples.
//!
//! LTC is biphase mark: every bit starts with a level change, and a 1 has a
//! second one halfway. A frame is 80 bits, sent least significant first,
//! ending in a sync word. Edges are found with hysteresis around a running
//! mean, scaled to a running peak, so any level, polarity or DC offset
//! decodes. The half-bit length is tracked, so any standard rate (10 to 12.5
//! samples a half-bit at 48 kHz) and some varispeed decode too. Frames played
//! backwards (the sync word reversed) are ignored.

/// A decoded frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LtcFrame {
    pub hours: u32,
    pub minutes: u32,
    pub seconds: u32,
    pub frames: u32,
    /// The drop-frame flag (bit 10).
    pub drop_frame: bool,
    /// The sample (counted from the decoder's first) its first bit starts at.
    pub start: u64,
    /// Its length in samples.
    pub samples: u32,
}

/// Bits 64–79 as sent (`0011 1111 1111 1101`), the last one lowest.
const SYNC: u128 = 0x3FFD;
const FRAME_MASK: u128 = (1 << 80) - 1;
/// Below this peak (about −50 dBFS) the input counts as silence.
const MIN_PEAK: f32 = 0.003;
/// Edges come at this fraction of the peak either side of the mean.
const HYSTERESIS: f32 = 0.3;
/// Half a bit at 48 kHz: between 24 fps (12.5 samples) and 30 fps (10).
const INITIAL_HALF: f32 = 11.0;

pub struct Decoder {
    /// Samples seen.
    sample: u64,
    mean: f32,
    peak: f32,
    high: bool,
    /// Where the last edge was.
    last_edge: Option<u64>,
    /// The half-bit length, in samples.
    half: f32,
    /// Where a 1 started, after its first half.
    pending_one: Option<u64>,
    /// The last 80 bits, the newest lowest.
    window: u128,
    /// Where each of the last 80 bits started; `starts[next]` is the oldest.
    starts: [u64; 80],
    next: usize,
    /// Bits in a row since the bit clock was last lost.
    run: u32,
}

impl Default for Decoder {
    fn default() -> Self {
        Self {
            sample: 0,
            mean: 0.0,
            peak: 0.0,
            high: false,
            last_edge: None,
            half: INITIAL_HALF,
            pending_one: None,
            window: 0,
            starts: [0; 80],
            next: 0,
            run: 0,
        }
    }
}

impl Decoder {
    /// Samples seen so far: the next sample's number.
    pub fn position(&self) -> u64 {
        self.sample
    }

    /// Take one sample; a frame when it completes one.
    pub fn push(&mut self, x: f32) -> Option<LtcFrame> {
        let n = self.sample;
        self.sample += 1;
        // About 20 ms to follow a DC offset: long against a bit (under half
        // a millisecond), so the signal itself hardly moves it. The peak
        // decays over about 50 ms.
        self.mean += (x - self.mean) / 960.0;
        let level = x - self.mean;
        self.peak = (self.peak * 0.9996).max(level.abs());
        if self.peak < MIN_PEAK {
            self.lose_clock();
            self.last_edge = None;
            return None;
        }
        let threshold = self.peak * HYSTERESIS;
        let edge = if self.high {
            level < -threshold
        } else {
            level > threshold
        };
        if !edge {
            return None;
        }
        self.high = !self.high;
        let last = self.last_edge.replace(n)?;
        self.interval(last, n)
    }

    /// An edge at `n`, the one before at `last`.
    fn interval(&mut self, last: u64, n: u64) -> Option<LtcFrame> {
        let len = (n - last) as f32;
        if len < 0.5 * self.half || len > 2.6 * self.half {
            self.lose_clock();
            return None;
        }
        if len > 1.5 * self.half {
            // A whole bit: a 0.
            self.half += (len / 2.0 - self.half) * 0.1;
            if self.pending_one.take().is_some() {
                self.lose_clock();
                return None;
            }
            self.bit(last, false, n)
        } else {
            self.half += (len - self.half) * 0.1;
            match self.pending_one.take() {
                None => {
                    self.pending_one = Some(last);
                    None
                }
                Some(start) => self.bit(start, true, n),
            }
        }
    }

    fn lose_clock(&mut self) {
        self.pending_one = None;
        self.run = 0;
    }

    /// A bit that started at `start` and ended at `end`.
    fn bit(&mut self, start: u64, one: bool, end: u64) -> Option<LtcFrame> {
        self.window = ((self.window << 1) | one as u128) & FRAME_MASK;
        self.starts[self.next] = start;
        self.next = (self.next + 1) % 80;
        self.run = self.run.saturating_add(1);
        if self.run < 80 || self.window & 0xFFFF != SYNC {
            return None;
        }
        let first = self.starts[self.next];
        self.frame(first, (end - first) as u32)
    }

    fn frame(&self, start: u64, samples: u32) -> Option<LtcFrame> {
        // Bit k of the frame (k = 0 sent first).
        let bits = |k: u32, len: u32| -> u32 {
            (0..len)
                .map(|i| (((self.window >> (79 - (k + i))) & 1) as u32) << i)
                .sum()
        };
        let (fu, ft) = (bits(0, 4), bits(8, 2));
        let (su, st) = (bits(16, 4), bits(24, 3));
        let (mu, mt) = (bits(32, 4), bits(40, 3));
        let (hu, ht) = (bits(48, 4), bits(56, 2));
        if [fu, su, mu, hu].iter().any(|&u| u > 9) {
            return None;
        }
        let frame = LtcFrame {
            hours: ht * 10 + hu,
            minutes: mt * 10 + mu,
            seconds: st * 10 + su,
            frames: ft * 10 + fu,
            drop_frame: bits(10, 1) == 1,
            start,
            samples,
        };
        (frame.hours < 24 && frame.minutes < 60 && frame.seconds < 60 && frame.frames < 30)
            .then_some(frame)
    }
}

/// LTC audio for tests: `frames` frames counted from `start` (`[h, m, s,
/// f]`) at `fps` (whole frames a second) and `samples_per_frame` samples
/// each, at `amplitude` (negative inverts it).
#[cfg(test)]
pub(crate) fn encode(
    start: [u32; 4],
    fps: u32,
    drop_frame: bool,
    samples_per_frame: f64,
    frames: u32,
    amplitude: f32,
) -> Vec<f32> {
    let mut out = Vec::new();
    let mut level = amplitude;
    let first = ((start[0] * 60 + start[1]) * 60 + start[2]) * fps + start[3];
    let mut at = 0f64;
    for total in first..first + frames {
        let f = total % fps;
        let s = total / fps % 60;
        let m = total / fps / 60 % 60;
        let h = total / fps / 3600 % 24;
        let mut bits = [false; 80];
        let mut put = |at: usize, len: usize, v: u32| {
            for i in 0..len {
                bits[at + i] = (v >> i) & 1 == 1;
            }
        };
        put(0, 4, f % 10);
        put(8, 2, f / 10);
        put(10, 1, drop_frame as u32);
        put(16, 4, s % 10);
        put(24, 3, s / 10);
        put(32, 4, m % 10);
        put(40, 3, m / 10);
        put(48, 4, h % 10);
        put(56, 2, h / 10);
        put(64, 16, 0b1011_1111_1111_1100);
        let half = samples_per_frame / 160.0;
        for bit in bits {
            for second_half in [false, true] {
                // A level change starts each bit, and halfway through a 1.
                if !second_half || bit {
                    level = -level;
                }
                let end = at + half;
                let n = (end.round() as usize).saturating_sub(out.len());
                out.extend(std::iter::repeat_n(level, n));
                at = end;
            }
        }
    }
    // The next frame's first edge, which ends this one's last bit.
    out.extend(std::iter::repeat_n(
        -level,
        (samples_per_frame / 160.0) as usize,
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(samples: &[f32]) -> Vec<LtcFrame> {
        let mut d = Decoder::default();
        samples.iter().filter_map(|&x| d.push(x)).collect()
    }

    fn label(f: &LtcFrame) -> String {
        format!(
            "{:02}:{:02}:{:02}:{:02}",
            f.hours, f.minutes, f.seconds, f.frames
        )
    }

    /// Every frame but the first (whose first edge has nothing before it to
    /// time it) decodes, in order, starting where it was put.
    #[test]
    fn decodes_each_frame_at_standard_rates() {
        for (fps, samples) in [(24, 2000.0), (25, 1920.0), (30, 1600.0), (30, 1601.6)] {
            let audio = encode([10, 59, 59, 0], fps, false, samples, 2 * fps, 0.5);
            let frames = decode(&audio);
            assert_eq!(frames.len() as u32, 2 * fps - 1, "{fps} fps: {frames:?}");
            assert_eq!(label(&frames[0]), "10:59:59:01");
            assert_eq!(label(&frames[fps as usize - 1]), "11:00:00:00", "{fps} fps");
            for (i, f) in frames.iter().enumerate() {
                let want = samples * (i + 1) as f64;
                assert!(
                    (f.start as f64 - want).abs() <= 1.0,
                    "{fps} fps frame {i} starts at {}, not {want}",
                    f.start
                );
                assert!((f.samples as f64 - samples).abs() <= 2.0);
            }
        }
    }

    #[test]
    fn decodes_any_level_and_polarity() {
        for amplitude in [0.01, 0.9, -0.3] {
            let frames = decode(&encode([1, 2, 3, 4], 25, false, 1920.0, 25, amplitude));
            // The first frame too when its first sample is already an edge.
            assert!(frames.len() >= 24, "amplitude {amplitude}: {frames:?}");
            assert_eq!(label(frames.last().unwrap()), "01:02:04:03");
        }
    }

    /// An offset 20 times the signal, there from the first sample, costs a
    /// few frames while the mean settles.
    #[test]
    fn settles_on_a_dc_offset() {
        let mut audio = encode([1, 2, 3, 4], 25, false, 1920.0, 25, 0.01);
        for x in &mut audio {
            *x += 0.2;
        }
        let frames = decode(&audio);
        assert!(frames.len() >= 21, "{frames:?}");
        assert_eq!(label(frames.last().unwrap()), "01:02:04:03");
    }

    #[test]
    fn reads_the_drop_frame_flag() {
        let audio = encode([0, 0, 0, 0], 30, true, 1601.6, 5, 0.5);
        assert!(decode(&audio).iter().all(|f| f.drop_frame));
    }

    #[test]
    fn decodes_through_varispeed_and_noise() {
        for speed in [0.9, 1.1] {
            let mut audio = encode([2, 0, 0, 0], 25, false, 1920.0 / speed, 50, 0.5);
            // Deterministic noise, about −30 dB under the signal.
            let mut seed = 1u32;
            for x in &mut audio {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *x += ((seed >> 8) as f32 / (1 << 24) as f32 - 0.5) * 0.03;
            }
            let frames = decode(&audio);
            assert_eq!(frames.len(), 49, "speed {speed}: {frames:?}");
        }
    }

    /// Silence, then LTC: it locks within a frame or two of the signal.
    #[test]
    fn locks_after_silence_and_ignores_it() {
        let mut audio = vec![0.0; 48_000];
        audio.extend(encode([3, 0, 0, 0], 25, false, 1920.0, 10, 0.5));
        let frames = decode(&audio);
        assert!(frames.len() >= 8, "{frames:?}");
        assert!(frames.iter().all(|f| f.hours == 3));
    }

    #[test]
    fn ignores_ltc_played_backwards() {
        let mut audio = encode([4, 0, 0, 0], 25, false, 1920.0, 10, 0.5);
        audio.reverse();
        assert!(decode(&audio).is_empty());
    }
}
