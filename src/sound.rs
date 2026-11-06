//! Sounds, made here rather than loaded: nothing to go missing. One sound at a time, the siren is never cut by a chime.

use crate::engine::Cue;
use std::f32::consts::{PI, TAU};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_LOOP, SND_MEMORY, SND_NODEFAULT, SND_PURGE};

const RATE: u32 = 22050;

fn wav(samples: &[f32], volume: f32) -> Vec<u8> {
    let data_len = samples.len() as u32 * 2;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s * volume).clamp(-1.0, 1.0);
        out.extend_from_slice(&((v * 32000.0) as i16).to_le_bytes());
    }
    out
}

/// The rising and falling wail, one 6 s cycle that loops without a click (whole cycles of each voice)
pub fn siren_samples() -> Vec<f32> {
    let n = (RATE * 6) as usize;
    let (fmin, fmax) = (420.0f32, 880.0f32);
    let mut out = Vec::with_capacity(n);
    let (mut p1, mut p2) = (0f32, 0f32);
    let dt = 1.0 / RATE as f32;
    for i in 0..n {
        let t = i as f32 * dt;
        let f = fmin + (fmax - fmin) * (0.5 - 0.5 * (TAU * t / 6.0).cos());
        p1 = (p1 + TAU * f * dt) % TAU;
        p2 = (p2 + TAU * f * (3920.0 / 3900.0) * dt) % TAU;
        let raw = p1.sin() + 0.45 * (2.0 * p1).sin() + 0.2 * (3.0 * p1).sin() + 0.5 * p2.sin();
        out.push((raw * 0.9).tanh() * 0.95);
    }
    out
}

fn bell(out: &mut Vec<f32>, start_s: f32, freq: f32, decay: f32, gain: f32) {
    let start = (start_s * RATE as f32) as usize;
    let len = (RATE as f32 * decay * 5.0) as usize;
    if out.len() < start + len {
        out.resize(start + len, 0.0);
    }
    for i in 0..len {
        let t = i as f32 / RATE as f32;
        let env = (-t / decay).exp() * (t * 400.0).min(1.0);
        let s = (TAU * freq * t).sin() + 0.35 * (TAU * freq * 2.01 * t).sin() + 0.12 * (TAU * freq * 3.02 * t).sin();
        out[start + i] += s * env * gain;
    }
}

fn beep(out: &mut Vec<f32>, start_s: f32, len_s: f32, freq: f32, gain: f32) {
    let start = (start_s * RATE as f32) as usize;
    let len = (len_s * RATE as f32) as usize;
    if out.len() < start + len {
        out.resize(start + len, 0.0);
    }
    for i in 0..len {
        let t = i as f32 / RATE as f32;
        let env = (PI * i as f32 / len as f32).sin().powf(0.3);
        let s = (TAU * freq * t).sin() + 0.3 * (TAU * freq * 3.0 * t).sin();
        out[start + i] += s * env * gain;
    }
}

fn chime(notes: &[(f32, f32)], decay: f32, gain: f32) -> Vec<f32> {
    let mut out = vec![];
    for &(at, f) in notes {
        bell(&mut out, at, f, decay, gain);
    }
    out
}

struct Bank {
    siren: Vec<u8>,
    near: Vec<u8>,
    far: Vec<u8>,
    pre: Vec<u8>,
    end: Vec<u8>,
    down: Vec<u8>,
    hush: Vec<u8>,
}

static BANK: Mutex<Option<&'static Bank>> = Mutex::new(None);
static SIREN_ON: AtomicBool = AtomicBool::new(false);

