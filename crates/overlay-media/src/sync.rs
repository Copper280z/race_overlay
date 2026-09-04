use rustfft::{FftPlanner, num_complex::Complex};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlignmentResult {
    /// Delay of the second signal relative to the first. Positive means the
    /// second signal starts later.
    pub offset_seconds: f64,
    /// Normalized correlation at the selected peak, in [-1, 1].
    pub normalized_peak: f64,
    pub next_peak: f64,
    pub confidence: f64,
}
impl AlignmentResult {
    pub fn offset(&self) -> f64 {
        self.offset_seconds
    }
    pub fn peak(&self) -> f64 {
        self.normalized_peak
    }
    pub fn is_auto_acceptable(&self) -> bool {
        self.auto_acceptable()
    }
    pub fn auto_acceptable(&self) -> bool {
        self.normalized_peak >= 0.35 && self.confidence >= 2.0
    }
    pub fn acceptable(&self) -> bool {
        self.auto_acceptable()
    }
}

/// Find the relative delay between two PCM streams.  Correlation is searched
/// coarsely after reducing to 2 kHz, then refined near that result at 8 kHz.
pub fn align_audio(reference: &[f32], candidate: &[f32], sample_rate: u32) -> AlignmentResult {
    if reference.is_empty() || candidate.is_empty() || sample_rate == 0 {
        return AlignmentResult {
            offset_seconds: 0.0,
            normalized_peak: 0.0,
            next_peak: 0.0,
            confidence: 0.0,
        };
    }
    let coarse_rate = 2_000u32.min(sample_rate);
    let a2 = downsample(reference, sample_rate, coarse_rate);
    let b2 = downsample(candidate, sample_rate, coarse_rate);
    let coarse = best_lag(&a2, &b2, coarse_rate, None);
    let fine_rate = 8_000u32.min(sample_rate);
    let a8 = downsample(reference, sample_rate, fine_rate);
    let b8 = downsample(candidate, sample_rate, fine_rate);
    let radius = fine_rate as isize; // one second around coarse result
    let center = (coarse.lag as f64 * fine_rate as f64 / coarse_rate as f64).round() as isize;
    let fine = best_lag(
        &a8,
        &b8,
        fine_rate,
        Some((center - radius, center + radius)),
    );
    let mut next = next_peak(&a8, &b8, fine_rate, fine.lag, fine.score);
    // If local refinement has too little context, use the coarse result's
    // global runner-up rather than manufacturing a high confidence value.
    if !next.is_finite() {
        next = 0.0;
    }
    let confidence = if next > 0.0 {
        fine.score / next
    } else if fine.score > 0.0 {
        f64::INFINITY
    } else {
        0.0
    };
    AlignmentResult {
        offset_seconds: fine.lag as f64 / fine_rate as f64,
        normalized_peak: fine.score,
        next_peak: next,
        confidence,
    }
}

fn downsample(input: &[f32], source_rate: u32, target_rate: u32) -> Vec<f32> {
    if source_rate <= target_rate {
        return input.to_vec();
    }
    let output_len = (input.len() as u64 * target_rate as u64 / source_rate as u64) as usize;
    let mut output = Vec::with_capacity(output_len.max(1));
    for i in 0..output_len.max(1) {
        // Average the source interval instead of simply decimating it. Raw
        // onboard audio contains a great deal of high-frequency engine noise;
        // point sampling aliases that energy and can erase the true coarse
        // correlation peak before the 8 kHz refinement step.
        let start = (i as u64 * source_rate as u64 / target_rate as u64) as usize;
        let mut end = ((i as u64 + 1) * source_rate as u64 / target_rate as u64) as usize;
        end = end.max(start + 1).min(input.len());
        let mean = input[start..end].iter().map(|v| *v as f64).sum::<f64>() / (end - start) as f64;
        output.push(mean as f32);
    }
    output
}

#[derive(Clone, Copy)]
struct Peak {
    lag: isize,
    score: f64,
}

