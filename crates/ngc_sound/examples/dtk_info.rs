//! How long `.dtk` music runs and how loud it is, from its first seconds.
//!
//! `cargo run -p ngc_sound --example dtk_info -- FILE.dtk...`
use ngc_sound::Dtk;

fn main() {
    for path in std::env::args().skip(1) {
        let dtk = Dtk::new(std::fs::read(&path).unwrap());
        let seconds = dtk.seconds();
        // Ten seconds of stereo, after the first five.
        let samples: Vec<i32> = dtk
            .skip(5 * 48_000 * 2)
            .take(10 * 48_000 * 2)
            .map(i32::from)
            .collect();
        let n = samples.len().max(1) as f64;
        let rms = (samples.iter().map(|&s| f64::from(s * s)).sum::<f64>() / n).sqrt();
        // Neighbouring samples of one channel: music changes smoothly.
        let step = samples
            .windows(3)
            .map(|w| f64::from((w[2] - w[0]).abs()))
            .sum::<f64>()
            / n;
        println!("{path}: {seconds:.0} s, rms {rms:.0}, mean step {step:.0}");
    }
}
