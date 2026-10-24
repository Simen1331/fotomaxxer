//! EXIF-uttrekk uten ExifTool (erstatter pyexiftool + `exiftool -G -n`).
//!
//! Henter `taken_at`/`year`/`season`, kamera, dimensjoner og GPS i desimalgrader med
//! **samme verdier som Python-versjonen fikk fra ExifTool**: samme feltprioritet,
//! samme datoparsing, og GPS regnet som ExifTool gjør det (hver rasjonal avrundet til
//! 10 signifikante siffer, grader skrevet ut med 15). EXIF-feil blokkerer aldri ingest —
//! da blir feltene bare `None`.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use exif::{Exif, In, Tag, Value};
use imagesize::ImageType;

/// Normaliserte EXIF-felter for én fil (tilsvarer dict-en fra `exif._parse`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExifData {
    pub taken_at: Option<String>,
    pub year: Option<i32>,
    pub season: Option<&'static str>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
}

/// Måned → årstid (nordlig halvkule), per TB.
pub fn season_for_month(month: u32) -> Option<&'static str> {
    match month {
        12 | 1 | 2 => Some("vinter"),
        3..=5 => Some("vår"),
        6..=8 => Some("sommer"),
        9..=11 => Some("høst"),
        _ => None,
    }
}

/// Les og normaliser EXIF for `path`. Ulesbar fil → alle felter `None`.
pub fn extract(path: &Path) -> ExifData {
    let Ok(file) = File::open(path) else {
        return ExifData::default();
    };
    let mut reader = BufReader::new(file);
    let mut header = [0u8; 64];
    let n = reader.read(&mut header).unwrap_or(0);
    let kind = imagesize::image_type(&header[..n]).ok();
    let mut dims = || {
        reader.seek(SeekFrom::Start(0)).ok()?;
        imagesize::reader_size(&mut reader).ok()
    };
    // ExifTool: «File:ImageWidth» finnes for JPEG (SOF) og HEIF (ispe); PNG har sin egen.
    let (container, png) = match kind {
        Some(ImageType::Jpeg | ImageType::Heif(_)) => (dims(), None),
        Some(ImageType::Png) => (None, dims()),
        _ => (None, None),
    };
    let exif = File::open(path).ok().and_then(|f| {
        exif::Reader::new()
            .read_from_container(&mut BufReader::new(f))
            .ok()
    });
    parse(exif.as_ref(), container, png)
}

fn parse(
    exif: Option<&Exif>,
    container: Option<imagesize::ImageSize>,
    png: Option<imagesize::ImageSize>,
) -> ExifData {
    let ascii = |tag| exif.and_then(|e| ascii_field(e, tag));
    let uint = |tag| exif.and_then(|e| uint_field(e, tag));

    // `m.get(DateTimeOriginal) or m.get(CreateDate)`: første ikke-tomme verdi parses;
    // er den ugyldig, blir datoen `None` (ingen fallback til neste).
    let taken_at = [Tag::DateTimeOriginal, Tag::DateTimeDigitized]
        .into_iter()
        .filter_map(ascii)
        .find(|v| !v.is_empty())
        .and_then(|v| parse_datetime(&v));
    let (year, season) = match &taken_at {
        Some(t) => {
            let year = t[..4].parse().ok();
            let month: u32 = t[5..7].parse().unwrap_or(0);
            (year, season_for_month(month))
        }
        None => (None, None),
    };

    let width = uint(Tag::ImageWidth)
        .or(container.map(|d| d.width as i64))
        .or(uint(Tag::PixelXDimension))
        .or(png.map(|d| d.width as i64));
    let height = uint(Tag::ImageLength)
        .or(container.map(|d| d.height as i64))
        .or(uint(Tag::PixelYDimension))
        .or(png.map(|d| d.height as i64));

    let clean = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

    ExifData {
        taken_at,
        year,
        season,
        width,
        height,
        camera_make: clean(ascii(Tag::Make)),
        camera_model: clean(ascii(Tag::Model)),
        gps_lat: exif.and_then(|e| gps_coordinate(e, Tag::GPSLatitude, Tag::GPSLatitudeRef, b'S')),
        gps_lon: exif
            .and_then(|e| gps_coordinate(e, Tag::GPSLongitude, Tag::GPSLongitudeRef, b'W')),
    }
}

/// Felt fra hoved-IFD-en (med Exif-/GPS-under-IFD-er), ellers fra miniatyr-IFD-en —
/// samme prioritet som ExifTool bruker når en tag finnes i flere IFD-er.
fn field(exif: &Exif, tag: Tag) -> Option<&exif::Field> {
    exif.get_field(tag, In::PRIMARY)
        .or_else(|| exif.get_field(tag, In::THUMBNAIL))
}