/// Builds the sounds at these volumes (0-100). The old set stays in memory: Windows may still be playing it.
pub fn init(vol_home: u8, vol_far: u8) {
    let home = vol_home as f32 / 100.0;
    let far = vol_far as f32 / 100.0;
    let mut pre = vec![];
    for (i, f) in [660.0, 880.0, 1100.0].iter().enumerate() {
        beep(&mut pre, i as f32 * 0.32, 0.26, *f, 0.5);
    }
    let mut down = vec![];
    beep(&mut down, 0.0, 0.25, 330.0, 0.5);
    beep(&mut down, 0.32, 0.25, 262.0, 0.5);
    let bank = Bank {
        siren: wav(&siren_samples(), home),
        // a siren close by: louder than far, never the siren itself
        near: wav(&chime(&[(0.0, 988.0), (0.18, 988.0), (0.36, 740.0)], 0.28, 0.55), home.max(far)),
        far: wav(&chime(&[(0.0, 880.0), (0.16, 660.0)], 0.3, 0.6), far),
        pre: wav(&pre, home),
        end: wav(&chime(&[(0.0, 523.0), (0.16, 659.0), (0.32, 784.0)], 0.35, 0.5), home.max(far)),
        down: wav(&down, far.max(0.25)),
        hush: wav(&vec![0.0f32; RATE as usize / 20], 0.0),
    };
    *BANK.lock().unwrap() = Some(Box::leak(Box::new(bank)));
}

fn bank() -> Option<&'static Bank> {
    *BANK.lock().unwrap()
}

fn send(bytes: &[u8], looped: bool) -> bool {
    let mut flags = SND_MEMORY | SND_ASYNC | SND_NODEFAULT;
    if looped {
        flags |= SND_LOOP;
    }
    unsafe { PlaySoundW(PCWSTR(bytes.as_ptr() as *const u16), None, flags).as_bool() }
}

/// Opens the sound path once, silently, so the first real sound starts without its delay
pub fn warm_up() {
    if let Some(b) = bank() {
        if !send(&b.hush, false) {
            crate::paths::log("the sound path did not open at start");
        }
    }
}

pub fn play(cue: Cue) {
    let Some(b) = bank() else { return };
    let ok = match cue {
        Cue::Siren => {
            SIREN_ON.store(true, Ordering::SeqCst);
            send(&b.siren, true)
        }
        other => {
            if SIREN_ON.load(Ordering::SeqCst) {
                return;
            }
            send(
                match other {
                    Cue::Near => &b.near,
                    Cue::Far => &b.far,
                    Cue::Pre => &b.pre,
                    _ => &b.end,
                },
                false,
            )
        }
    };
    if !ok {
        crate::paths::log(&format!("sound did not start: {cue:?}"));
    }
}

/// Ends the siren; an "event ended" tone may follow it
pub fn stop_siren() {
    if SIREN_ON.swap(false, Ordering::SeqCst) {
        unsafe {
            let _ = PlaySoundW(PCWSTR::null(), None, SND_PURGE);
        }
    }
}

pub fn siren_on() -> bool {
    SIREN_ON.load(Ordering::SeqCst)
}

/// The double low tone: the program lost its sources
pub fn play_down() {
    if !SIREN_ON.load(Ordering::SeqCst) {
        if let Some(b) = bank() {
            send(&b.down, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_siren_loops_without_a_click() {
        let s = siren_samples();
        assert_eq!(s.len(), RATE as usize * 6);
        // the step across the loop point is no bigger than the steps inside the sound
        let max_step = s.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        let seam = (s[0] - s[s.len() - 1]).abs();
        assert!(seam <= max_step * 1.05, "seam {seam}, largest step {max_step}");
        assert!(s.iter().all(|v| v.abs() <= 1.0));
        let rms = (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
        assert!(rms > 0.3, "too quiet: {rms}");
    }

    #[test]
    fn the_file_format_is_what_windows_reads() {
        let w = wav(&[0.0, 1.0, -1.0], 1.0);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[8..12], b"WAVE");
        assert_eq!(w.len(), 44 + 6);
        assert_eq!(i16::from_le_bytes([w[46], w[47]]), 32000);
    }
}
