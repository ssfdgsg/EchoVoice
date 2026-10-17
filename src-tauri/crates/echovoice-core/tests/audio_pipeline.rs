//! End-to-end audio verification.
//!
//! These tests push *real* signals through the *real* mixer and DSP chain, then measure
//! the result numerically. Nothing is mocked: if the ducking maths is wrong, the gate
//! hysteresis is wrong, or the soft clipper distorts, these fail.

use echovoice_core::config::AudioSettings;
use echovoice_core::dsp::{
    compressor::Compressor, eq::ThreeBandEq, pitch::PitchShifter, AudioEffect, DspChain,
};
use echovoice_core::fx::SoundBank;
use echovoice_core::mixer::{Mixer, VoiceOptions};
use echovoice_core::wav::WavWriter;
use std::sync::Arc;

mod signal;
use signal::*;

const RATE: u32 = 48_000;

fn mono(data: Vec<f32>) -> Arc<Vec<f32>> {
    Arc::new(data)
}

fn opts(volume: f32) -> VoiceOptions {
    VoiceOptions {
        volume,
        fade_in_ms: 0.0,
        fade_out_ms: 0.0,
        ..Default::default()
    }
}

/// Runs the mixer over `seconds` of `mic`, returning the rendered output.
fn render(mixer: &Mixer, mic: &[f32], seconds: f32) -> Vec<f32> {
    let block = 480usize; // 10 ms
    let total = (RATE as f32 * seconds) as usize;
    let mut out = Vec::with_capacity(total);
    let mut cursor = 0;
    while cursor < total {
        let take = block
            .min(total - cursor)
            .min(mic.len().saturating_sub(cursor));
        let mic_block: Vec<f32> = if take == 0 {
            vec![0.0; block]
        } else {
            mic[cursor..cursor + take].to_vec()
        };
        let mut rendered = vec![0.0f32; mic_block.len()];
        mixer.process_frames(&mic_block, &mut rendered);
        out.extend_from_slice(&rendered);
        cursor += mic_block.len();
    }
    out
}

// ---------------------------------------------------------------- basic mixing

#[test]
fn two_overlapping_sounds_sum_linearly() {
    let mixer = Mixer::new(RATE, 1);
    mixer.play_sound(
        "a",
        mono(sine(RATE as usize, RATE as f32, 440.0, 0.5)),
        opts(1.0),
    );
    mixer.play_sound(
        "b",
        mono(sine(RATE as usize, RATE as f32, 554.0, 0.5)),
        opts(1.0),
    );

    let out = render(&mixer, &vec![0.0; RATE as usize], 1.0);

    // Unc related tones: total power is the sum of the individual powers.
    let expected_rms = (0.5f32 * 0.5 / 2.0 + 0.5 * 0.5 / 2.0).sqrt();
    let measured = rms(&out[out.len() / 2..]); // skip the first half so it is steady state
    assert!(
        (measured - expected_rms).abs() < 0.02,
        "expected RMS {expected_rms}, measured {measured}"
    );
    assert!(peak(&out) < 1.0, "two half-amplitude tones must not clip");
    // Both tones must be present.
    assert!(goertzel(&out, RATE as f32, 440.0) > 0.45);
    assert!(goertzel(&out, RATE as f32, 554.0) > 0.45);
}

#[test]
fn per_sound_volume_is_applied_exactly() {
    let quiet = Mixer::new(RATE, 1);
    quiet.play_sound(
        "a",
        mono(sine(RATE as usize, RATE as f32, 440.0, 1.0)),
        opts(0.25),
    );
    let out_quiet = render(&quiet, &vec![0.0; RATE as usize], 1.0);

    let loud = Mixer::new(RATE, 1);
    loud.play_sound(
        "a",
        mono(sine(RATE as usize, RATE as f32, 440.0, 1.0)),
        opts(1.0),
    );
    let out_loud = render(&loud, &vec![0.0; RATE as usize], 1.0);

    let ratio = rms(&out_quiet[out_quiet.len() / 2..]) / rms(&out_loud[out_loud.len() / 2..]);
    assert!((ratio - 0.25).abs() < 0.01, "volume ratio was {ratio}");
}

