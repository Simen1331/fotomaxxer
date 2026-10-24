//! Lesing og skriving av bilder — erstatter Pillow + pillow-heif.
//!
//! Formatet gjenkjennes på **innholdet**, som Pillow gjør, ikke på filendelsen: en HEIC
//! med `.jpg`-endelse leses som HEIC. PNG (og GIF/BMP/WebP/TIFF) dekodes med `image`,
//! HEIC/HEIF med libheif (som selv anvender containerens rotasjon/speiling). JPEG dekodes
//! med [`crate::jpegdec`] (samme piksler som libjpeg-turbo i både Pillow og OpenCV), med
//! `image` som reserve for varianter den ikke dekker.
//!
//! [`open_oriented`] gir det samme som Pythons
//! `ImageOps.exif_transpose(Image.open(p)).convert("RGB")`:
//! samme modus-konvertering (alfa droppes uten kompositt, 16-bit som Pillow), samme
//! orienteringskilde (EXIF IFD0, ellers XMP `tiff:Orientation`) og de samme feilene der
//! Pillow/libjpeg gir opp (avkuttet fil, ukjent markør) — der `image` selv ville vært
//! mer tilgivende. Skriving av JPEG ligger i [`crate::jpegenc`].

use std::io::Cursor;
use std::path::Path;

use image::{DynamicImage, ImageFormat, ImageReader, RgbImage};
use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};

/// Pillows `MAX_IMAGE_PIXELS`; over det dobbelte nekter `Image.open` å åpne bildet.
const MAX_IMAGE_PIXELS: u64 = 1024 * 1024 * 1024 / 4 / 3;

/// Feil ved lesing/skriving. Teksten følger Pythons `"{Unntakstype}: {melding}"`, så
/// `processing_state.error` ligner det Python-versjonen lagret.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("UnidentifiedImageError: cannot identify image file '{0}'")]
    Unidentified(String),
    #[error("OSError: image file is truncated")]
    Truncated,
    /// libjpeg ga opp (`IMAGING_CODEC_BROKEN` i Pillow).
    #[error("OSError: broken data stream when reading image file{}", detail(.0))]
    Broken(Option<String>),
    #[error(
        "DecompressionBombError: Image size ({0} pixels) exceeds limit of {limit} pixels, \
         could be decompression bomb DOS attack.",
        limit = 2 * MAX_IMAGE_PIXELS
    )]
    Bomb(u64),
    /// Ugyldig TIFF-hode i EXIF-blokka (Pillows `SyntaxError: not a TIFF file`).
    #[error("SyntaxError: not a TIFF file (header {0} not valid)")]
    ExifHeader(String),
    #[error("OSError: {0}")]
    Image(#[from] image::ImageError),
    #[error("OSError: HEIF: {0}")]
    Heif(#[from] libheif_rs::HeifError),
    #[error("OSError: {0}")]
    Io(#[from] std::io::Error),
    #[error("OSError: HEIF-bildet mangler RGB-plan")]
    MissingPlane,
    /// libjpeg nekter å skrive fila (f.eks. en kommentar lengre enn én markør).
    #[error("OSError: broken data stream when writing image file")]
    EncodeBroken,
}

fn detail(d: &Option<String>) -> String {
    d.as_ref().map(|d| format!(" ({d})")).unwrap_or_default()
}

/// Bildetype slik Pillow (med pillow-heif registrert) ville identifisert den.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Jpeg,
    Png,
    Heif,
    /// Andre formater `image` kan lese (GIF, BMP, WebP, TIFF).
    Other(ImageFormat),
}

/// Gjenkjenn bildetypen fra innholdet. `None` = Pillow ville sagt «cannot identify».
pub fn sniff(data: &[u8]) -> Option<Kind> {
    if data.starts_with(b"\xFF\xD8\xFF") {
        return Some(Kind::Jpeg);
    }
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(Kind::Png);
    }
    // pillow-heif sin `_is_supported_heif` (+ AVIF-merkene Pillow selv godtar).
    const HEIF_BRANDS: &[&[u8; 4]] = &[
        b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"hevm", b"hevs", b"mif1", b"msf1",
        b"avif", b"avis",
    ];
    if data.len() >= 12 && &data[4..8] == b"ftyp" && HEIF_BRANDS.iter().any(|b| &data[8..12] == *b)
    {
        return Some(Kind::Heif);
    }
    match image::guess_format(data) {
        Ok(f @ (ImageFormat::Gif | ImageFormat::Bmp | ImageFormat::WebP | ImageFormat::Tiff)) => {
            Some(Kind::Other(f))
        }
        _ => None,
    }
}

/// Er filendelsen en HEIF-variant (`.heic`/`.heif`, uansett store/små bokstaver)?
pub fn is_heif(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("heic") || e.eq_ignore_ascii_case("heif"))
}

/// Dekod et bilde til 8-bits RGB (uten EXIF-orientering; HEIF-transformasjoner anvendes).
///
/// Ansiktsløypa leser 1024-derivatene med denne. JPEG går via [`crate::jpegdec`], som gir
/// de samme pikslene som Pythons `cv2.imdecode` (OpenCV bruker libjpeg-turbo med
/// standardinnstillingene, islow-IDCT og «fancy» oppsampling, akkurat som Pillow): bit-like
/// på alle 926 derivatene i testbiblioteket, mot maks 5 nivåer i 9 % av verdiene med
/// `image`. Derivatene har ingen EXIF, så OpenCVs EXIF-rotasjon trengs ikke. Varianter
/// `jpegdec` ikke dekker, og filer libjpeg ville gitt opp, leses som før med `image`.
pub fn open_rgb(path: &Path) -> Result<RgbImage, DecodeError> {
    let data = std::fs::read(path)?;
    match sniff(&data) {
        Some(Kind::Heif) => decode_heif(&data),
        Some(Kind::Jpeg) => match crate::jpegdec::decode(&data) {
            Ok(Some(img)) => Ok(img),
            Ok(None) | Err(_) => Ok(read_image(&data, None)?.to_rgb8()),
        },
        Some(_) => Ok(read_image(&data, None)?.to_rgb8()),
        None => Err(DecodeError::Unidentified(path.display().to_string())),
    }
}

