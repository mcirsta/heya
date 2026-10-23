//! Original synthesized call sounds, played by the existing finite audio helper.
use crate::{
    audio,
    audio_devices::Settings,
    audio_player::{Desired, Output, Worker},
    voice_recorder::Input,
};
use std::{f64::consts::TAU, sync::Arc};
use tokio::sync::watch;

#[derive(Clone, Copy)]
pub enum Tone {
    Incoming,
    Outgoing,
}

pub fn start(tone: Tone, settings: Settings, input: Input) -> Option<Worker> {
    if !audio::SUPPORTED {
        return None;
    }
    let output = if cfg!(test) || !matches!(input, Input::Microphone) {
        Output::Silent
    } else {
        Output::Desktop(settings)
    };
    let (updates, _) = watch::channel(audio::Status::default());
    Worker::start(desired(tone), updates, output).ok()
}

pub async fn stop(player: &mut Option<Worker>) {
    if let Some(mut player) = player.take() {
        player.stop();
        // Includes pipe closure and helper reaping, even if the output is stuck.
        let _ = player.finished().await;
    }
}

fn desired(tone: Tone) -> Desired {
    Desired {
        generation: 1,
        bytes: Some(wave(tone)),
        playing: true,
        speed: 1.,
        ..Default::default()
    }
}

fn wave(tone: Tone) -> Arc<[u8]> {
    // One four-second cycle, repeated to the existing offer/ringing deadline.
    // Short fades avoid clicks. Generate on the call worker, never the UI loop.
    const RATE: usize = 8_000;
    let (seconds, notes): (_, &[(usize, usize, f64, f64)]) = match tone {
        Tone::Incoming => (
            90,
            &[
                (0, 180, 660., 880.),
                (260, 180, 880., 1100.),
                (520, 180, 660., 880.),
            ],
        ),
        Tone::Outgoing => (60, &[(0, 400, 440., 480.), (600, 400, 440., 480.)]),
    };
    let mut cycle = vec![0i16; RATE * 4];
    for &(offset_ms, length_ms, a, b) in notes {
        let length = length_ms * RATE / 1000;
        let offset = offset_ms * RATE / 1000;
        for i in 0..length {
            let fade = (i.min(length - 1 - i) as f64 / (RATE / 100) as f64).min(1.);
            let t = i as f64 / RATE as f64;
            cycle[offset + i] =
                (i16::MAX as f64 * 0.11 * fade * ((TAU * a * t).sin() + (TAU * b * t).sin()))
                    as i16;
        }
    }
    let samples = seconds * RATE;
    let size = (samples * 2) as u32;
    let mut bytes = Vec::with_capacity(size as usize + 44);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + size).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&(RATE as u32).to_le_bytes());
    bytes.extend_from_slice(&(RATE as u32 * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&size.to_le_bytes());
    for sample in cycle.iter().cycle().take(samples) {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn call_sounds_are_distinct_bounded_pcm_with_quiet_gaps() {
        let incoming = wave(Tone::Incoming);
        let outgoing = wave(Tone::Outgoing);
        for (bytes, seconds) in [(&incoming, 90), (&outgoing, 60)] {
            assert_eq!(&bytes[..4], b"RIFF");
            assert_eq!(&bytes[8..16], b"WAVEfmt ");
            assert_eq!(bytes.len(), 44 + seconds * 8000 * 2);
            assert!(bytes.len() < audio::MAX_BYTES);
            let samples: Vec<_> = bytes[44..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|v| i16::from_le_bytes(*v))
                .collect();
            assert!(samples.iter().any(|v| v.unsigned_abs() > 1000));
            assert!(samples.iter().all(|v| v.unsigned_abs() < 8000));
            assert!(samples[8000..32000].iter().all(|v| *v == 0));
            assert_eq!(&samples[..32000], &samples[32000..64000]);
        }
        assert_ne!(&incoming[44..16044], &outgoing[44..16044]);
    }

    #[tokio::test]
    async fn both_sounds_play_offscreen_and_stop_before_their_owner_returns() {
        if !audio::SUPPORTED {
            assert!(start(Tone::Incoming, Settings::default(), Input::Synthetic).is_none());
            return;
        }
        for tone in [Tone::Incoming, Tone::Outgoing] {
            let (updates, mut status) = watch::channel(audio::Status::default());
            let mut player = Some(Worker::start(desired(tone), updates, Output::Silent).unwrap());
            tokio::time::timeout(Duration::from_secs(4), status.wait_for(|s| s.phase == 2))
                .await
                .unwrap()
                .unwrap();
            tokio::time::timeout(Duration::from_secs(3), stop(&mut player))
                .await
                .unwrap();
            assert!(player.is_none());
            stop(&mut player).await;
        }
    }
}