fn best_lag(a: &[f32], b: &[f32], rate: u32, window: Option<(isize, isize)>) -> Peak {
    if a.is_empty() || b.is_empty() {
        return Peak { lag: 0, score: 0.0 };
    }
    let aa = demean(a);
    let bb = demean(b);
    let n = (aa.len() + bb.len()).next_power_of_two();
    let mut fa = vec![Complex::ZERO; n];
    let mut fb = vec![Complex::ZERO; n];
    for (d, &v) in fa.iter_mut().zip(aa.iter()) {
        d.re = v as f64;
    }
    for (d, &v) in fb.iter_mut().zip(bb.iter()) {
        d.re = v as f64;
    }
    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(n);
    let ifft = planner.plan_fft_inverse(n);
    fft.process(&mut fa);
    fft.process(&mut fb);
    for (x, y) in fa.iter_mut().zip(fb.iter()) {
        *x = x.conj() * *y;
    }
    ifft.process(&mut fa);
    let pa = energy_prefix(&aa);
    let pb = energy_prefix(&bb);
    // `lag` indexes B as `b[a_index + lag]`. A clip that begins deep inside
    // the reference therefore has a large negative lag, bounded by A's
    // length (not B's length).
    let lo = window.map(|w| w.0).unwrap_or(-(a.len() as isize - 1));
    let hi = window.map(|w| w.1).unwrap_or(b.len() as isize - 1);
    let mut best = Peak {
        lag: 0,
        score: -2.0,
    };
    // A tiny overlap at either end can have a deceptively high normalized
    // score. For clip-within-recording sync, require enough shared program
    // material to make the peak meaningful.
    let min_overlap = (a.len().min(b.len()) / 2).max(rate as usize / 10).max(8);
    for lag in lo..=hi {
        let start_a = 0.max(-lag) as usize;
        let end_signed = (a.len() as isize).min(b.len() as isize - lag);
        if end_signed <= 0 {
            continue;
        }
        let end_a = end_signed as usize;
        if end_a <= start_a || end_a - start_a < min_overlap {
            continue;
        }
        let overlap_energy_a = pa[end_a] - pa[start_a];
        let b_start = (start_a as isize + lag) as usize;
        let b_end = (end_a as isize + lag) as usize;
        let overlap_energy_b = pb[b_end] - pb[b_start];
        if overlap_energy_a <= 1e-12 || overlap_energy_b <= 1e-12 {
            continue;
        }
        let raw = fa[(lag.rem_euclid(n as isize)) as usize].re / n as f64;
        let score = raw / (overlap_energy_a * overlap_energy_b).sqrt();
        if score > best.score {
            best = Peak { lag, score };
        }
    }
    let _ = rate;
    if best.score < -1.0 {
        Peak { lag: 0, score: 0.0 }
    } else {
        best
    }
}

