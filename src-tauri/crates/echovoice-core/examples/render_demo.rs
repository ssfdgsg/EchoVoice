//! Renders a short demo scene through the real mixer and writes it to a WAV file.
//!
//!   cargo run -p echovoice-core --example render_demo -- /tmp/echovoice-demo.wav
//!
//! The point is to have a tangible artifact: open the WAV in any editor and you can see
//! the mic bed, the ducking dip, the two overlapping effects and the soft-limited peaks.

use echovoice_core::config::AudioSettings;
use echovoice_core::mixer::{Mixer, VoiceOptions};
use echovoice_core::wav::WavWriter;
use std::sync::Arc;

const RATE: u32 = 48_000;

fn sine(len: usize, freq: f32, amp: f32) -> Vec<f32> {
    (0..len)
        .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / RATE as f32).sin() * amp)
        .collect()
}

fn interleave(left: &[f32], right: &[f32]) -> Vec<f32> {
    left.iter()
        .zip(right.iter())
        .flat_map(|(l, r)| [*l, *r])
        .collect()
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/echovoice-demo.wav".to_string());

    let mixer = Mixer::new(RATE, 2);
    mixer.apply_settings(&AudioSettings {
        mic_volume: 0.9,
        fx_volume: 0.8,
        music_volume: 0.45,
        noise_gate_threshold: 0.01,
        mic_muted: false,
        fx_muted: false,
        ducking_enabled: true,
        ducking_amount: 0.5,
        max_concurrent_sounds: 8,
        fade_ms: 8.0,
        eq: Default::default(),
        compressor: Default::default(),
        pitch: Default::default(),
    });

    let mic = sine(RATE as usize * 4, 180.0, 0.45);
    let music = sine(RATE as usize * 4, 110.0, 0.5);
    let sfx_a = sine(RATE as usize, 880.0, 0.6);
    let sfx_b = sine(RATE as usize, 1320.0, 0.35);

    mixer.play_music(
        "music",
        Arc::new(interleave(&music, &music)),
        VoiceOptions {
            loop_playback: true,
            fade_in_ms: 20.0,
            fade_out_ms: 20.0,
            ..Default::default()
        },
    );
    mixer.play_sound(
        "airhorn",
        Arc::new(interleave(&sfx_a, &sfx_a)),
        VoiceOptions::default(),
    );
    mixer.play_sound(
        "violin",
        Arc::new(interleave(&sfx_b, &sfx_b)),
        VoiceOptions::default(),
    );

    let mut output: Vec<f32> = Vec::new();
    for chunk in 0..(RATE as usize * 4 / 480) {
        let start = chunk * 480;
        let mic_block = interleave(&mic[start..start + 480], &mic[start..start + 480]);
        let mut rendered = vec![0.0f32; 960];
        mixer.process_frames(&mic_block, &mut rendered);
        output.extend_from_slice(&rendered);
    }

    let mut writer = WavWriter::new(RATE, 2);
    writer.push_interleaved(&output);
    writer.write_to_file(&path).unwrap();

    let peak = output.iter().fold(0.0f32, |p, s| p.max(s.abs()));
    let rms = (output.iter().map(|s| s * s).sum::<f32>() / output.len() as f32).sqrt();
    println!("wrote {path}");
    println!("  duration : {} ms", writer.duration_ms());
    println!("  samples  : {}", output.len());
    println!("  peak     : {peak:.4}");
    println!("  rms      : {rms:.4}  ({:.1} dBFS)", 20.0 * rms.log10());
    let stats = mixer.stats();
    println!("  clipped  : {} samples", stats.clipped_samples);
    println!("  ducking  : {}", stats.ducking);
    println!(
        "  music    : playing={} paused={}",
        stats.music_playing, stats.music_paused
    );
}
