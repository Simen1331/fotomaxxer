//! Dekod mange bilder med én valgt dekoder og skriv rå RGB til stdout, for bit-for-bit-
//! sammenligning med OpenCV/Pillow i `tools/golden/compare_decode.py`.
//!
//!     ls derived/*_1024.jpg | decode_stream <jpegdec|image|open_rgb>
//!
//! Filstiene leses (én per linje) fra stdin før noe skrives. Per fil skrives bredde og
//! høyde (`u32` LE) og så `bredde × høyde × 3` byte RGB; 0 × 0 betyr at dekodingen feilet.
//!
//! - `jpegdec`: [`fotomaxxer_core::jpegdec::decode`] (libjpeg-turbo-ekvivalent).
//! - `image`: `image`-craten (zune-jpeg), slik `imageio::open_rgb` dekodet JPEG før.
//! - `open_rgb`: [`fotomaxxer_core::imageio::open_rgb`] slik den er nå (ansiktsløypa).

use std::io::{Read, Write};
use std::path::Path;
use std::process::ExitCode;

use image::RgbImage;

fn decode(mode: &str, path: &Path) -> Option<RgbImage> {
    match mode {
        "jpegdec" => fotomaxxer_core::jpegdec::decode(&std::fs::read(path).ok()?).ok()?,
        "image" => image::open(path).ok().map(|i| i.to_rgb8()),
        "open_rgb" => fotomaxxer_core::imageio::open_rgb(path).ok(),
        _ => None,
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let [_, mode] = args.as_slice() else {
        eprintln!("bruk: decode_stream <jpegdec|image|open_rgb>  (filstier på stdin)");
        return ExitCode::from(2);
    };
    if !matches!(mode.as_str(), "jpegdec" | "image" | "open_rgb") {
        eprintln!("ukjent dekoder: {mode}");
        return ExitCode::from(2);
    }
    let mut list = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut list) {
        eprintln!("{e}");
        return ExitCode::from(1);
    }
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for line in list.lines().filter(|l| !l.is_empty()) {
        let img = decode(mode, Path::new(line));
        let (w, h) = img.as_ref().map_or((0, 0), |i| i.dimensions());
        let written = out
            .write_all(&w.to_le_bytes())
            .and_then(|()| out.write_all(&h.to_le_bytes()))
            .and_then(|()| img.map_or(Ok(()), |i| out.write_all(i.as_raw())));
        if let Err(e) = written {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    }
    match out.flush() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}