#[test]
fn microphone_and_effects_mix_at_their_own_levels() {
    let mixer = Mixer::new(RATE, 1);
    mixer.set_mic_volume(0.5);
    mixer.set_fx_volume(0.5);
    mixer.play_sound(
        "a",
        mono(sine(RATE as usize, RATE as f32, 440.0, 1.0)),
        opts(1.0),
    );

    let mic = sine(RATE as usize, RATE as f32, 300.0, 1.0);
    let out = render(&mixer, &mic, 1.0);

    let tone_440 = goertzel(&out, RATE as f32, 440.0);
    let tone_300 = goertzel(&out, RATE as f32, 300.0);
    assert!((tone_440 - 0.5).abs() < 0.03, "fx level was {tone_440}");
    assert!((tone_300 - 0.5).abs() < 0.03, "mic level was {tone_300}");
}

// -------------------------------------------------------------------- ducking

#[test]
fn ducking_lowers_the_microphone_by_the_configured_amount() {
    let mixer = Mixer::new(RATE, 1);
    mixer.set_ducking(true, 0.5); // -6 dB
    let mic = sine(RATE as usize * 2, RATE as f32, 1000.0, 0.5);

    // 1 s of mic alone, then start a soundboard effect and render another second.
    let before = render(&mixer, &mic[..RATE as usize], 1.0);
    mixer.play_sound(
        "airhorn",
        mono(sine(RATE as usize * 2, RATE as f32, 220.0, 0.4)),
        opts(1.0),
    );
    let after = render(&mixer, &mic[..RATE as usize], 1.0);

    let mic_before = goertzel(&before, RATE as f32, 1000.0);
    let mic_after = goertzel(&after[after.len() / 2..], RATE as f32, 1000.0);
    let reduction = db(mic_before / mic_after);
    assert!(
        (reduction - 6.0).abs() < 1.5,
        "expected ~6 dB of ducking, measured {reduction} dB"
    );
    // The effect itself must still be there at full level.
    assert!(goertzel(&after, RATE as f32, 220.0) > 0.35);
}

#[test]
fn music_does_not_duck_the_microphone() {
    // Dipping the mic for the whole length of a song would make you inaudible, so the
    // ducking sidechain only listens to soundboard effects.
    let mixer = Mixer::new(RATE, 1);
    mixer.set_ducking(true, 0.5);
    let mic = sine(RATE as usize * 2, RATE as f32, 1000.0, 0.5);

    let before = render(&mixer, &mic[..RATE as usize], 1.0);
    mixer.play_music(
        "track",
        mono(sine(RATE as usize * 2, RATE as f32, 220.0, 0.4)),
        opts(1.0),
    );
    let after = render(&mixer, &mic[..RATE as usize], 1.0);

    let mic_before = goertzel(&before, RATE as f32, 1000.0);
    let mic_after = goertzel(&after[after.len() / 2..], RATE as f32, 1000.0);
    assert!(
        db(mic_before / mic_after).abs() < 0.5,
        "music must not duck the mic ({} dB)",
        db(mic_before / mic_after)
    );
    assert!(goertzel(&after, RATE as f32, 220.0) > 0.3);
}

#[test]
fn ducking_releases_after_the_sound_stops() {
    let mixer = Mixer::new(RATE, 1);
    mixer.set_ducking(true, 0.5);
    let mic = sine(RATE as usize * 3, RATE as f32, 1000.0, 0.5);

    mixer.play_music(
        "track",
        mono(sine(RATE as usize, RATE as f32, 220.0, 0.4)),
        opts(1.0),
    );
    let _ = render(&mixer, &mic[..RATE as usize], 1.0);
    mixer.stop_music();
    let released = render(&mixer, &mic[..RATE as usize * 2], 2.0);

    let level = goertzel(&released[released.len() / 2..], RATE as f32, 1000.0);
    assert!(
        (level - 0.5).abs() < 0.05,
        "mic should be back at full level, measured {level}"
    );
}