/// `ImageOps.exif_transpose(Image.open(path)).convert("RGB")` fra Python-versjonen.
pub fn open_oriented(path: &Path) -> Result<RgbImage, DecodeError> {
    open_for_derive(path).map(|o| o.image)
}

/// Et åpnet bilde slik derive trenger det.
pub struct Opened {
    /// Riktig orientert RGB.
    pub image: RgbImage,
    /// `im.info["comment"]`, som Pillow skriver som `COM` i JPEG-ene den lagrer.
    pub comment: Option<Vec<u8>>,
}

/// Som [`open_oriented`], pluss kommentaren Pillow ville tatt med videre.
pub fn open_for_derive(path: &Path) -> Result<Opened, DecodeError> {
    let data = std::fs::read(path)?;
    let kind = sniff(&data).ok_or_else(|| DecodeError::Unidentified(path.display().to_string()))?;
    if let Ok(size) = imagesize::blob_size(&data) {
        let pixels = (size.width.max(1) as u64) * (size.height.max(1) as u64);
        if pixels > 2 * MAX_IMAGE_PIXELS {
            return Err(DecodeError::Bomb(pixels));
        }
    }
    let (img, meta) = match kind {
        // pillow-heif lar libheif anvende irot/imir og nullstiller EXIF-orienteringen.
        Kind::Heif => {
            return Ok(Opened {
                image: decode_heif(&data)?,
                comment: None,
            });
        }
        Kind::Jpeg => {
            use crate::jpegdec::{self, JpegError};
            let (meta, structure) = jpeg_check(&data);
            // libjpeg-ekvivalent dekoding; sjeldne varianter (aritmetisk, tapsfri, …) via
            // `image`, med strukturkontrollen som erstatning for libjpegs feil.
            let img = match jpegdec::decode(&data) {
                Ok(Some(img)) => img,
                Ok(None) => {
                    structure?;
                    pillow_rgb(read_image(&data, Some(ImageFormat::Jpeg))?, None)
                }
                Err(JpegError::Truncated) => return Err(DecodeError::Truncated),
                Err(JpegError::Broken(e)) => return Err(DecodeError::Broken(Some(e))),
            };
            (img, meta)
        }
        Kind::Png => {
            let meta = png_meta(&data);
            let depth = png_depth(&data);
            let img = pillow_rgb(read_image(&data, Some(ImageFormat::Png))?, depth);
            (img, meta)
        }
        Kind::Other(format) => {
            let meta = match format {
                ImageFormat::WebP => webp_meta(&data),
                ImageFormat::Tiff => Meta {
                    exif: Some(data.clone()),
                    ..Meta::default()
                },
                _ => Meta::default(),
            };
            (read_image(&data, Some(format))?.to_rgb8(), meta)
        }
    };
    let orientation = meta.orientation()?;
    Ok(Opened {
        image: apply_orientation(img, orientation),
        comment: meta.comment.filter(|c| !c.is_empty()),
    })
}

fn read_image(data: &[u8], format: Option<ImageFormat>) -> Result<DynamicImage, DecodeError> {
    let mut reader = ImageReader::new(Cursor::new(data));
    match format {
        Some(f) => reader.set_format(f),
        None => reader = reader.with_guessed_format()?,
    }
    // Pillows egen grense (`Bomb`) gjelder; `image` sin standard (512 MiB) er strengere.
    reader.no_limits();
    Ok(reader.decode()?)
}

fn decode_heif(data: &[u8]) -> Result<RgbImage, DecodeError> {
    let ctx = HeifContext::read_from_bytes(data)?;
    let handle = ctx.primary_image_handle()?;
    let decoded = LibHeif::new().decode(&handle, ColorSpace::Rgb(RgbChroma::Rgb), None)?;
    let plane = decoded
        .planes()
        .interleaved
        .ok_or(DecodeError::MissingPlane)?;
    let (w, h) = (plane.width, plane.height);
    // Radene kan ha padding (stride > bredde·3); kopier ut tett pakket.
    let mut buf = Vec::with_capacity((w * h * 3) as usize);
    for row in plane.data.chunks(plane.stride).take(h as usize) {
        buf.extend_from_slice(&row[..w as usize * 3]);
    }
    RgbImage::from_raw(w, h, buf).ok_or(DecodeError::MissingPlane)
}

/// PNG-ens bitdybde og fargetype fra IHDR.
fn png_depth(data: &[u8]) -> Option<(u8, u8)> {
    (data.len() >= 26 && &data[12..16] == b"IHDR").then(|| (data[24], data[25]))
}

