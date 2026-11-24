//! Sound effects: short chiptune cues synthesized in code (pulse waves,
//! 8-bit-style arpeggios and sweeps), so no audio files or audio crates.
//!
//! [`wav`] builds a cue as an in-memory 16-bit mono WAV with the volume
//! applied to the samples; [`play`] hands it to the OS (`PlaySoundW` on
//! Windows, nothing elsewhere for now). Which cues play is the user's
//! `[settings.sounds]` (see [`Sounds::plays`]).

use crate::config::{Sounds, VOLUME_MAX};

/// Something the app can make a noise about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sound {
    /// The add-game wizard is waiting for a tag: a "ready" chirp.
    Waiting,
    /// A tag write started: a rising arpeggio.
    Writing,
    /// The tag was written (or bound): a coin jingle.
    Success,
    /// The write failed: a low descending buzz.
    Failure,
    /// A cart launched a game: a power-on sweep.
    Launch,
}

impl Sound {
    pub const ALL: [Sound; 5] = [
        Sound::Waiting,
        Sound::Writing,
        Sound::Success,
        Sound::Failure,
        Sound::Launch,
    ];

    /// The notes, in order.
    fn tones(self) -> &'static [Tone] {
        match self {
            Sound::Waiting => {
                const {
                    &[
                        Tone::note(E6, 0.045, 0.25, 0.6),
                        Tone::rest(0.02),
                        Tone::note(B6, 0.07, 0.25, 0.6).fading(),
                    ]
                }
            }
            Sound::Writing => {
                const {
                    &[
                        Tone::note(C5, 0.055, 0.5, 0.8),
                        Tone::note(E5, 0.055, 0.5, 0.8),
                        Tone::note(G5, 0.055, 0.5, 0.8),
                        Tone::note(C6, 0.09, 0.5, 0.8).fading(),
                    ]
                }
            }
            Sound::Success => {
                const {
                    &[
                        Tone::note(B5, 0.075, 0.5, 1.0),
                        Tone::note(E6, 0.4, 0.5, 1.0).fading(),
                    ]
                }
            }
            Sound::Failure => {
                const {
                    &[
                        Tone::note(185.0, 0.14, 0.125, 1.0),
                        Tone::rest(0.04),
                        Tone::slide(147.0, 92.0, 0.32, 0.125, 1.0).fading(),
                    ]
                }
            }
            Sound::Launch => {
                const {
                    &[
                        Tone::slide(220.0, 1760.0, 0.26, 0.25, 0.8),
                        Tone::rest(0.03),
                        Tone::note(A6, 0.1, 0.5, 0.8).fading(),
                    ]
                }
            }
        }
    }
}

impl Sounds {
    /// Whether `sound` should play: sounds are on, that event is on, and
    /// the volume isn't 0.
    pub fn plays(&self, sound: Sound) -> bool {
        let event = match sound {
            Sound::Waiting => self.waiting,
            Sound::Writing => self.writing,
            Sound::Success => self.success,
            Sound::Failure => self.failure,
            Sound::Launch => self.launch,
        };
        self.enabled && event && self.volume > 0
    }
}

/// Samples per second: plenty for square waves, and small buffers.
pub const SAMPLE_RATE: u32 = 22_050;

/// The loudest sample at full volume, as a fraction of full scale: square
/// waves are harsh, so even 100 % stays well below clipping.
const HEADROOM: f32 = 0.4;

// Note frequencies (Hz), equal temperament.
const C5: f32 = 523.25;
const E5: f32 = 659.26;
const G5: f32 = 783.99;
const B5: f32 = 987.77;
const C6: f32 = 1046.50;
const E6: f32 = 1318.51;
const A6: f32 = 1760.00;
const B6: f32 = 1975.53;

/// One pulse-wave note (or a rest, at 0 Hz).
#[derive(Debug, Clone, Copy)]
struct Tone {
    /// Start and end frequency (an exponential slide when they differ).
    from: f32,
    to: f32,
    secs: f32,
    /// The pulse's high fraction: 0.5 is a square, 0.125 a thin buzz.
    duty: f32,
    level: f32,
    /// Fades out over the whole note instead of only at its end.
    fade: bool,
}