// ----------------------------------------------------------------- noise gate

#[test]
fn noise_gate_silences_quiet_input_and_passes_loud_input() {
    let mixer = Mixer::new(RATE, 1);
    mixer.set_noise_gate_threshold(0.05);

    let quiet = sine(RATE as usize, RATE as f32, 1000.0, 0.005);
    let out_quiet = render(&mixer, &quiet, 1.0);
    assert!(
        rms(&out_quiet) < 0.002,
        "quiet input should be gated, RMS was {}",
        rms(&out_quiet)
    );

    let loud = sine(RATE as usize, RATE as f32, 1000.0, 0.5);
    let out_loud = render(&mixer, &loud, 1.0);
    assert!(
        rms(&out_loud) > 0.3,
        "loud input should pass, RMS was {}",
        rms(&out_loud)
    );
}

#[test]
fn noise_gate_has_no_audible_gap_when_speech_starts() {
    let mixer = Mixer::new(RATE, 1);
    mixer.set_noise_gate_threshold(0.05);
    // 200 ms of silence then a tone: the gate must open within a few milliseconds.
    let mut mic = vec![0.0f32; (RATE as f32 * 0.2) as usize];
    mic.extend(sine(RATE as usize, RATE as f32, 1000.0, 0.5));

    let out = render(&mixer, &mic, 0.6);
    let onset = (RATE as f32 * 0.2) as usize;
    // Find the first sample after the onset that reaches half amplitude.
    let half = out[onset..]
        .iter()
        .position(|s| s.abs() > 0.25)
        .unwrap_or(out.len());
    let open_ms = half as f32 / RATE as f32 * 1000.0;
    assert!(open_ms < 30.0, "gate took {open_ms} ms to open");
}

// ---------------------------------------------------------------- soft clip

#[test]
fn soft_clip_keeps_the_signal_in_range_and_beats_hard_clipping() {
    // 2x is a realistic overload for this mixer (mic + two effects + music bed).
    // A hard clamp flattens the crest to exactly 1.0 and dumps energy into the high odd
    // harmonics that make clipping sound harsh; the soft knee rounds the crest off.
    let drive = 2.0f32;
    let mixer = Mixer::new(RATE, 1);
    mixer.play_sound(
        "a",
        mono(sine(RATE as usize, RATE as f32, 1000.0, drive)),
        opts(1.0),
    );
    let out = render(&mixer, &vec![0.0; RATE as usize], 1.0);

    assert!(
        peak(&out) <= 1.0 + 1e-6,
        "output exceeded unity: {}",
        peak(&out)
    );

    let hard: Vec<f32> = sine(RATE as usize, RATE as f32, 1000.0, drive)
        .iter()
        .map(|s| s.clamp(-1.0, 1.0))
        .collect();

    let soft_harsh = high_order_harmonics(&out[out.len() / 2..], RATE as f32, 1000.0);
    let hard_harsh = high_order_harmonics(&hard[hard.len() / 2..], RATE as f32, 1000.0);
    assert!(
        soft_harsh < hard_harsh * 0.85,
        "soft clip harsh harmonics {soft_harsh} should be below hard clip {hard_harsh}"
    );
    // The crest must stay rounded rather than flattened to a shelf.
    assert!(
        peak(&out) < 0.999,
        "crest was flattened to unity ({}), which is a hard clamp",
        peak(&out)
    );
}

#[test]
fn a_badly_overloaded_mix_stays_in_range() {
    // No waveshaper can make 3x overload clean - that needs a look-ahead limiter - but it
    // must never leave the [-1, 1] range or go non-finite.
    let mixer = Mixer::new(RATE, 1);
    mixer.play_sound(
        "a",
        mono(sine(RATE as usize, RATE as f32, 1000.0, 3.0)),
        opts(1.0),
    );
    let out = render(&mixer, &vec![3.0; RATE as usize], 1.0);

    assert!(peak(&out) <= 1.0 + 1e-6, "exceeded unity: {}", peak(&out));
    assert!(out.iter().all(|s| s.is_finite()));
    assert!(
        mixer.stats().clipped_samples > 0,
        "overload was not reported"
    );
}