/// `.convert("RGB")` som i Pillow. 8-bits data: alfa droppes og grå repliseres (som
/// `to_rgb8`). 16-bits PNG åpnes i Pillow med «høy byte» (`RGB;16B`, `LA;16B`), men
/// 16-bits gråtone blir `I;16`, og `I;16 → RGB` klemmer til 255 i stedet for å skalere.
fn pillow_rgb(img: DynamicImage, png: Option<(u8, u8)>) -> RgbImage {
    let (w, h) = (img.width(), img.height());
    let gray16 = png == Some((16, 0));
    let from16 = |px: &mut dyn Iterator<Item = [u16; 3]>| {
        let buf = px
            .flat_map(|p| {
                p.map(|v| {
                    if gray16 {
                        v.min(255) as u8
                    } else {
                        (v >> 8) as u8
                    }
                })
            })
            .collect();
        RgbImage::from_raw(w, h, buf).expect("bufferstørrelsen stemmer")
    };
    match img {
        DynamicImage::ImageLuma16(i) => from16(&mut i.pixels().map(|p| [p[0]; 3])),
        DynamicImage::ImageLumaA16(i) => from16(&mut i.pixels().map(|p| [p[0]; 3])),
        DynamicImage::ImageRgb16(i) => from16(&mut i.pixels().map(|p| p.0)),
        DynamicImage::ImageRgba16(i) => from16(&mut i.pixels().map(|p| [p[0], p[1], p[2]])),
        other => other.to_rgb8(),
    }
}

// ---------------------------------------------------------------------------
// Orientering (Pillows `getexif()` + `exif_transpose`)
// ---------------------------------------------------------------------------

/// Metadata Pillow ser på for orientering.
#[derive(Debug, Default)]
struct Meta {
    /// `info["exif"]`: TIFF-data, evt. med `Exif\0\0` foran.
    exif: Option<Vec<u8>>,
    /// XMP-pakka (`info["XML:com.adobe.xmp"]`/`info["xmp"]`).
    xmp: Option<Vec<u8>>,
    /// JPEG uten DPI i JFIF-hodet: Pillow leser da EXIF allerede i `_open` (for å finne
    /// DPI) og svelger en eventuell feil der. Ellers gir en ugyldig EXIF-blokk feil i
    /// `exif_transpose`.
    exif_read_at_open: bool,
    /// `info["comment"]`: JPEG `COM` (siste før SOS) eller PNG-tekst med nøkkelen
    /// `comment` (UTF-8-kodet, slik Pillow koder en `str` når den lagrer).
    comment: Option<Vec<u8>>,
}

impl Meta {
    /// `image.getexif().get(Orientation, 1)` slik `exif_transpose` ser den.
    fn orientation(&self) -> Result<Option<u16>, DecodeError> {
        if let Some(exif) = &self.exif {
            match tiff_orientation(exif) {
                Ok(Some(tag)) => return Ok(tag.as_orientation()),
                Ok(None) => {}
                // Feilen ble svelget i `_open`; `getexif()` gir da et tomt (men «lastet»)
                // resultat uten XMP-oppslag.
                Err(_) if self.exif_read_at_open => return Ok(None),
                Err(e) => return Err(e),
            }
        }
        Ok(self.xmp.as_deref().and_then(xmp_orientation))
    }
}

/// Verdien til Orientation-taggen (274) i IFD0, slik Pillow tolker den.
#[derive(Debug, Clone, Copy, PartialEq)]
enum TagValue {
    /// Tall (heltall, rasjonal eller flyttall) — første verdi.
    Number(f64),
    /// BYTE/UNDEFINED/ASCII: Pillow får `bytes`/`str`, som aldri matcher en orientering.
    Other,
}

impl TagValue {
    fn as_orientation(self) -> Option<u16> {
        match self {
            TagValue::Number(v) if v.fract() == 0.0 && (2.0..=8.0).contains(&v) => Some(v as u16),
            _ => None,
        }
    }
}

/// Orientation fra IFD0 i rå EXIF/TIFF-data, med Pillows toleranse: ugyldig TIFF-hode er
/// en feil, men avkuttede oppføringer avslutter bare lesingen (tidligere tagger beholdes).
fn tiff_orientation(data: &[u8]) -> Result<Option<TagValue>, DecodeError> {
    let mut data = data;
    while let Some(rest) = data.strip_prefix(b"Exif\0\0") {
        data = rest;
    }
    if data.is_empty() {
        return Ok(None);
    }
    const PREFIXES: [&[u8; 4]; 6] = [
        b"MM\x00\x2a",
        b"II\x2a\x00",
        b"MM\x2a\x00",
        b"II\x00\x2a",
        b"MM\x00\x2b",
        b"II\x2b\x00",
    ];
    let head = &data[..data.len().min(8)];
    if head.len() < 8 || !PREFIXES.iter().any(|p| head.starts_with(*p)) || head[2] == 0x2b {
        // (BigTIFF-hodet feiler også i Pillow, med struct.error.)
        return Err(DecodeError::ExifHeader(py_bytes_repr(head)));
    }
    let big = head[0] == b'M';
    let u16_at = |o: usize| -> Option<u16> {
        let b = data.get(o..o + 2)?;
        Some(if big {
            u16::from_be_bytes([b[0], b[1]])
        } else {
            u16::from_le_bytes([b[0], b[1]])
        })
    };
    let u32_at = |o: usize| -> Option<u32> {
        let b: [u8; 4] = data.get(o..o + 4)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    };
    let ifd = u32_at(4).unwrap_or(0) as usize;
    let Some(count) = u16_at(ifd) else {
        return Ok(None);
    };
    let mut found = None;
    for i in 0..usize::from(count) {
        let e = ifd + 2 + i * 12;
        let (Some(tag), Some(typ), Some(n)) = (u16_at(e), u16_at(e + 2), u32_at(e + 4)) else {
            break; // avkuttet: Pillow advarer og beholder det som er lest
        };
        let unit = match typ {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 | 16 => 8,
            _ => continue, // ukjent type ignoreres
        };
        let size = n as usize * unit;
        if size == 0 {
            continue;
        }
        let at = if size > 4 {
            match u32_at(e + 8) {
                Some(o) if data.len() >= o as usize + size => o as usize,
                _ => break, // `_safe_read` feiler → Pillow avbryter IFD-lesingen
            }
        } else {
            e + 8
        };
        if tag != 274 {
            continue;
        }
        let rat = |num: f64, den: f64| if den == 0.0 { f64::NAN } else { num / den };
        let v = match typ {
            1 | 2 | 7 => TagValue::Other,
            3 => TagValue::Number(f64::from(u16_at(at).unwrap_or(0))),
            8 => TagValue::Number(f64::from(u16_at(at).unwrap_or(0) as i16)),
            6 => TagValue::Number(f64::from(data[at] as i8)),
            4 | 13 => TagValue::Number(f64::from(u32_at(at).unwrap_or(0))),
            9 => TagValue::Number(f64::from(u32_at(at).unwrap_or(0) as i32)),
            11 => TagValue::Number(f64::from(f32::from_bits(u32_at(at).unwrap_or(0)))),
            5 => TagValue::Number(rat(
                f64::from(u32_at(at).unwrap_or(0)),
                f64::from(u32_at(at + 4).unwrap_or(0)),
            )),
            10 => TagValue::Number(rat(
                f64::from(u32_at(at).unwrap_or(0) as i32),
                f64::from(u32_at(at + 4).unwrap_or(0) as i32),
            )),
            12 | 16 => {
                let b: [u8; 8] = data[at..at + 8].try_into().expect("8 byte");
                let raw = if big {
                    u64::from_be_bytes(b)
                } else {
                    u64::from_le_bytes(b)
                };
                TagValue::Number(if typ == 12 {
                    f64::from_bits(raw)
                } else {
                    raw as f64
                })
            }
            _ => TagValue::Other,
        };
        found = Some(v); // en senere duplikat-tagg overskriver, som i Pillow
    }
    Ok(found)
}

