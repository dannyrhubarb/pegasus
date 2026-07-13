// ---- Procedural audio -----------------------------------------------------
// Tiny WAVs are synthesised in memory at startup (no asset files): a looping
// low-passed noise rumble for the engine (volume follows `glow`) and a
// decaying noise burst for crashes. Native builds link macroquad's dummy audio
// backend (the "audio" feature is wasm-only, see Cargo.toml) and stay silent.

use pegasus_sim::world::Rng;

pub const AUDIO_RATE: u32 = 22050;

// 16-bit mono PCM → WAV bytes.
pub fn wav_from_samples(samples: &[i16], rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());      // fmt chunk size
    w.extend_from_slice(&1u16.to_le_bytes());       // PCM
    w.extend_from_slice(&1u16.to_le_bytes());       // mono
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 2).to_le_bytes()); // byte rate
    w.extend_from_slice(&2u16.to_le_bytes());       // block align
    w.extend_from_slice(&16u16.to_le_bytes());      // bits per sample
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

// 1 s of low-passed noise, cross-faded into itself for a click-free loop.
pub fn thruster_wav() -> Vec<u8> {
    let n = AUDIO_RATE as usize;
    let mut rng = Rng::new(0x7448_5254);
    let mut lp = 0.0f32;
    let mut s = Vec::with_capacity(n);
    for _ in 0..n {
        let white = rng.unit() * 2.0 - 1.0;
        lp += (white - lp) * 0.12; // one-pole low-pass → deep rumble
        s.push(lp * 2.4);
    }
    let fade = 2048;
    for k in 0..fade {
        let t = k as f32 / fade as f32;
        s[n - fade + k] = s[n - fade + k] * (1.0 - t) + s[k] * t;
    }
    let pcm: Vec<i16> = s.iter().map(|v| (v.clamp(-1.0, 1.0) * 22000.0) as i16).collect();
    wav_from_samples(&pcm, AUDIO_RATE)
}

// 0.9 s noise burst that darkens as it decays — the crash boom.
pub fn boom_wav() -> Vec<u8> {
    let n = (AUDIO_RATE as f32 * 0.9) as usize;
    let mut rng = Rng::new(0x424f_4f4d);
    let mut lp = 0.0f32;
    let mut pcm = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / n as f32;
        let white = rng.unit() * 2.0 - 1.0;
        lp += (white - lp) * (0.30 * (1.0 - t) + 0.04);
        let env = (1.0 - t) * (1.0 - t);
        pcm.push(((lp * env * 3.0).clamp(-1.0, 1.0) * 26000.0) as i16);
    }
    wav_from_samples(&pcm, AUDIO_RATE)
}
