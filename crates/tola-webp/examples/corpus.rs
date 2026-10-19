use std::error::Error;
use std::path::PathBuf;

#[derive(Clone, Copy)]
enum Pattern {
    Gray,
    Gradient,
    Checker,
    Noise,
    NoiseAlpha,
}

impl Pattern {
    fn name(self) -> &'static str {
        match self {
            Self::Gray => "gray",
            Self::Gradient => "gradient",
            Self::Checker => "checker",
            Self::Noise => "noise",
            Self::NoiseAlpha => "noise-alpha",
        }
    }

    fn rgba(self, width: u32, height: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        let mut random = 0x1234_5678u32 ^ (width * 65_537 + height);
        for y in 0..height {
            for x in 0..width {
                random ^= random << 13;
                random ^= random >> 17;
                random ^= random << 5;
                let pixel = match self {
                    Self::Gray => [128, 128, 128, 255],
                    Self::Gradient => [
                        (x * 255 / (width - 1).max(1)) as u8,
                        (y * 255 / (height - 1).max(1)) as u8,
                        ((x + y) * 255 / (width + height - 2).max(1)) as u8,
                        255,
                    ],
                    Self::Checker => {
                        let value = if (x / 2 + y / 2) % 2 == 0 { 255 } else { 0 };
                        [
                            value,
                            255 - value,
                            value,
                            [0, 128, 255][((x + y) % 3) as usize],
                        ]
                    }
                    Self::Noise | Self::NoiseAlpha => [
                        random as u8,
                        (random >> 8) as u8,
                        (random >> 16) as u8,
                        if matches!(self, Self::NoiseAlpha) {
                            (random >> 24) as u8
                        } else {
                            255
                        },
                    ],
                };
                rgba.extend_from_slice(&pixel);
            }
        }
        rgba
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let output = PathBuf::from(arguments.next().ok_or("usage: corpus OUTPUT_DIRECTORY")?);
    if arguments.next().is_some() {
        return Err("usage: corpus OUTPUT_DIRECTORY".into());
    }
    std::fs::create_dir_all(&output)?;
    let mut count = 0;
    for (width, height) in [
        (1, 1),
        (1, 65),
        (65, 1),
        (2, 3),
        (3, 5),
        (15, 17),
        (16, 16),
        (17, 19),
        (32, 33),
        (65, 65),
    ] {
        for pattern in [
            Pattern::Gray,
            Pattern::Gradient,
            Pattern::Checker,
            Pattern::Noise,
            Pattern::NoiseAlpha,
        ] {
            let rgba = pattern.rgba(width, height);
            for quality in [0, 25, 50, 75, 90, 99, 100] {
                let webp = tola_webp::encode_rgba(&rgba, width, height, quality)?;
                let filename = format!("{width}x{height}-{}-q{quality:03}.webp", pattern.name());
                std::fs::write(output.join(filename), webp)?;
                count += 1;
            }
        }
    }
    println!("{count} WebP files");
    Ok(())
}