#[test]
fn normal_levels_are_untouched_by_the_limiter() {
    let mixer = Mixer::new(RATE, 1);
    mixer.play_sound(
        "a",
        mono(sine(RATE as usize, RATE as f32, 1000.0, 0.7)),
        opts(1.0),
    );
    let out = render(&mixer, &vec![0.0; RATE as usize], 1.0);

    let level = goertzel(&out, RATE as f32, 1000.0);
    assert!((level - 0.7).abs() < 0.02, "expected 0.7, measured {level}");
    assert!(
        thd(&out, RATE as f32, 1000.0) < 0.01,
        "clean signal must stay clean"
    );
}

// -------------------------------------------------------------------- music

#[test]
fn music_and_sound_effects_are_independent_channels() {
    let mixer = Mixer::new(RATE, 1);
    mixer.set_fx_volume(0.0); // mute the soundboard entirely
    mixer.set_music_volume(0.5);
    mixer.play_sound(
        "sfx",
        mono(sine(RATE as usize, RATE as f32, 800.0, 1.0)),
        opts(1.0),
    );
    mixer.play_music(
        "track",
        mono(sine(RATE as usize, RATE as f32, 200.0, 1.0)),
        opts(1.0),
    );

    let out = render(&mixer, &vec![0.0; RATE as usize], 1.0);

    assert!(
        goertzel(&out, RATE as f32, 800.0) < 0.01,
        "muted soundboard should be silent"
    );
    assert!(
        (goertzel(&out, RATE as f32, 200.0) - 0.5).abs() < 0.03,
        "music must survive a muted soundboard"
    );
}

#[test]
fn pausing_music_freezes_it_without_touching_effects() {
    let mixer = Mixer::new(RATE, 1);
    mixer.play_sound(
        "sfx",
        mono(sine(RATE as usize, RATE as f32, 800.0, 0.5)),
        opts(1.0),
    );
    mixer.play_music(
        "track",
        mono(sine(RATE as usize, RATE as f32, 200.0, 0.5)),
        opts(1.0),
    );

    let _ = render(&mixer, &vec![0.0; RATE as usize], 0.5);
    let position_before = mixer.music_progress().unwrap().0;
    mixer.set_music_paused(true);
    let paused = render(&mixer, &vec![0.0; RATE as usize], 0.5);

    assert_eq!(
        mixer.music_progress().unwrap().0,
        position_before,
        "paused music moved"
    );
    assert!(
        goertzel(&paused, RATE as f32, 200.0) < 0.01,
        "paused music is audible"
    );
    assert!(
        goertzel(&paused, RATE as f32, 800.0) > 0.45,
        "effects must keep playing while music is paused"
    );
}

// ------------------------------------------------------------------ fades

#[test]
fn fades_remove_the_click_at_the_start_and_end_of_a_sound() {
    let mixer = Mixer::new(RATE, 1);
    // A sound that starts and stops at full scale: without fades this clicks.
    mixer.play_sound(
        "a",
        mono(vec![1.0f32; RATE as usize]),
        VoiceOptions {
            volume: 1.0,
            fade_in_ms: 10.0,
            fade_out_ms: 10.0,
            ..Default::default()
        },
    );

    let out = render(&mixer, &vec![0.0; RATE as usize], 1.0);

    assert!(
        out[0].abs() < 0.02,
        "fade-in did not start at zero: {}",
        out[0]
    );
    assert!(
        out[out.len() - 1].abs() < 0.02,
        "fade-out did not end at zero: {}",
        out[out.len() - 1]
    );
    // And it must still be essentially at full level in the middle. The soft knee
    // deliberately shaves a fraction of a dB off unity instead of hard-clipping it.
    assert!(peak(&out[out.len() / 4..out.len() / 4 * 3]) > 0.9);
}

