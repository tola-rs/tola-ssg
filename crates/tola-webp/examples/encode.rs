use std::error::Error;
use std::hint::black_box;
use std::time::Instant;

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if !(2..=4).contains(&arguments.len()) {
        return Err("usage: encode SOURCE OUTPUT.webp [QUALITY=75] [RUNS=5]".into());
    }
    let quality: u8 = arguments
        .get(2)
        .map(|value| value.to_str().ok_or("quality must be an integer"))
        .transpose()?
        .map(str::parse)
        .transpose()?
        .unwrap_or(75);
    let runs: usize = arguments
        .get(3)
        .map(|value| value.to_str().ok_or("runs must be an integer"))
        .transpose()?
        .map(str::parse)
        .transpose()?
        .unwrap_or(5);
    if quality > 100 || runs == 0 {
        return Err("quality must be 0..=100 and runs must be positive".into());
    }

    let source = image::open(&arguments[0])?.to_rgba8();
    let encode =
        || tola_webp::encode_rgba(source.as_raw(), source.width(), source.height(), quality);
    let webp = encode()?;
    let mut durations = Vec::with_capacity(runs);
    for _ in 0..runs {
        let started = Instant::now();
        let encoded = encode()?;
        durations.push(started.elapsed());
        assert_eq!(black_box(&encoded), &webp);
    }
    durations.sort_unstable();
    std::fs::write(&arguments[1], &webp)?;
    println!(
        "{}x{} q{}: {} bytes, median {} us ({} runs)",
        source.width(),
        source.height(),
        quality,
        webp.len(),
        durations[runs / 2].as_micros(),
        runs,
    );
    Ok(())
}