fn next_peak(a: &[f32], b: &[f32], rate: u32, selected: isize, selected_score: f64) -> f64 {
    let aa = demean(a);
    let bb = demean(b);
    let n = (aa.len() + bb.len()).next_power_of_two();
    let mut fa = vec![Complex::ZERO; n];
    let mut fb = vec![Complex::ZERO; n];
    for (d, &v) in fa.iter_mut().zip(aa.iter()) {
        d.re = v as f64;
    }
    for (d, &v) in fb.iter_mut().zip(bb.iter()) {
        d.re = v as f64;
    }
    let mut p = FftPlanner::<f64>::new();
    let f = p.plan_fft_forward(n);
    let i = p.plan_fft_inverse(n);
    f.process(&mut fa);
    f.process(&mut fb);
    for (x, y) in fa.iter_mut().zip(fb.iter()) {
        *x = x.conj() * *y;
    }
    i.process(&mut fa);
    let pa = energy_prefix(&aa);
    let pb = energy_prefix(&bb);
    let exclude = rate as isize;
    let min_overlap = (a.len().min(b.len()) / 2).max(rate as usize / 10).max(8);
    let mut result = 0.0;
    for lag in -(a.len() as isize - 1)..=b.len() as isize - 1 {
        if (lag - selected).abs() <= exclude {
            continue;
        }
        let sa = 0.max(-lag) as usize;
        let end_signed = (a.len() as isize).min(b.len() as isize - lag);
        if end_signed <= 0 {
            continue;
        }
        let ea = end_signed as usize;
        if ea <= sa || ea - sa < min_overlap {
            continue;
        }
        let b_start = sa as isize + lag;
        let b_end = ea as isize + lag;
        if b_start < 0 || b_end < 0 || b_end as usize > bb.len() {
            continue;
        }
        let eb = pb[b_end as usize] - pb[b_start as usize];
        let ea_energy = pa[ea] - pa[sa];
        if ea_energy <= 1e-12 || eb <= 1e-12 {
            continue;
        }
        let score = fa[lag.rem_euclid(n as isize) as usize].re / n as f64 / (ea_energy * eb).sqrt();
        if score > result {
            result = score;
        }
    }
    let _ = selected_score;
    result
}

fn demean(a: &[f32]) -> Vec<f32> {
    let mean = a.iter().map(|v| *v as f64).sum::<f64>() / a.len() as f64;
    a.iter().map(|v| (*v as f64 - mean) as f32).collect()
}
fn energy_prefix(a: &[f32]) -> Vec<f64> {
    let mut p = vec![0.0];
    for &v in a {
        p.push(p.last().copied().unwrap() + v as f64 * v as f64);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shifted_signal_has_positive_delay() {
        let mut a = vec![0f32; 12_000];
        let mut state = 0x1234_5678u32;
        for value in a.iter_mut().take(11_000).skip(1000) {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *value = (state as i32 as f32) / i32::MAX as f32;
        }
        let mut b = vec![0f32; 12_000];
        let shift = 700;
        for i in 0..a.len() - shift {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            b[i + shift] = a[i] + (state as i32 as f32) / i32::MAX as f32 * 0.05;
        }
        let r = align_audio(&a, &b, 8000);
        assert!(
            (r.offset_seconds - shift as f64 / 8000.0).abs() < 0.01,
            "{r:?}"
        );
        assert!(r.normalized_peak > 0.35);
    }

    #[test]
    fn clipped_signal_has_negative_delay() {
        let mut a = vec![0f32; 12_000];
        let mut state = 0x9abc_def0u32;
        for value in &mut a {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *value = state as i32 as f32 / i32::MAX as f32;
        }
        let shift = 2_300;
        let b = a[shift..shift + 7_000].to_vec();
        let r = align_audio(&a, &b, 2_000);
        assert!(
            (r.offset_seconds + shift as f64 / 2_000.0).abs() < 0.01,
            "{r:?}"
        );
        assert!(r.normalized_peak > 0.9, "{r:?}");
    }

    #[test]
    #[ignore = "decodes the large supplied recordings with FFmpeg"]
    fn supplied_insta360_audio_maps_export_to_raw_timeline() {
        use crate::{FfmpegConfig, discover, extract_mono_pcm};
        use std::path::Path;

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let raw = root.join("LRV_20260830_124108_01_017.lrv");
        let export = root.join("VID_20260830_124108_00_017.mp4");
        if !raw.exists() || !export.exists() {
            return;
        }
        let tools = discover(&FfmpegConfig::default()).unwrap();
        let reference = extract_mono_pcm(&tools, raw, 2_000).unwrap();
        let candidate = extract_mono_pcm(&tools, export, 2_000).unwrap();
        let result = align_audio(&reference, &candidate, 2_000);
        eprintln!("supplied recording alignment: {result:?}");
        let source_offset = -result.offset_seconds;
        assert!((78.5..79.5).contains(&source_offset), "{result:?}");
        assert!(result.normalized_peak > 0.35, "{result:?}");
    }
}