impl Tone {
    const fn note(hz: f32, secs: f32, duty: f32, level: f32) -> Tone {
        Tone::slide(hz, hz, secs, duty, level)
    }

    const fn slide(from: f32, to: f32, secs: f32, duty: f32, level: f32) -> Tone {
        Tone {
            from,
            to,
            secs,
            duty,
            level,
            fade: false,
        }
    }

    const fn rest(secs: f32) -> Tone {
        Tone::note(0.0, secs, 0.5, 0.0)
    }

    const fn fading(self) -> Tone {
        Tone { fade: true, ..self }
    }
}

/// The cue as samples in -1..=1 (before volume).
fn samples(sound: Sound) -> Vec<f32> {
    // Short ramps at each note's edges, so notes don't click.
    const ATTACK: f32 = 0.003;
    const RELEASE: f32 = 0.008;
    let rate = SAMPLE_RATE as f32;
    let mut out = Vec::new();
    let mut phase = 0.0f32;
    for tone in sound.tones() {
        let n = (tone.secs * rate).round() as usize;
        for i in 0..n {
            let t = i as f32 / rate;
            if tone.from <= 0.0 {
                out.push(0.0);
                continue;
            }
            let hz = tone.from * (tone.to / tone.from).powf(t / tone.secs);
            phase = (phase + hz / rate).fract();
            let wave = if phase < tone.duty { 1.0 } else { -1.0 };
            let left = tone.secs - t;
            let mut env = (t / ATTACK).min(1.0) * (left / RELEASE).min(1.0);
            if tone.fade {
                env *= 1.0 - t / tone.secs;
            }
            out.push(wave * env * tone.level);
        }
    }
    out
}

/// The amplitude for `volume` (0 to [`VOLUME_MAX`]): squared, so the
/// steps sound roughly even.
fn gain(volume: u8) -> f32 {
    let v = f32::from(volume.min(VOLUME_MAX)) / f32::from(VOLUME_MAX);
    v * v * HEADROOM * f32::from(i16::MAX)
}

