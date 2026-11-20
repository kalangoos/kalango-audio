//! What the level meters show, and how they move (our own ballistics).
use crate::dsp::lin_to_db;
use crate::engine::Meter;

/// What a meter shows. Bar values are 0..1 on a dB scale (-60 dBFS .. 0 dBFS), updated in ticks of 1/60 s.
/// Ballistics (our own, in the spirit of IEC 60268-10 peak meters):
/// - peak and RMS bars rise instantly and fall at 20 dB/s;
/// - the peak-hold marker jumps to every new maximum, stays for 1.5 s, then falls at 20 dB/s;
/// - the clip light turns on when a sample reaches 0 dBFS and stays on for 2 s after the last one;
/// - `vu` is the RMS level integrated like a VU meter (99 % of a step in 300 ms), same 0..1 scale.
#[derive(Clone, Copy, Default)]
pub struct Disp { pub peak: [f32; 2], pub rms: [f32; 2], pub hold: [f32; 2], pub hold_ticks: [u32; 2], pub clip: [f32; 2], pub clip_ticks: [u32; 2], pub vu: [f32; 2], pub vu_lin: [f32; 2] }

const METER_RANGE_DB: f32 = 60.0;
const METER_TICK_S: f32 = 1.0 / 60.0;
const METER_FALL_DB_PER_S: f32 = 20.0;
const METER_HOLD_S: f32 = 1.5;
const METER_CLIP_HOLD_S: f32 = 2.0;
/// One-pole time constant that reaches 99 % of a step in 300 ms: 0.3 / ln(100).
const METER_VU_TAU_S: f32 = 0.3 / 4.605_170_2;

fn norm_db(x: f32) -> f32 { ((lin_to_db(x) + METER_RANGE_DB) / METER_RANGE_DB).clamp(0.0, 1.0) }

impl Disp {
    pub fn update(&mut self, m: &Meter, ticks: u32) {
        let fall = METER_FALL_DB_PER_S * METER_TICK_S / METER_RANGE_DB;
        let hold_ticks = (METER_HOLD_S / METER_TICK_S).round() as u32;
        let clip_ticks = (METER_CLIP_HOLD_S / METER_TICK_S).round() as u32;
        let vu_k = 1.0 - (-METER_TICK_S / METER_VU_TAU_S).exp();
        for _ in 0..ticks.max(1) {
            for c in 0..2 {
                self.peak[c] = (self.peak[c] - fall).max(norm_db(m.peak[c])).max(0.0);
                self.rms[c] = (self.rms[c] - fall).max(norm_db(m.rms[c])).max(0.0);
                if self.peak[c] >= self.hold[c] {
                    self.hold[c] = self.peak[c];
                    self.hold_ticks[c] = hold_ticks;
                } else if self.hold_ticks[c] > 0 {
                    self.hold_ticks[c] -= 1;
                } else {
                    self.hold[c] = (self.hold[c] - fall).max(self.peak[c]).max(0.0);
                }
                if m.peak[c] >= 1.0 {
                    self.clip[c] = 1.0;
                    self.clip_ticks[c] = clip_ticks;
                } else if self.clip_ticks[c] > 0 {
                    self.clip_ticks[c] -= 1;
                } else {
                    self.clip[c] = 0.0;
                }
                self.vu_lin[c] += (m.rms[c] - self.vu_lin[c]) * vu_k;
                self.vu[c] = norm_db(self.vu_lin[c]);
            }
        }
    }
}

impl Disp {
    /// The louder channel of each reading, for one-bar meters.
    pub fn peak_max(&self) -> f32 { self.peak[0].max(self.peak[1]) }
    pub fn rms_max(&self) -> f32 { self.rms[0].max(self.rms[1]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_falls_at_twenty_db_per_second() {
        let mut d = Disp::default();
        d.update(&Meter { peak: [1.0, 0.1], rms: [0.5, 0.0] }, 1);
        assert_eq!(d.peak[0], 1.0);
        let start_rms = d.rms[0];
        // one second of silence (60 ticks) takes 20 dB = 1/3 of the 60 dB scale off the bars
        d.update(&Meter::default(), 60);
        assert!((d.peak[0] - (1.0 - 1.0 / 3.0)).abs() < 1e-4, "peak {}", d.peak[0]);
        assert!((d.rms[0] - (start_rms - 1.0 / 3.0)).abs() < 1e-4, "rms {}", d.rms[0]);
        // a new louder reading jumps straight up
        d.update(&Meter { peak: [1.0, 0.0], rms: [0.0, 0.0] }, 1);
        assert_eq!(d.peak[0], 1.0);
        // and three seconds of silence empty it
        d.update(&Meter::default(), 180);
        assert_eq!(d.peak[0], 0.0);
    }

    #[test]
    fn hold_marker_stays_for_a_second_and_a_half_then_falls() {
        let mut d = Disp::default();
        d.update(&Meter { peak: [0.5, 0.0], rms: [0.2, 0.0] }, 1);
        let held = d.hold[0];
        assert!((held - (60.0 - 6.0206) / 60.0).abs() < 1e-3, "hold at {held}");
        let quiet = Meter::default();
        d.update(&quiet, 89);
        assert_eq!(d.hold[0], held, "hold stays put for 1.5 s");
        d.update(&quiet, 1 + 60);
        assert!((d.hold[0] - (held - 1.0 / 3.0)).abs() < 0.01, "then falls at 20 dB/s, at {}", d.hold[0]);
        // a higher peak takes over at once
        d.update(&Meter { peak: [1.0, 0.0], rms: [0.0, 0.0] }, 1);
        assert_eq!(d.hold[0], 1.0);
    }

    #[test]
    fn clip_lights_at_full_scale_and_holds_two_seconds() {
        let mut d = Disp::default();
        d.update(&Meter { peak: [1.0, 0.999], rms: [0.5, 0.1] }, 1);
        assert!(d.clip[0] > 0.3 && d.clip[1] == 0.0);
        d.update(&Meter::default(), 119);
        assert!(d.clip[0] > 0.3, "still lit just under 2 s");
        d.update(&Meter::default(), 2);
        assert_eq!(d.clip[0], 0.0);
    }

    #[test]
    fn vu_level_integrates_in_300_ms() {
        let mut d = Disp::default();
        let tone = Meter { peak: [0.14, 0.14], rms: [0.1, 0.1] };
        d.update(&tone, 6); // 100 ms: well under the final value
        assert!(d.vu_lin[0] < 0.08, "{}", d.vu_lin[0]);
        d.update(&tone, 12); // 300 ms in total: 99 %
        assert!((d.vu_lin[0] - 0.1).abs() < 0.0015, "{}", d.vu_lin[0]);
        assert!((d.vu[0] - 40.0 / 60.0).abs() < 0.005);
    }
}
