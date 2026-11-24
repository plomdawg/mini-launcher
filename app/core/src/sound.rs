//! Sound effects: short, soft chimes synthesized in code (bell-like notes
//! in a major key), so no audio files or audio crates.
//!
//! [`wav`] builds a cue as an in-memory 16-bit mono WAV with the volume
//! applied to the samples; [`play`] hands it to the OS (`PlaySoundW` on
//! Windows, nothing elsewhere for now). Which cues play is the user's
//! `[settings.sounds]` (see [`Sounds::plays`]).

use crate::config::{Sounds, VOLUME_MAX};

/// Something the app can make a noise about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sound {
    /// The add-game wizard is waiting for a tag: a "ready" chime.
    Waiting,
    /// A tag write started: a quick rising run.
    Writing,
    /// The tag was written (or bound): a major arpeggio.
    Success,
    /// The write failed: a gentle falling third.
    Failure,
    /// A cart launched a game: a pentatonic sparkle.
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

    /// The notes. They may overlap: each rings on under the next.
    fn notes(self) -> &'static [Note] {
        match self {
            // A soft rising fourth: "ding-dong, ready".
            Sound::Waiting => const { &[Note::at(0.0, G5, 0.12), Note::at(0.09, C6, 0.3)] },
            // A quick, light climb while the tag is written.
            Sound::Writing => {
                const {
                    &[
                        Note::at(0.0, C5, 0.1),
                        Note::at(0.06, E5, 0.1),
                        Note::at(0.12, G5, 0.2),
                    ]
                }
            }
            // A bright major arpeggio that rings out: "ta-da!".
            Sound::Success => {
                const {
                    &[
                        Note::at(0.0, C5, 0.15),
                        Note::at(0.07, E5, 0.15),
                        Note::at(0.14, G5, 0.15),
                        Note::at(0.21, C6, 0.34),
                    ]
                }
            }
            // A gentle falling third: "uh-oh", not an alarm.
            Sound::Failure => const { &[Note::at(0.0, E5, 0.16), Note::at(0.15, C5, 0.33)] },
            // A quick pentatonic sparkle up to a held high note.
            Sound::Launch => {
                const {
                    &[
                        Note::at(0.0, C5, 0.08),
                        Note::at(0.035, D5, 0.08),
                        Note::at(0.07, E5, 0.08),
                        Note::at(0.105, G5, 0.08),
                        Note::at(0.14, A5, 0.08),
                        Note::at(0.175, C6, 0.35),
                    ]
                }
            }
        }
    }

    /// How loud the cue is next to the others (its peak, before volume).
    fn level(self) -> f32 {
        match self {
            Sound::Waiting | Sound::Writing | Sound::Failure => 0.7,
            Sound::Success | Sound::Launch => 1.0,
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

/// Samples per second: plenty for these soft tones, and small buffers.
pub const SAMPLE_RATE: u32 = 22_050;

/// The loudest sample at full volume, as a fraction of full scale. Kept
/// low: these play over games and music, so 100 % should still be polite.
const HEADROOM: f32 = 0.2;

// Note frequencies (Hz), equal temperament.
const C5: f32 = 523.25;
const D5: f32 = 587.33;
const E5: f32 = 659.26;
const G5: f32 = 783.99;
const A5: f32 = 880.00;
const C6: f32 = 1046.50;

/// One chime note: a sine with a touch of overtones, struck and left to
/// ring down (a music-box / mallet sound rather than a buzzy square wave).
#[derive(Debug, Clone, Copy)]
struct Note {
    /// When it starts, seconds from the start of the cue.
    at: f32,
    hz: f32,
    /// How long it rings before it is faded out.
    secs: f32,
}

impl Note {
    const fn at(at: f32, hz: f32, secs: f32) -> Note {
        Note { at, hz, secs }
    }
}

/// The cue as samples in -1..=1 (before volume), peaking at its level.
fn samples(sound: Sound) -> Vec<f32> {
    use std::f32::consts::TAU;
    // A short attack so notes don't click, a decay that makes them ring
    // like a struck bell, and a release that ends them at exactly zero.
    const ATTACK: f32 = 0.005;
    const DECAY: f32 = 0.12;
    const RELEASE: f32 = 0.03;
    let rate = SAMPLE_RATE as f32;
    let notes = sound.notes();
    let span = |n: &Note| {
        let first = (n.at * rate).round() as usize;
        (first, (n.secs * rate).round() as usize)
    };
    let len = notes
        .iter()
        .map(|n| {
            let (first, count) = span(n);
            first + count
        })
        .max()
        .unwrap_or(0);
    let mut out = vec![0.0f32; len];
    for note in notes {
        let (first, n) = span(note);
        for i in 0..n {
            let t = i as f32 / rate;
            let x = TAU * note.hz * t;
            let wave = x.sin() + 0.25 * (2.0 * x).sin() + 0.06 * (3.0 * x).sin();
            let left = note.secs - t;
            let env = (t / ATTACK).min(1.0) * (-t / DECAY).exp() * (left / RELEASE).min(1.0);
            out[first + i] += wave * env;
        }
    }
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 0.0 {
        let scale = sound.level() / peak;
        out.iter_mut().for_each(|s| *s *= scale);
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