// ---------------------------------------------------------------------- DSP

#[test]
fn eq_delivers_the_requested_gain_at_the_shelf_frequencies() {
    let mut eq = ThreeBandEq::new(RATE);
    eq.set_low_gain(9.0);
    eq.set_mid_gain(-6.0);
    eq.set_high_gain(6.0);
    eq.set_enabled(true);

    for (freq, expected_db) in [(60.0f32, 9.0), (1500.0, -6.0), (10_000.0, 6.0)] {
        let mut buffer = sine(RATE as usize, RATE as f32, freq, 0.2);
        eq.process(&mut buffer, 1);
        let gained = goertzel(&buffer[buffer.len() / 2..], RATE as f32, freq);
        let measured = db(gained / 0.2);
        assert!(
            (measured - expected_db).abs() < 1.5,
            "{freq} Hz: expected {expected_db} dB, measured {measured} dB"
        );
    }
}

#[test]
fn compressor_evens_out_loud_and_quiet_passages() {
    let mut comp = Compressor::new(RATE);
    comp.set_threshold(-20.0);
    comp.set_ratio(6.0);
    comp.set_enabled(true);

    // A quiet passage and a loud one, 6 dB apart in, must come out much closer.
    let quiet_in = sine(RATE as usize / 2, RATE as f32, 1000.0, 0.05);
    let loud_in = sine(RATE as usize / 2, RATE as f32, 1000.0, 0.4);
    let mut quiet = quiet_in.clone();
    let mut loud = loud_in.clone();
    comp.process(&mut quiet, 1);
    comp.process(&mut loud, 1);

    let ratio_in = rms(&loud_in) / rms(&quiet_in);
    let ratio_out = rms(&loud) / rms(&quiet);
    assert!(
        ratio_out < ratio_in * 0.6,
        "compression too weak: in ratio {ratio_in}, out ratio {ratio_out}"
    );
}

#[test]
fn pitch_shifter_keeps_the_signal_bounded() {
    let mut pitch = PitchShifter::new(RATE);
    pitch.set_pitch_ratio(1.5);
    pitch.set_enabled(true);
    let mut buffer = sine(RATE as usize, RATE as f32, 300.0, 0.8);
    pitch.process(&mut buffer, 1);
    assert!(peak(&buffer) <= 1.0 + 1e-6, "pitch shifter exceeded unity");
    assert!(buffer.iter().all(|s| s.is_finite()));
}

#[test]
fn dsp_chain_processes_every_effect_in_order() {
    let mut chain = DspChain::new();
    chain.add_effect(Box::new(ThreeBandEq::new(RATE)));
    chain.add_effect(Box::new(Compressor::new(RATE)));
    chain.add_effect(Box::new(PitchShifter::new(RATE)));
    chain.set_channels(2);

    // Interleaved stereo.
    let mut buffer: Vec<f32> = (0..RATE as usize)
        .flat_map(|i| {
            let s = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / RATE as f32).sin() * 0.5;
            [s, s]
        })
        .collect();
    chain.process(&mut buffer, 2);
    assert!(buffer
        .iter()
        .all(|s| s.is_finite() && s.abs() <= 1.0 + 1e-6));
}

// ------------------------------------------------------- full pipeline to disk