/// `repr(bytes)` som i Python (`b'II*\x00'`), for feiltekster.
fn py_bytes_repr(bytes: &[u8]) -> String {
    let body: String = bytes
        .iter()
        .map(|&b| match b {
            b'\\' => "\\\\".to_string(),
            b'\'' => "\\'".to_string(),
            b'\t' => "\\t".to_string(),
            b'\n' => "\\n".to_string(),
            b'\r' => "\\r".to_string(),
            0x20..=0x7e => char::from(b).to_string(),
            _ => format!("\\x{b:02x}"),
        })
        .collect();
    format!("b'{body}'")
}

/// Pillows XMP-reserve: `re.search(rb'tiff:Orientation(="|>)([0-9])', xmp)`.
fn xmp_orientation(xmp: &[u8]) -> Option<u16> {
    const KEY: &[u8] = b"tiff:Orientation";
    let mut from = 0;
    while let Some(pos) = find(&xmp[from..], KEY) {
        let rest = &xmp[from + pos + KEY.len()..];
        let digit = match rest {
            [b'=', b'"', d, ..] | [b'>', d, ..] if d.is_ascii_digit() => Some(*d),
            _ => None,
        };
        if let Some(d) = digit {
            let v = u16::from(d - b'0');
            return (2..=8).contains(&v).then_some(v);
        }
        from += pos + 1;
    }
    None
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// `exif_transpose`: Pillows `Transpose`-operasjon for orientering 2–8.
pub fn apply_orientation(img: RgbImage, orientation: Option<u16>) -> RgbImage {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let src = img.as_raw();
    // `f(x, y)` = kildepikselens indeks for utpiksel (x, y).
    match orientation {
        Some(2) => remap(src, w, h, |x, y| y * w + (w - 1 - x)), // FLIP_LEFT_RIGHT
        Some(3) => remap(src, w, h, |x, y| (h - 1 - y) * w + (w - 1 - x)), // ROTATE_180
        Some(4) => remap(src, w, h, |x, y| (h - 1 - y) * w + x), // FLIP_TOP_BOTTOM
        Some(5) => remap(src, h, w, |x, y| x * w + y),           // TRANSPOSE
        Some(6) => remap(src, h, w, |x, y| (h - 1 - x) * w + y), // ROTATE_270 (90° med klokka)
        Some(7) => remap(src, h, w, |x, y| (h - 1 - x) * w + (w - 1 - y)), // TRANSVERSE
        Some(8) => remap(src, h, w, |x, y| x * w + (w - 1 - y)), // ROTATE_90 (mot klokka)
        _ => img,
    }
}

/// Bygg et `ow × oh`-bilde der utpiksel (x, y) hentes fra kildepiksel `f(x, y)`. Går i
/// fliser på 32×32, så både lesing og skriving holder seg i hurtigbufferet.
fn remap(src: &[u8], ow: usize, oh: usize, f: impl Fn(usize, usize) -> usize) -> RgbImage {
    const T: usize = 32;
    let mut out = vec![0u8; ow * oh * 3];
    for ty in (0..oh).step_by(T) {
        for tx in (0..ow).step_by(T) {
            for y in ty..(ty + T).min(oh) {
                let row = &mut out[(y * ow + tx) * 3..(y * ow + (tx + T).min(ow)) * 3];
                for (x, px) in (tx..).zip(row.as_chunks_mut::<3>().0) {
                    let i = f(x, y) * 3;
                    px.copy_from_slice(&src[i..i + 3]);
                }
            }
        }
    }
    RgbImage::from_raw(ow as u32, oh as u32, out).expect("bufferstørrelsen stemmer")
}

// ---------------------------------------------------------------------------
// JPEG: metadata og libjpeg-ekvivalent integritetssjekk
// ---------------------------------------------------------------------------

/// Gå gjennom JPEG-markørene: EXIF/XMP/JFIF hentes slik Pillows `_open` gjør (før første
/// SOS), og strukturen sjekkes omtrent slik libjpeg leser den.
///
/// Strukturresultatet brukes bare når `image` (zune-jpeg) må dekode: den fyller ut
/// avkuttede og korrupte filer i stillhet, mens Pillow feiler når fila slutter midt i
/// bildedataene («image file is truncated») og libjpeg gir opp ved ukjente markører
/// («broken data stream»). [`crate::jpegdec`] avgjør dette selv, og mer presist.
fn jpeg_check(data: &[u8]) -> (Meta, Result<(), DecodeError>) {
    let mut meta = Meta::default();
    let mut jfif_dpi = false;
    let result = jpeg_walk(data, &mut meta, &mut jfif_dpi);
    meta.exif_read_at_open = !jfif_dpi;
    (meta, result)
}

fn jpeg_walk(data: &[u8], meta: &mut Meta, jfif_dpi: &mut bool) -> Result<(), DecodeError> {
    let mut pos = 2; // etter SOI
    let (mut saw_sof, mut sof_components, mut progressive) = (false, 0u8, false);
    let (mut scans, mut multi_scan) = (0u32, false);
    let broken = |m: String| Err(DecodeError::Broken(Some(m)));

    loop {
        // `next_marker`: hopp over søppel og fyll-0xFF; FF00 er ikke en markør.
        let marker = loop {
            while pos < data.len() && data[pos] != 0xFF {
                pos += 1;
            }
            while pos < data.len() && data[pos] == 0xFF {
                pos += 1;
            }
            match data.get(pos) {
                None => break None,
                Some(0) => pos += 1,
                Some(&m) => {
                    pos += 1;
                    break Some(m);
                }
            }
        };
        let Some(marker) = marker else {
            // Fila slutter før EOI. Med ett enkelt scan har libjpeg allerede levert alle
            // linjene, og Pillow godtar at `finish_decompress` mangler resten.
            return if scans > 0 && !multi_scan {
                Ok(())
            } else {
                Err(DecodeError::Truncated)
            };
        };
        let segment = |pos: usize| -> Result<&[u8], DecodeError> {
            let len = data
                .get(pos..pos + 2)
                .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])))
                .ok_or(DecodeError::Truncated)?;
            if len < 2 {
                return Err(DecodeError::Broken(Some(format!(
                    "ugyldig segmentlengde {len}"
                ))));
            }
            data.get(pos + 2..pos + len).ok_or(DecodeError::Truncated)
        };
        match marker {
            0xD8 => return broken("to SOI-markører".into()),
            0xD9 => return Ok(()),
            0xD0..=0xD7 | 0x01 => {}
            0xC0..=0xC3 | 0xC9..=0xCB => {
                let s = segment(pos)?;
                if saw_sof {
                    return broken("to SOF-markører".into());
                }
                saw_sof = true;
                sof_components = s.get(5).copied().unwrap_or(0);
                progressive = matches!(marker, 0xC2 | 0xCA);
                pos += 2 + s.len();
            }
            0xC5..=0xC8 | 0xCD..=0xCF => {
                return broken(format!("SOF-type 0x{marker:02x} støttes ikke"));
            }
            0xDA => {
                let s = segment(pos)?;
                if !saw_sof {
                    return broken("SOS før SOF".into());
                }
                let comps = s.first().copied().unwrap_or(0);
                multi_scan |= progressive || comps < sof_components;
                scans += 1;
                pos += 2 + s.len();
                // Entropikodede data fram til neste ekte markør.
                loop {
                    match data.get(pos) {
                        None => return Err(DecodeError::Truncated),
                        Some(0xFF) => match data.get(pos + 1) {
                            None => return Err(DecodeError::Truncated),
                            Some(0x00 | 0xD0..=0xD7 | 0xFF) => pos += 1,
                            Some(_) => break,
                        },
                        Some(_) => pos += 1,
                    }
                }
            }
            0xC4 | 0xCC | 0xDB | 0xDD | 0xDC | 0xE0..=0xEF | 0xFE => {
                let s = segment(pos)?;
                if scans == 0 {
                    // Pillows `APP`-håndtering: første Exif-blokk (senere hektes på),
                    // siste XMP-blokk, JFIF-DPI.
                    if marker == 0xE1 && s.starts_with(b"Exif\0\0") {
                        match &mut meta.exif {
                            Some(e) => e.extend_from_slice(&s[6..]),
                            None => meta.exif = Some(s.to_vec()),
                        }
                    } else if marker == 0xE1 && s.starts_with(b"http://ns.adobe.com/xap/1.0/\0") {
                        meta.xmp = Some(s[29..].to_vec());
                    } else if marker == 0xFE {
                        meta.comment = Some(s.to_vec());
                    } else if marker == 0xE0 && s.starts_with(b"JFIF") && s.len() >= 12 {
                        *jfif_dpi = matches!(s[7], 1 | 2);
                    }
                }
                pos += 2 + s.len();
            }
            other => return broken(format!("Unsupported marker type 0x{other:02x}")),
        }
    }
}

