//! Dekod et bilde slik derive gjør (`imageio::open_oriented`: innholdsbasert format,
//! libjpeg-ekvivalent JPEG, EXIF-orientering) og lagre pikslene tapsfritt som PNG.
//! Brukes av `tools/golden/jpeg_variants.py` for å sammenligne med Pillow bit for bit.
//!
//!     cargo run --release -p fotomaxxer-core --example decode_rgb -- inn.jpg ut.png

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let [_, src, out] = args.as_slice() else {
        eprintln!("bruk: decode_rgb <inn> <ut.png>");
        return ExitCode::from(2);
    };
    match fotomaxxer_core::imageio::open_oriented(Path::new(src)) {
        Ok(img) => match img.save(out) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(1)
            }
        },
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}