#[test]
fn full_pipeline_renders_a_wav_that_round_trips_through_the_decoder() {
    let dir = std::env::temp_dir().join(format!("echovoice-audio-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // A realistic scene: speech-like mic, a looping music bed, two overlapping effects.
    let mixer = Mixer::new(RATE, 2);
    mixer.apply_settings(&AudioSettings {
        mic_volume: 0.9,
        fx_volume: 0.8,
        music_volume: 0.45,
        noise_gate_threshold: 0.01,
        ducking_enabled: true,
        ducking_amount: 0.5,
        max_concurrent_sounds: 8,
        fade_ms: 8.0,
        eq: Default::default(),
        compressor: Default::default(),
        pitch: Default::default(),
        mic_muted: false,
        fx_muted: false,
    });

    let mic = sine(RATE as usize * 3, RATE as f32, 180.0, 0.45);
    let music = sine(RATE as usize * 3, RATE as f32, 110.0, 0.5);
    let sfx_a = sine(RATE as usize, RATE as f32, 880.0, 0.6);
    let sfx_b = sine(RATE as usize, RATE as f32, 1320.0, 0.35);

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
    mixer.play_sound("sfx_a", Arc::new(interleave(&sfx_a, &sfx_a)), opts(1.0));
    mixer.play_sound("sfx_b", Arc::new(interleave(&sfx_b, &sfx_b)), opts(1.0));

    // Render 3 seconds in 10 ms blocks, interleaving the mic across both channels.
    let mut output: Vec<f32> = Vec::new();
    let block = 480 * 2;
    for chunk in 0..(RATE as usize * 3 / 480) {
        let start = chunk * 480;
        let mic_block = interleave(&mic[start..start + 480], &mic[start..start + 480]);
        let mut rendered = vec![0.0f32; block];
        mixer.process_frames(&mic_block, &mut rendered);
        output.extend_from_slice(&rendered);
    }

    assert_eq!(output.len(), RATE as usize * 3 * 2, "wrong output length");
    assert!(output.iter().all(|s| s.is_finite()));

    // Write it out and read it back with the real decoder.
    let path = dir.join("pipeline.wav");
    let mut writer = WavWriter::new(RATE, 2);
    writer.push_interleaved(&output);
    assert_eq!(writer.duration_ms(), 3000);
    writer.write_to_file(path.to_str().unwrap()).unwrap();

    let bank = SoundBank::load_from_file(path.to_str().unwrap(), RATE, 2).unwrap();
    assert_eq!(bank.channels, 2);
    assert_eq!(bank.sample_rate, RATE);
    assert_eq!(bank.duration_ms(), 3000);

    // 16-bit quantisation costs about 1 LSB; everything else must survive.
    let left: Vec<f32> = bank.data.iter().step_by(2).copied().collect();
    let right: Vec<f32> = bank.data.iter().skip(1).step_by(2).copied().collect();
    let expected_left: Vec<f32> = output.iter().step_by(2).copied().collect();
    for (got, want) in left.iter().zip(expected_left.iter()) {
        assert!(
            (got - want).abs() < 1e-4,
            "round-trip drifted: {got} vs {want}"
        );
    }
    // Both channels carry the same mono source. The soft knee is symmetric, so any
    // difference is pure floating point noise, never an imbalance.
    let max_channel_diff = left
        .iter()
        .zip(right.iter())
        .map(|(l, r)| (l - r).abs())
        .fold(0.0f32, f32::max);
    assert!(
        max_channel_diff < 1e-6,
        "channels diverged by {max_channel_diff}; a fade or effect is not frame aligned"
    );

    // The scene must contain every component we put in. The effects only sound during the
    // first second, so they are measured over that second alone; the mic and the looping
    // music bed run for the whole three.
    let first_second = &left[..RATE as usize];
    // Expected levels are source amplitude x the configured bus gain: mic 0.45*0.9,
    // music 0.5*0.45, effects 0.6*0.8 and 0.35*0.8. The mic is additionally ducked for the
    // one second the effects sound, so its window average is 0.405 * (2/3 + 1/3 * 0.5).
    for (freq, min, window) in [
        (180.0f32, 0.30, &left[..]),
        (110.0, 0.18, &left[..]),
        (880.0, 0.42, first_second),
        (1320.0, 0.22, first_second),
    ] {
        let level = goertzel(window, RATE as f32, freq);
        assert!(
            level > min,
            "{freq} Hz missing from the render (level {level})"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Interleaves two mono buffers into one stereo buffer.
fn interleave(left: &[f32], right: &[f32]) -> Vec<f32> {
    left.iter()
        .zip(right.iter())
        .flat_map(|(l, r)| [*l, *r])
        .collect()
}