// ---------------------------------------------------------------------------
// PNG og WebP: metadata
// ---------------------------------------------------------------------------

/// EXIF og XMP fra PNG-chunks slik Pillow fyller `info` (også chunks etter IDAT, siden
/// `derive` kaller `load()` før `exif_transpose`).
fn png_meta(data: &[u8]) -> Meta {
    let mut meta = Meta::default();
    let mut raw_profile: Option<Vec<u8>> = None;
    let mut pos = 8;
    while let Some(head) = data.get(pos..pos + 8) {
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let kind = &head[4..8];
        let Some(body) = data.get(pos + 8..pos + 8 + len) else {
            break;
        };
        pos += 12 + len;
        match kind {
            b"IEND" => break,
            b"eXIf" => meta.exif = Some([b"Exif\0\0".as_slice(), body].concat()),
            b"tEXt" | b"zTXt" | b"iTXt" => {
                let Some((key, value)) = png_text(kind, body) else {
                    continue;
                };
                match key {
                    b"XML:com.adobe.xmp" => meta.xmp = Some(value),
                    // tEXt/zTXt er latin-1 i Pillow (blir `str`), iTXt er UTF-8.
                    b"comment" if kind == b"iTXt" => {
                        meta.comment = String::from_utf8(value).ok().map(String::into_bytes)
                    }
                    b"comment" => {
                        meta.comment = Some(
                            value
                                .iter()
                                .map(|&b| char::from(b))
                                .collect::<String>()
                                .into_bytes(),
                        )
                    }
                    b"Raw profile type exif" => raw_profile = Some(value),
                    b"exif" if kind == b"tEXt" => meta.exif = Some(value),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    if meta.exif.is_none() {
        // ImageMagick-stil: "\nexif\n   lengde\nhex…" → bytes.fromhex(linje 3 og utover).
        meta.exif = raw_profile.and_then(|v| {
            let text = String::from_utf8_lossy(&v);
            let hex: String = text
                .split('\n')
                .skip(3)
                .collect::<String>()
                .chars()
                .filter(|c| !c.is_ascii_whitespace())
                .collect();
            (0..hex.len() / 2)
                .map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok())
                .collect()
        });
    }
    meta
}

/// (nøkkel, verdi) fra en tekst-chunk, dekomprimert ved behov.
fn png_text<'a>(kind: &[u8], body: &'a [u8]) -> Option<(&'a [u8], Vec<u8>)> {
    let nul = body.iter().position(|&b| b == 0)?;
    let (key, rest) = (&body[..nul], &body[nul + 1..]);
    let inflate = |b: &[u8]| miniz_oxide::inflate::decompress_to_vec_zlib(b).ok();
    let value = match kind {
        b"tEXt" => rest.to_vec(),
        b"zTXt" => inflate(rest.get(1..)?)?,
        _ => {
            // iTXt: komprimert-flagg, metode, språk\0, oversatt nøkkel\0, tekst.
            let (&flag, rest) = rest.split_first()?;
            let rest = rest.get(1..)?;
            let lang_end = rest.iter().position(|&b| b == 0)?;
            let rest = &rest[lang_end + 1..];
            let tk_end = rest.iter().position(|&b| b == 0)?;
            let text = &rest[tk_end + 1..];
            if flag != 0 {
                inflate(text)?
            } else {
                text.to_vec()
            }
        }
    };
    Some((key, value))
}

/// WebP: `EXIF`- og `XMP `-chunks i RIFF-containeren.
fn webp_meta(data: &[u8]) -> Meta {
    let mut meta = Meta::default();
    let mut pos = 12;
    while let Some(head) = data.get(pos..pos + 8) {
        let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
        let Some(body) = data.get(pos + 8..pos + 8 + len) else {
            break;
        };
        match &head[..4] {
            b"EXIF" => meta.exif = Some(body.to_vec()),
            b"XMP " => meta.xmp = Some(body.to_vec()),
            _ => {}
        }
        pos += 8 + len + (len & 1);
    }
    meta
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name)
    }

    #[test]
    fn leser_jpeg_png_og_heic_med_samme_dimensjoner() {
        for name in ["astronaut.png", "astronaut.jpg", "astronaut.heic"] {
            let img = open_rgb(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(img.dimensions(), (512, 512), "{name}");
            let img = open_oriented(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(img.dimensions(), (512, 512), "{name}");
        }
    }

    /// Ansiktsløypa skal få libjpeg-turbos piksler (som `cv2.imdecode`), ikke `image` sine.
    #[test]
    fn open_rgb_dekoder_jpeg_med_jpegdec() {
        let path = fixture("astronaut.jpg");
        let ours = open_rgb(&path).unwrap();
        let libjpeg = crate::jpegdec::decode(&std::fs::read(&path).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(ours, libjpeg);
        assert_ne!(ours, image::open(&path).unwrap().to_rgb8());
    }

    #[test]
    fn heic_ligner_png_originalen() {
        let png = open_rgb(&fixture("astronaut.png")).unwrap();
        let heic = open_rgb(&fixture("astronaut.heic")).unwrap();
        let mean_diff = png
            .as_raw()
            .iter()
            .zip(heic.as_raw())
            .map(|(a, b)| (*a as f64 - *b as f64).abs())
            .sum::<f64>()
            / png.as_raw().len() as f64;
        assert!(mean_diff < 8.0, "snittavvik {mean_diff}");
    }

    #[test]
    fn heif_gjenkjennes_uavhengig_av_store_bokstaver() {
        assert!(is_heif(Path::new("a/IMG_1.HEIC")));
        assert!(is_heif(Path::new("b.heif")));
        assert!(!is_heif(Path::new("c.jpg")));
    }

    #[test]
    fn format_gjenkjennes_paa_innhold_ikke_endelse() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = tmp.path().join("egentlig-heic.jpg");
        std::fs::copy(fixture("astronaut.heic"), &fake).unwrap();
        assert_eq!(sniff(&std::fs::read(&fake).unwrap()), Some(Kind::Heif));
        assert_eq!(open_oriented(&fake).unwrap().dimensions(), (512, 512));
        assert_eq!(open_rgb(&fake).unwrap().dimensions(), (512, 512));

        let txt = tmp.path().join("tekst.png");
        std::fs::write(&txt, "ikke et bilde").unwrap();
        let err = open_oriented(&txt).unwrap_err().to_string();
        assert!(
            err.starts_with("UnidentifiedImageError: cannot identify image file"),
            "{err}"
        );
    }

    /// 3×2-mønster med verdi `x + 10y`; fasit fra Pillow 12.2 (`Image.transpose`).
    #[test]
    fn orientering_som_pillow_transpose() {
        let img = RgbImage::from_fn(3, 2, |x, y| image::Rgb([(x + 10 * y) as u8, 0, 0]));
        type Case<'a> = (u16, (u32, u32), &'a [&'a [u8]]);
        let cases: &[Case] = &[
            (2, (3, 2), &[&[2, 1, 0], &[12, 11, 10]]),
            (3, (3, 2), &[&[12, 11, 10], &[2, 1, 0]]),
            (4, (3, 2), &[&[10, 11, 12], &[0, 1, 2]]),
            (5, (2, 3), &[&[0, 10], &[1, 11], &[2, 12]]),
            (6, (2, 3), &[&[10, 0], &[11, 1], &[12, 2]]),
            (7, (2, 3), &[&[12, 2], &[11, 1], &[10, 0]]),
            (8, (2, 3), &[&[2, 12], &[1, 11], &[0, 10]]),
        ];
        for &(o, dims, rows) in cases {
            let out = apply_orientation(img.clone(), Some(o));
            assert_eq!(out.dimensions(), dims, "orientering {o}");
            for (y, row) in rows.iter().enumerate() {
                for (x, &v) in row.iter().enumerate() {
                    assert_eq!(
                        out.get_pixel(x as u32, y as u32)[0],
                        v,
                        "orientering {o} ({x},{y})"
                    );
                }
            }
        }
        assert_eq!(apply_orientation(img.clone(), Some(1)), img);
        assert_eq!(apply_orientation(img.clone(), Some(9)), img);
    }

    fn tiff_ifd0(big: bool, entries: &[(u16, u16, u32, [u8; 4])]) -> Vec<u8> {
        let mut d = Vec::new();
        let w16 = |d: &mut Vec<u8>, v: u16| {
            d.extend(if big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            })
        };
        let w32 = |d: &mut Vec<u8>, v: u32| {
            d.extend(if big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            })
        };
        d.extend(if big { b"MM\x00\x2a" } else { b"II\x2a\x00" });
        w32(&mut d, 8);
        w16(&mut d, entries.len() as u16);
        for &(tag, typ, n, val) in entries {
            w16(&mut d, tag);
            w16(&mut d, typ);
            w32(&mut d, n);
            d.extend(val);
        }
        w32(&mut d, 0);
        d
    }

    #[test]
    fn exif_orientering_tolkes_som_pillow() {
        let short6_le = tiff_ifd0(false, &[(274, 3, 1, [6, 0, 0, 0])]);
        assert_eq!(
            tiff_orientation(&short6_le).unwrap(),
            Some(TagValue::Number(6.0))
        );
        let short8_be = tiff_ifd0(true, &[(0x10f, 2, 4, *b"Abc\0"), (274, 3, 1, [0, 8, 0, 0])]);
        let with_header = [b"Exif\0\0".as_slice(), &short8_be].concat();
        assert_eq!(
            Meta {
                exif: Some(with_header),
                ..Meta::default()
            }
            .orientation()
            .unwrap(),
            Some(8)
        );
        // BYTE-type gir `bytes` i Pillow → ingen rotasjon; LONG gir heltall.
        let byte = tiff_ifd0(false, &[(274, 1, 1, [6, 0, 0, 0])]);
        assert_eq!(
            tiff_orientation(&byte).unwrap().unwrap().as_orientation(),
            None
        );
        let long = tiff_ifd0(false, &[(274, 4, 1, [3, 0, 0, 0])]);
        assert_eq!(
            tiff_orientation(&long).unwrap().unwrap().as_orientation(),
            Some(3)
        );
        // Flere verdier: Pillow tar den første.
        let two = tiff_ifd0(false, &[(274, 3, 2, [5, 0, 1, 0])]);
        assert_eq!(
            tiff_orientation(&two).unwrap().unwrap().as_orientation(),
            Some(5)
        );
        // Ugyldig TIFF-hode er en feil (som Pillows SyntaxError).
        let err = tiff_orientation(b"Exif\0\0XXXXXXXX")
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "SyntaxError: not a TIFF file (header b'XXXXXXXX' not valid)"
        );
        assert_eq!(py_bytes_repr(b"II*\0\x01'\\"), r"b'II*\x00\x01\'\\'");
        // … men i en JPEG uten JFIF-DPI har Pillow allerede svelget den ved åpning.
        let bad = Meta {
            exif: Some(b"Exif\0\0XXXXXXXX".to_vec()),
            ..Meta::default()
        };
        assert!(bad.orientation().is_err());
        let swallowed = Meta {
            exif_read_at_open: true,
            ..bad
        };
        assert_eq!(swallowed.orientation().unwrap(), None);
    }

    #[test]
    fn xmp_er_reserve_naar_exif_mangler_taggen() {
        let xmp = br#"<x:xmpmeta><rdf:Description tiff:Orientation="6"/></x:xmpmeta>"#;
        let meta = Meta {
            xmp: Some(xmp.to_vec()),
            ..Meta::default()
        };
        assert_eq!(meta.orientation().unwrap(), Some(6));
        assert_eq!(
            xmp_orientation(b"<tiff:Orientation>3</tiff:Orientation>"),
            Some(3)
        );
        assert_eq!(
            xmp_orientation(b"<tiff:Orientation>1</tiff:Orientation>"),
            None
        );
        // EXIF med taggen vinner over XMP.
        let exif = tiff_ifd0(false, &[(274, 3, 1, [1, 0, 0, 0])]);
        let both = Meta {
            exif: Some(exif),
            xmp: Some(xmp.to_vec()),
            ..Meta::default()
        };
        assert_eq!(both.orientation().unwrap(), None);
    }

    /// JPEG med EXIF-orientering 6 lagd med vår egen koder; leses rotert.
    #[test]
    fn jpeg_med_exif_orientering_roteres() {
        let img = RgbImage::from_fn(64, 32, |x, _| {
            image::Rgb([if x < 32 { 255 } else { 0 }, 0, 0])
        });
        let mut jpeg = crate::jpegenc::encode(&img, 95);
        let exif = [
            b"Exif\0\0".as_slice(),
            &tiff_ifd0(false, &[(274, 3, 1, [6, 0, 0, 0])]),
        ]
        .concat();
        let mut app1 = vec![0xFF, 0xE1];
        app1.extend(((exif.len() + 2) as u16).to_be_bytes());
        app1.extend(&exif);
        jpeg.splice(2..2, app1);
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rotert.jpg");
        std::fs::write(&path, &jpeg).unwrap();
        let out = open_oriented(&path).unwrap();
        assert_eq!(out.dimensions(), (32, 64));
        // Venstre (røde) halvdel havner øverst etter 90° med klokka.
        assert!(out.get_pixel(16, 8)[0] > 200 && out.get_pixel(16, 56)[0] < 50);
    }

    #[test]
    fn avkuttet_og_ukjent_markoer_feiler_som_libjpeg() {
        let img = RgbImage::from_fn(64, 64, |x, y| image::Rgb([x as u8 * 4, y as u8 * 4, 9]));
        let jpeg = crate::jpegenc::encode(&img, 85);
        assert!(jpeg_check(&jpeg).1.is_ok());
        // Fasit fra Pillow 12.2: mangler EOI → «image file is truncated» (bildedataene er
        // ikke avsluttet av en markør), men en markør og så EOF etter scannet er greit.
        let no_eoi = &jpeg[..jpeg.len() - 2];
        assert!(matches!(jpeg_check(no_eoi).1, Err(DecodeError::Truncated)));
        assert!(
            jpeg_check(&[no_eoi, &[0xFF, 0xC4, 0x00, 0x02]].concat())
                .1
                .is_ok()
        );
        assert!(
            jpeg_check(&[jpeg.as_slice(), b"rusk etter EOI"].concat())
                .1
                .is_ok()
        );
        // Kuttet midt i bildedataene.
        let cut = &jpeg[..jpeg.len() / 2];
        assert!(matches!(jpeg_check(cut).1, Err(DecodeError::Truncated)));
        // Ukjent markør etter bildedataene.
        let mut bad = jpeg[..jpeg.len() - 2].to_vec();
        bad.extend([0xFF, 0x78, 0xFF, 0xD9]);
        assert!(matches!(jpeg_check(&bad).1, Err(DecodeError::Broken(_))));
    }

    /// PNG med tekst-chunks bygget for hånd (IHDR først, slik `png_meta` forventer).
    fn png_with_text(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut d = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut chunk = |kind: &[u8; 4], body: &[u8]| {
            d.extend((body.len() as u32).to_be_bytes());
            d.extend(kind);
            d.extend(body);
            d.extend([0; 4]); // CRC sjekkes ikke her
        };
        chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]);
        for (kind, body) in chunks {
            chunk(kind, body);
        }
        chunk(b"IEND", &[]);
        d
    }

    #[test]
    fn kommentar_som_pillow_tar_med_videre() {
        // tEXt er latin-1 i Pillow og kodes som UTF-8 når den lagres; iTXt er UTF-8.
        let png = png_with_text(&[(b"tEXt", b"comment\0bl\xe5b\xe6r")]);
        assert_eq!(png_meta(&png).comment.unwrap(), "blåbær".as_bytes());
        let png = png_with_text(&[(b"iTXt", "comment\0\0\0\0\0æøå".as_bytes())]);
        assert_eq!(png_meta(&png).comment.unwrap(), "æøå".as_bytes());
        // Nøkkelen skiller store og små bokstaver.
        let png = png_with_text(&[(b"tEXt", b"Comment\0nei")]);
        assert_eq!(png_meta(&png).comment, None);
        // JPEG: siste COM før SOS.
        let img = RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3]));
        let mut jpeg = crate::jpegenc::encode_with_comment(&img, 85, Some(b"en"));
        jpeg.splice(2..2, [0xFF, 0xFE, 0x00, 0x05, b't', b'o', b'?']);
        let (meta, ok) = jpeg_check(&jpeg);
        assert!(ok.is_ok());
        assert_eq!(meta.comment.unwrap(), b"en");
    }

    #[test]
    fn png_16bit_og_alfa_konverteres_som_pillow() {
        let rgba = image::RgbaImage::from_pixel(2, 1, image::Rgba([10, 20, 30, 0]));
        assert_eq!(
            pillow_rgb(DynamicImage::ImageRgba8(rgba), Some((8, 6)))
                .get_pixel(0, 0)
                .0,
            [10, 20, 30]
        );
        let rgb16 = image::ImageBuffer::from_pixel(1, 1, image::Rgb([0x1234u16, 0xFFFF, 0x00FF]));
        assert_eq!(
            pillow_rgb(DynamicImage::ImageRgb16(rgb16), Some((16, 2)))
                .get_pixel(0, 0)
                .0,
            [0x12, 0xFF, 0x00]
        );
        let gray16 = image::ImageBuffer::from_pixel(1, 1, image::Luma([0x0080u16]));
        assert_eq!(
            pillow_rgb(DynamicImage::ImageLuma16(gray16.clone()), Some((16, 0)))
                .get_pixel(0, 0)
                .0,
            [0x80; 3]
        );
        let gray16_hi = image::ImageBuffer::from_pixel(1, 1, image::Luma([0x1234u16]));
        assert_eq!(
            pillow_rgb(DynamicImage::ImageLuma16(gray16_hi), Some((16, 0)))
                .get_pixel(0, 0)
                .0,
            [255; 3]
        );
    }
}