/// `sound` at `volume` (0 to 100) as a complete 16-bit mono PCM WAV file.
pub fn wav(sound: Sound, volume: u8) -> Vec<u8> {
    let gain = gain(volume);
    let pcm: Vec<i16> = samples(sound)
        .into_iter()
        .map(|s| (s * gain).round() as i16)
        .collect();
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + pcm.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // bytes per second
    out.extend_from_slice(&2u16.to_le_bytes()); // bytes per frame
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Plays `sound` at `volume` without waiting, cutting off whatever cue was
/// still playing. Does nothing where sound isn't supported yet.
pub fn play(sound: Sound, volume: u8) {
    player::play(wav(sound, volume));
}

#[cfg(windows)]
mod player {
    use std::ptr::{null, null_mut};
    use std::sync::Mutex;

    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

    /// The WAV playing now. With `SND_ASYNC | SND_MEMORY` Windows reads the
    /// buffer while it plays, so it lives here (a static is never freed)
    /// and is only replaced after the sound playing from it was stopped.
    /// The lock also keeps two threads from swapping it at once.
    static PLAYING: Mutex<Vec<u8>> = Mutex::new(Vec::new());

    pub fn play(wav: Vec<u8>) {
        let mut playing = PLAYING.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: a null sound stops the cue playing from `playing` (and
        // returns once it is stopped), so the old buffer can be freed. The
        // new buffer stays in `PLAYING` until the next call stops it.
        unsafe {
            PlaySoundW(null(), null_mut(), 0);
            *playing = wav;
            let flags = SND_MEMORY | SND_ASYNC | SND_NODEFAULT;
            if PlaySoundW(playing.as_ptr().cast(), null_mut(), flags) == 0 {
                tracing::debug!("PlaySoundW could not play a sound");
            }
        }
    }
}

#[cfg(not(windows))]
mod player {
    pub fn play(_wav: Vec<u8>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(wav: &[u8]) -> Vec<i16> {
        let (pairs, rest) = wav[44..].as_chunks::<2>();
        assert!(rest.is_empty());
        pairs.iter().map(|&b| i16::from_le_bytes(b)).collect()
    }

    fn peak(wav: &[u8]) -> i32 {
        pcm(wav)
            .into_iter()
            .map(|s| i32::from(s).abs())
            .max()
            .unwrap()
    }

    fn u32_at(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }

    fn u16_at(b: &[u8], at: usize) -> u16 {
        u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
    }

    #[test]
    fn every_cue_is_a_well_formed_16_bit_mono_wav() {
        for sound in Sound::ALL {
            let w = wav(sound, 60);
            assert_eq!(&w[0..4], b"RIFF");
            assert_eq!(u32_at(&w, 4) as usize, w.len() - 8, "{sound:?}");
            assert_eq!(&w[8..16], b"WAVEfmt ");
            assert_eq!(u32_at(&w, 16), 16);
            assert_eq!(u16_at(&w, 20), 1, "PCM");
            assert_eq!(u16_at(&w, 22), 1, "mono");
            assert_eq!(u32_at(&w, 24), SAMPLE_RATE);
            assert_eq!(u32_at(&w, 28), SAMPLE_RATE * 2);
            assert_eq!(u16_at(&w, 32), 2);
            assert_eq!(u16_at(&w, 34), 16);
            assert_eq!(&w[36..40], b"data");
            assert_eq!(u32_at(&w, 40) as usize, w.len() - 44);
        }
    }

    #[test]
    fn cues_are_short() {
        for sound in Sound::ALL {
            let secs = (wav(sound, 60).len() - 44) as f32 / 2.0 / SAMPLE_RATE as f32;
            assert!((0.1..=0.6).contains(&secs), "{sound:?}: {secs} s");
        }
    }

    #[test]
    fn volume_scales_the_amplitude_and_zero_is_silence() {
        for sound in Sound::ALL {
            assert!(pcm(&wav(sound, 0)).iter().all(|&s| s == 0), "{sound:?}");
            let (low, mid, full) = (
                peak(&wav(sound, 20)),
                peak(&wav(sound, 60)),
                peak(&wav(sound, 100)),
            );
            assert!(0 < low && low < mid && mid < full, "{sound:?}");
            // Squared: 60 % is 36 % of the full amplitude.
            let ratio = mid as f32 / full as f32;
            assert!((ratio - 0.36).abs() < 0.01, "{sound:?}: {ratio}");
            assert!(full <= (HEADROOM * f32::from(i16::MAX)).round() as i32);
        }
        // Out-of-range volumes are capped, not wrapped.
        assert_eq!(wav(Sound::Success, 255), wav(Sound::Success, 100));
    }

    #[test]
    fn cues_are_deterministic_and_distinct() {
        for sound in Sound::ALL {
            assert_eq!(wav(sound, 60), wav(sound, 60));
        }
        for (i, a) in Sound::ALL.iter().enumerate() {
            for b in &Sound::ALL[i + 1..] {
                assert_ne!(wav(*a, 60), wav(*b, 60), "{a:?} / {b:?}");
            }
        }
    }

    #[test]
    fn notes_start_and_end_quietly() {
        // No clicks: every cue ramps in and out.
        for sound in Sound::ALL {
            let s = pcm(&wav(sound, 100));
            let full = peak(&wav(sound, 100));
            assert!(i32::from(s[0]).abs() < full / 4, "{sound:?}");
            assert!(i32::from(*s.last().unwrap()).abs() < full / 4, "{sound:?}");
        }
    }

    #[test]
    fn sounds_play_only_when_on_for_that_event_and_audible() {
        let on = Sounds::default();
        assert!(Sound::ALL.iter().all(|&s| on.plays(s)));
        let off = Sounds {
            enabled: false,
            ..on
        };
        assert!(!Sound::ALL.iter().any(|&s| off.plays(s)));
        let silent = Sounds { volume: 0, ..on };
        assert!(!Sound::ALL.iter().any(|&s| silent.plays(s)));
        let no_failure = Sounds {
            failure: false,
            ..on
        };
        assert!(!no_failure.plays(Sound::Failure));
        assert!(no_failure.plays(Sound::Success));
    }
}