/// ASCII-verdi som ExifTool gir den: avkuttet ved første NUL.
fn ascii_field(exif: &Exif, tag: Tag) -> Option<String> {
    match &field(exif, tag)?.value {
        Value::Ascii(parts) => {
            let first = parts.first()?;
            let end = first.iter().position(|&b| b == 0).unwrap_or(first.len());
            Some(String::from_utf8_lossy(&first[..end]).into_owned())
        }
        _ => None,
    }
}

fn uint_field(exif: &Exif, tag: Tag) -> Option<i64> {
    field(exif, tag)?.value.get_uint(0).map(i64::from)
}

/// `"2020:06:15 14:30:00"` → `"2020-06-15T14:30:00"`. Ugyldig/tom → `None`
/// (som `datetime.strptime(head, "%Y:%m:%d %H:%M:%S")` på de første 19 tegnene).
pub fn parse_datetime(value: &str) -> Option<String> {
    let head: String = value.trim().chars().take(19).collect();
    let b = head.as_bytes();
    if b.len() != 19
        || b[4] != b':'
        || b[7] != b':'
        || b[10] != b' '
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let s = &head[r];
        s.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| s.parse().ok())?
    };
    let (y, mo, d, h, mi, s) = (
        num(0..4)?,
        num(5..7)?,
        num(8..10)?,
        num(11..13)?,
        num(14..16)?,
        num(17..19)?,
    );
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if y < 1 || d < 1 || d > days || h > 23 || mi > 59 || s > 59 {
        return None;
    }
    Some(format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}"))
}

/// Desimalgrader som ExifTools `Composite:GPSLatitude` med `-n`.
///
/// ExifTool avrunder hver rasjonal til 10 signifikante siffer (`GetRational64u`),
/// regner `d + (m + s/60)/60` (`GPS::ToDegrees`), bytter fortegn ved S/W, og JSON-
/// utskriften av Perl-tallet gir 15 signifikante siffer. Null-nevner → ugyldig.
fn gps_coordinate(exif: &Exif, value_tag: Tag, ref_tag: Tag, negative_ref: u8) -> Option<f64> {
    let Value::Rational(parts) = &field(exif, value_tag)?.value else {
        return None;
    };
    let mut dms = [0.0f64; 3];
    for (slot, r) in dms.iter_mut().zip(parts.iter()) {
        if r.denom == 0 {
            return None;
        }
        *slot = round_sig(r.num as f64 / r.denom as f64, 10);
    }
    if parts.is_empty() {
        return None;
    }
    let deg = dms[0] + (dms[1] + dms[2] / 60.0) / 60.0;
    let negative = ascii_field(exif, ref_tag)
        .and_then(|r| r.bytes().next())
        .is_some_and(|c| c.eq_ignore_ascii_case(&negative_ref));
    let deg = round_sig(deg, 15);
    Some(if negative { -deg } else { deg })
}

/// `float(sprintf("%.{sig}g", v))`: avrund til `sig` signifikante siffer.
pub fn round_sig(v: f64, sig: usize) -> f64 {
    format!("{:.*e}", sig.saturating_sub(1), v)
        .parse()
        .unwrap_or(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dato_parses_strengt_som_strptime() {
        assert_eq!(
            parse_datetime("2020:06:15 14:30:00").as_deref(),
            Some("2020-06-15T14:30:00")
        );
        assert_eq!(
            parse_datetime(" 2020:06:15 14:30:00+02:00 ").as_deref(),
            Some("2020-06-15T14:30:00")
        );
        assert_eq!(
            parse_datetime("2024:02:29 00:00:00").as_deref(),
            Some("2024-02-29T00:00:00")
        );
        assert_eq!(parse_datetime("2023:02:29 00:00:00"), None);
        assert_eq!(parse_datetime("0000:00:00 00:00:00"), None);
        assert_eq!(parse_datetime("2020:06:15 14:30"), None);
        assert_eq!(parse_datetime(""), None);
    }

    #[test]
    fn arstider() {
        assert_eq!(season_for_month(1), Some("vinter"));
        assert_eq!(season_for_month(5), Some("vår"));
        assert_eq!(season_for_month(8), Some("sommer"));
        assert_eq!(season_for_month(11), Some("høst"));
    }

    #[test]
    fn signifikante_siffer() {
        assert_eq!(round_sig(59.959941666666666, 15), 59.9599416666667);
        assert_eq!(round_sig(1.0 / 3.0, 10), 0.3333333333);
    }

    #[test]
    fn fixtures_uten_exif_gir_bare_dimensjoner() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
        for name in ["astronaut.png", "astronaut.jpg", "astronaut.heic"] {
            let e = extract(&dir.join(name));
            assert_eq!((e.width, e.height), (Some(512), Some(512)), "{name}");
            assert_eq!(e.taken_at, None, "{name}");
        }
        assert_eq!(extract(&dir.join("finnes-ikke.jpg")), ExifData::default());
    }
}
