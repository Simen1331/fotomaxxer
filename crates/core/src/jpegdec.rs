//! JPEG-dekoder som gir de samme pikslene som Pillow (libjpeg-turbo).
//!
//! `image`/zune-jpeg bruker en annen IDCT, annen oppsampling og annen fargekonvertering
//! enn libjpeg-turbo. Avvikene er små (maks 4 nivåer), men forsterkes når thumbnailen
//! JPEG-kodes på nytt (≈ 40–49 dB mot Python). Dette er derfor en port av libjpeg-turbos
//! dekodevei slik Pillow bruker den (standardinnstillinger):
//!
//! - Huffman-dekoding, sekvensiell og progressiv (`jdhuff.c`, `jdphuff.c`), med libjpegs
//!   oppførsel ved korrupte data: nullbits etter en markør midt i dataene, «tom» MCU
//!   resten av segmentet, gjenoppretting ved restart-markører.
//! - Heltalls-IDCT (`jpeg_idct_islow`).
//! - «Fancy» oppsampling (`h2v1`, `h1v2`, `h2v2` med kontekstrader), ellers boks.
//! - YCbCr → RGB med libjpegs tabeller (`ycc_rgb_convert`); CMYK/YCCK (Adobe) som Pillow:
//!   `ycck_cmyk_convert`, inverterte kanaler (`CMYK;I`) og så `cmyk2rgb`.
//!
//! Aritmetisk koding og tapsfri JPEG gir `Ok(None)`, og kalleren faller tilbake til
//! `image`. Kjente avvik, bare i ødelagte filer: libjpegs blokkutjevning av ufullstendige
//! progressive filer etterlignes ikke, og ekstreme koeffisienter (korrupte data) kan gi
//! andre piksler enn libjpeg-turbos SIMD-IDCT, som regner med 16-bits metning.

use image::RgbImage;

use crate::simd::avx2_dispatch;

/// `jpeg_natural_order` + 16 ekstra 63-ere som vern mot korrupte løpelengder.
const NATURAL: [usize; 80] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63, 63, 63, 63, 63, 63, 63, 63, 63, 63, 63,
    63, 63, 63, 63, 63, 63,
];

/// Standardtabellene libjpeg-turbo installerer for sekvensielle filer som mangler DHT
/// (`std_huff_tables` i `jinit_huff_decoder`; progressiv dekoding gjør det ikke).
mod std_tables {
    pub const DC_LUMA: ([u8; 16], &[u8]) = (
        [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0],
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    );
    pub const DC_CHROMA: ([u8; 16], &[u8]) = (
        [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0],
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    );
    pub const AC_LUMA: ([u8; 16], &[u8]) = (
        [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d],
        &[
            0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51,
            0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1,
            0x15, 0x52, 0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18,
            0x19, 0x1a, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39,
            0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57,
            0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75,
            0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92,
            0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
            0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
            0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8,
            0xd9, 0xda, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2,
            0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
        ],
    );
    pub const AC_CHROMA: ([u8; 16], &[u8]) = (
        [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77],
        &[
            0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07,
            0x61, 0x71, 0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09,
            0x23, 0x33, 0x52, 0xf0, 0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25,
            0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38,
            0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56,
            0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74,
            0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
            0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5,
            0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba,
            0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6,
            0xd7, 0xd8, 0xd9, 0xda, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2,
            0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
        ],
    );
}

/// Hvorfor Pillow/libjpeg ville gitt opp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JpegError {
    /// Fila slutter før dekoderen har fått dataene den trenger. libjpeg venter på mer
    /// data, og Pillow gir «image file is truncated».
    Truncated,
    /// Fatal libjpeg-feil (`ERREXIT`); Pillow gir «broken data stream».
    Broken(String),
}

impl From<String> for JpegError {
    fn from(s: String) -> Self {
        JpegError::Broken(s)
    }
}

impl From<&str> for JpegError {
    fn from(s: &str) -> Self {
        JpegError::Broken(s.to_string())
    }
}

type Result<T> = std::result::Result<T, JpegError>;

/// Dekod en JPEG som Pillow gjør (`Image.open(…).convert("RGB")`).
///
/// `Ok(None)` = varianten støttes ikke her; bruk en annen dekoder. `Err` = Pillow ville
/// feilet: avkuttet fil eller fatal libjpeg-feil. Advarsler (korrupte data som libjpeg
/// erstatter med nuller) er ikke feil.
pub fn decode(data: &[u8]) -> Result<Option<RgbImage>> {
    Decoder::new(data).run()
}

// ---------------------------------------------------------------------------
// Huffman
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct RawTable {
    bits: [u8; 16],
    values: [u8; 256],
}

impl RawTable {
    fn new(bits: [u8; 16], values: &[u8]) -> Self {
        let mut v = [0u8; 256];
        v[..values.len()].copy_from_slice(values);
        Self { bits, values: v }
    }
}

/// `d_derived_tbl`: kanoniske koder for bit-for-bit-dekoding, pluss 8-bits oppslag.
struct Huffman {
    maxcode: [i32; 18],
    valoffset: [i32; 18],
    values: [u8; 256],
    /// `(lengde << 8) | verdi` for koder ≤ 8 bit, ellers 0.
    lookup: [u16; 256],
}

impl Huffman {
    /// `jpeg_make_d_derived_tbl`, med libjpegs validering (fatal ved ugyldig tabell).
    fn build(raw: &RawTable, is_dc: bool) -> Result<Self> {
        let bad = || JpegError::from("Bogus Huffman table definition");
        let mut huffsize = Vec::with_capacity(257);
        for (l, &count) in raw.bits.iter().enumerate() {
            for _ in 0..count {
                huffsize.push(l as u8 + 1);
            }
        }
        if huffsize.len() > 256 {
            return Err(bad());
        }
        let n = huffsize.len();
        let mut huffcode = vec![0u32; n];
        let (mut code, mut p) = (0u32, 0);
        let mut si = huffsize.first().copied().unwrap_or(0);
        while p < n {
            while p < n && huffsize[p] == si {
                huffcode[p] = code;
                code += 1;
                p += 1;
            }
            if u64::from(code) >= 1u64 << si {
                return Err(bad());
            }
            code <<= 1;
            si += 1;
        }
        let mut maxcode = [-1i32; 18];
        let mut valoffset = [0i32; 18];
        let mut p = 0usize;
        for l in 1..=16 {
            let count = raw.bits[l - 1] as usize;
            if count > 0 {
                valoffset[l] = p as i32 - huffcode[p] as i32;
                p += count;
                maxcode[l] = huffcode[p - 1] as i32;
            }
        }
        maxcode[17] = 0xFFFFF;
        if is_dc && raw.values[..n].iter().any(|&v| v > 15) {
            return Err(bad());
        }
        let mut lookup = [0u16; 256];
        let mut p = 0usize;
        for l in 1..=8usize {
            for _ in 0..raw.bits[l - 1] {
                let lookbits = (huffcode[p] as usize) << (8 - l);
                for e in &mut lookup[lookbits..lookbits + (1 << (8 - l))] {
                    *e = ((l as u16) << 8) | u16::from(raw.values[p]);
                }
                p += 1;
            }
        }
        Ok(Self {
            maxcode,
            valoffset,
            values: raw.values,
            lookup,
        })
    }
}

// ---------------------------------------------------------------------------
// Bitleser
// ---------------------------------------------------------------------------

/// Bitleser over entropidataene (`fill_bit_buffer`): FF00 → FF, en markør stopper
/// lesingen og etterfølgende bits er nuller. Bruker vi nullbits, settes
/// `insufficient` (libjpeg: resten av segmentet blir tomme MCU-er).
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u64,
    /// Bits i `acc` (venstrejustert i de høyeste `n` bitene av et 64-bits vindu).
    n: u32,
    /// Hvor mange av dem som er ekte (resten er nullutfylling etter markør/EOF).
    real: u32,
    /// Markøren som avsluttet dataene (posisjonen peker forbi den).
    marker: Option<u8>,
    insufficient: bool,
    /// Fila tok slutt under fylling uten noen markør (libjpeg ville ventet på mer data).
    eof: bool,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Self {
            data,
            pos,
            acc: 0,
            n: 0,
            real: 0,
            marker: None,
            insufficient: false,
            eof: false,
        }
    }

    fn fill(&mut self) {
        // Rask vei: ingen 0xFF blant de neste 8 bytene → legg inn så mange hele bytes
        // som får plass.
        if self.marker.is_none()
            && let Some(chunk) = self.data.get(self.pos..self.pos + 8)
        {
            let w = u64::from_be_bytes(chunk.try_into().expect("8 byte"));
            let x = !w;
            if x.wrapping_sub(0x0101_0101_0101_0101) & !x & 0x8080_8080_8080_8080 == 0 {
                let take = (64 - self.n) / 8;
                self.acc |= (w >> (64 - take * 8)) << (64 - self.n - take * 8);
                self.pos += take as usize;
                self.n += take * 8;
                self.real = self.n;
                return;
            }
        }
        while self.n <= 56 {
            let mut byte = 0u8;
            let mut real = false;
            if self.marker.is_none() && self.pos < self.data.len() {
                let c = self.data[self.pos];
                self.pos += 1;
                if c == 0xFF {
                    // Fyll-FF-er før en markør hoppes over; FF..FF00 er én FF-databyte.
                    let mut m = 0xFF;
                    while m == 0xFF && self.pos < self.data.len() {
                        m = self.data[self.pos];
                        self.pos += 1;
                    }
                    if m == 0 {
                        byte = 0xFF;
                        real = true;
                    } else if m != 0xFF {
                        self.marker = Some(m);
                    }
                } else {
                    byte = c;
                    real = true;
                }
            }
            if !real && self.marker.is_none() && self.pos >= self.data.len() {
                // Slutt på fila uten markør: libjpeg suspenderer her, og Pillow melder
                // avkuttet fil. Vi fyller med nullbits og lar kalleren se `eof`.
                self.marker = Some(0);
                self.eof = true;
            }
            self.acc |= u64::from(byte) << (56 - self.n);
            self.n += 8;
            if real {
                self.real = self.n;
            }
        }
    }

    #[inline]
    fn peek(&mut self, k: u32) -> u32 {
        if self.n < k {
            self.fill();
        }
        (self.acc >> (64 - k)) as u32
    }

    #[inline]
    fn consume(&mut self, k: u32) {
        if k > self.real {
            self.insufficient = true;
            self.real = 0;
        } else {
            self.real -= k;
        }
        self.acc <<= k;
        self.n -= k;
    }

    #[inline]
    fn bits(&mut self, k: u32) -> u32 {
        if k == 0 {
            return 0;
        }
        let v = self.peek(k);
        self.consume(k);
        v
    }

    /// `jpeg_huff_decode` (med 8-bits oppslag som `HUFF_DECODE`).
    #[inline]
    fn decode(&mut self, h: &Huffman) -> u8 {
        let look = self.peek(8) as usize;
        let e = h.lookup[look];
        if e != 0 {
            self.consume(u32::from(e >> 8));
            return e as u8;
        }
        let mut l = 9usize;
        let mut code = self.bits(9) as i32;
        while code > h.maxcode[l] {
            code = (code << 1) | self.bits(1) as i32;
            l += 1;
        }
        if l > 16 {
            return 0; // JWRN_HUFF_BAD_CODE: «fake a zero»
        }
        h.values[((code + h.valoffset[l]) & 0xFF) as usize]
    }

    /// `s` verdibits (`GET_BITS` + `HUFF_EXTEND`).
    #[inline]
    fn receive_extend(&mut self, s: u32) -> i32 {
        extend(self.bits(s), s)
    }

    /// Neste markør etter dataene: den som stoppet lesingen, ellers søk videre forbi
    /// eventuelle restbytes (`next_marker`). `None` = slutt på fila.
    fn next_marker(&mut self) -> Option<u8> {
        if let Some(m) = self.marker.take().filter(|&m| m != 0) {
            return Some(m);
        }
        next_marker(self.data, &mut self.pos)
    }

    /// Kast bitbufferet (ved restart).
    fn reset(&mut self) {
        self.acc = 0;
        self.n = 0;
        self.real = 0;
    }
}

/// Huffman-tabell `id` for et scan; sekvensielle filer får standardtabellen hvis 0/1 mangler.
fn huffman(
    tables: &[Option<RawTable>; 4],
    id: usize,
    dc: bool,
    progressive: bool,
) -> Result<Huffman> {
    let standard = match (dc, id) {
        _ if progressive => None,
        (true, 0) => Some(std_tables::DC_LUMA),
        (true, 1) => Some(std_tables::DC_CHROMA),
        (false, 0) => Some(std_tables::AC_LUMA),
        (false, 1) => Some(std_tables::AC_CHROMA),
        _ => None,
    };
    let raw = match tables.get(id).and_then(Option::as_ref) {
        Some(raw) => raw.clone(),
        None => match standard {
            Some((bits, values)) => RawTable::new(bits, values),
            None => return Err(format!("Huffman table 0x{id:02x} was not defined").into()),
        },
    };
    Huffman::build(&raw, dc)
}

/// `read_restart_marker` + `jpeg_resync_to_restart`: finn restart-markøren `RST{next}`
/// etter et restart-intervall, med libjpegs regler når markørene er rotet til.
fn restart(bits: &mut Bits, next: u8) {
    let rst = |n: u8| 0xD0 + (n & 7);
    bits.reset();
    let mut m = bits.next_marker();
    while let Some(mk) = m {
        if mk < 0xC0 || mk == rst(next.wrapping_sub(1)) || mk == rst(next.wrapping_sub(2)) {
            // Ugyldig markør eller en tidligere restart: hopp videre til neste markør.
            m = next_marker(bits.data, &mut bits.pos);
        } else if !(0xD0..=0xD7).contains(&mk) || mk == rst(next + 1) || mk == rst(next + 2) {
            // Annen gyldig markør eller en av de to neste restartene: la den stå, så
            // resten av segmentet blir tomt.
            bits.marker = Some(mk);
            return;
        } else {
            // Den ventede restarten (eller en langt unna): svelg den og fortsett.
            bits.marker = None;
            bits.insufficient = false;
            return;
        }
    }
    bits.marker = Some(0); // slutt på fila
}

/// `HUFF_EXTEND`: tolk `s` bits som et fortegnet tall.
#[inline]
fn extend(r: u32, s: u32) -> i32 {
    if s == 0 {
        return 0;
    }
    let r = r as i32;
    if r < (1 << (s - 1)) {
        r + (-1 << s) + 1
    } else {
        r
    }
}

/// `next_marker`: hopp over søppel og fyll-FF; FF00 er ikke en markør.
fn next_marker(data: &[u8], pos: &mut usize) -> Option<u8> {
    loop {
        while *pos < data.len() && data[*pos] != 0xFF {
            *pos += 1;
        }
        while *pos < data.len() && data[*pos] == 0xFF {
            *pos += 1;
        }
        let m = *data.get(*pos)?;
        *pos += 1;
        if m != 0 {
            return Some(m);
        }
    }
}

// ---------------------------------------------------------------------------
// Ramme og komponenter
// ---------------------------------------------------------------------------

struct Component {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    /// `downsampled_width/height`.
    dw: usize,
    dh: usize,
    /// `width_in_blocks/height_in_blocks`.
    wib: usize,
    hib: usize,
    /// Koeffisientlager: `bw × bh` blokker (utfylt til hele MCU-er).
    bw: usize,
    coefs: Vec<i16>,
    /// Kvantiseringstabellen låst ved komponentens første scan (`latch_quant_tables`).
    quant: Option<[u16; 64]>,
    /// Progressiv: kjente bits per koeffisient (`coef_bits`), -1 = ikke sendt.
    coef_bits: [i32; 64],
}

impl Component {
    fn block(&mut self, bx: usize, by: usize) -> &mut [i16] {
        let i = (by * self.bw + bx) * 64;
        &mut self.coefs[i..i + 64]
    }
}

struct Frame {
    progressive: bool,
    width: usize,
    height: usize,
    hmax: usize,
    vmax: usize,
    mcux: usize,
    mcuy: usize,
    comps: Vec<Component>,
}

struct Scan {
    /// (indeks i `frame.comps`, DC-tabell, AC-tabell)
    comps: Vec<(usize, usize, usize)>,
    ss: usize,
    se: usize,
    ah: u32,
    al: u32,
}

/// Hva som skjer etter en markør.
enum Step {
    Next,
    /// Et scan er dekodet; markøren som fulgte (eller `None` ved EOF).
    AfterScan(Option<u8>),
    Eoi,
    Unsupported,
}

struct Decoder<'a> {
    data: &'a [u8],
    pos: usize,
    quant: [Option<[u16; 64]>; 4],
    dc: [Option<RawTable>; 4],
    ac: [Option<RawTable>; 4],
    restart_interval: usize,
    frame: Option<Frame>,
    saw_jfif: bool,
    adobe_transform: Option<u8>,
    scans: usize,
}

/// Et markørsegment slik libjpeg leser det: den oppgitte lengden, men bare bytene som
/// faktisk finnes. libjpeg validerer underveis mens den leser, så en feil i innholdet
/// oppdages før en eventuell avkutting; mangler en byte vi trenger, er fila avkuttet.
struct Seg<'a> {
    /// Den tilgjengelige delen av innholdet.
    body: &'a [u8],
    /// Oppgitt innholdslengde (lengdefeltet − 2); kan være negativ i ødelagte filer.
    len: isize,
}

impl<'a> Seg<'a> {
    fn read(data: &'a [u8], pos: usize) -> Result<Self> {
        let b = data.get(pos..pos + 2).ok_or(JpegError::Truncated)?;
        let len = u16::from_be_bytes([b[0], b[1]]) as isize - 2;
        let end = (pos + 2 + len.max(0) as usize).min(data.len());
        Ok(Self {
            body: &data[pos + 2..end],
            len,
        })
    }

    fn byte(&self, i: usize) -> Result<u8> {
        self.body.get(i).copied().ok_or(JpegError::Truncated)
    }

    fn bytes(&self, r: std::ops::Range<usize>) -> Result<&'a [u8]> {
        self.body.get(r).ok_or(JpegError::Truncated)
    }

    /// Hvor langt markøren rekker (lengdefelt + innhold).
    fn size(&self) -> usize {
        2 + self.len.max(0) as usize
    }
}

const BAD_LENGTH: &str = "Bogus marker length";

impl<'a> Decoder<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 2,
            quant: [None; 4],
            dc: [None, None, None, None],
            ac: [None, None, None, None],
            restart_interval: 0,
            frame: None,
            saw_jfif: false,
            adobe_transform: None,
            scans: 0,
        }
    }

    fn run(mut self) -> Result<Option<RgbImage>> {
        let mut marker = next_marker(self.data, &mut self.pos);
        let mut eoi = false;
        // `has_multiple_scans`, avgjort ved første SOS.
        let mut multi_scan = false;
        while let Some(m) = marker {
            match self.handle(m, &mut multi_scan) {
                Ok(Step::Next) => marker = next_marker(self.data, &mut self.pos),
                Ok(Step::AfterScan(next)) => marker = next,
                Ok(Step::Eoi) => {
                    eoi = true;
                    break;
                }
                Ok(Step::Unsupported) => return Ok(None),
                // Etter det eneste scannet har libjpeg levert alle linjene; at fila tar
                // slutt i markørene etterpå godtar Pillow (`finish_decompress` suspenderer).
                Err(JpegError::Truncated) if self.scans > 0 && !multi_scan => break,
                Err(e) => return Err(e),
            }
        }
        // Slutt på fila før EOI: med ett scan er det greit (som over); ellers venter
        // libjpeg på mer data.
        if !eoi && (self.scans == 0 || multi_scan) {
            return Err(JpegError::Truncated);
        }
        let Some(frame) = self.frame.take().filter(|_| self.scans > 0) else {
            return Err("ingen bildedata (SOF/SOS mangler)".into());
        };
        self.finish(frame)
    }

    /// Behandle én markør (`read_markers`).
    fn handle(&mut self, m: u8, multi_scan: &mut bool) -> Result<Step> {
        match m {
            0xD9 => return Ok(Step::Eoi),
            0xD8 => return Err("Invalid JPEG file structure: two SOI markers".into()),
            0xD0..=0xD7 | 0x01 => {}
            0xC0..=0xC2 => {
                if self.frame.is_some() {
                    return Err("Invalid JPEG file structure: two SOF markers".into());
                }
                let seg = Seg::read(self.data, self.pos)?;
                self.frame = Some(self.parse_sof(&seg, m == 0xC2)?);
                self.pos += seg.size();
            }
            // Tapsfri og aritmetisk koding: overlates til reservedekoderen.
            0xC3 | 0xC9..=0xCB => return Ok(Step::Unsupported),
            0xC5..=0xC8 | 0xCD..=0xCF => {
                return Err(format!("Unsupported JPEG process: SOF type 0x{m:02x}").into());
            }
            0xC4 => {
                let seg = Seg::read(self.data, self.pos)?;
                self.parse_dht(&seg)?;
                self.pos += seg.size();
            }
            0xDB => {
                let seg = Seg::read(self.data, self.pos)?;
                self.parse_dqt(&seg)?;
                self.pos += seg.size();
            }
            0xDD => {
                let seg = Seg::read(self.data, self.pos)?;
                if seg.len != 2 {
                    return Err(BAD_LENGTH.into());
                }
                let v = seg.bytes(0..2)?;
                self.restart_interval = usize::from(u16::from_be_bytes([v[0], v[1]]));
                self.pos += seg.size();
            }
            0xDA => {
                if self.frame.is_none() {
                    return Err("Invalid JPEG file structure: SOS before SOF".into());
                }
                let seg = Seg::read(self.data, self.pos)?;
                let scan = self.parse_sos(&seg)?;
                self.pos += seg.size();
                let frame = self.frame.as_ref().expect("SOF finnes (sjekket over)");
                if self.scans == 0 {
                    *multi_scan = frame.progressive || scan.comps.len() < frame.comps.len();
                } else if !*multi_scan {
                    return Err("Didn't expect more than one scan".into());
                }
                let next = self.decode_scan(&scan)?;
                self.scans += 1;
                return Ok(Step::AfterScan(next));
            }
            0xCC | 0xDC | 0xE0..=0xEF | 0xFE => {
                // `skip_variable` / `examine_app0` / `examine_app14`.
                let seg = Seg::read(self.data, self.pos)?;
                let b = seg.body;
                if m == 0xE0 && seg.len >= 14 && b.len() >= 14 && b.starts_with(b"JFIF\0") {
                    self.saw_jfif = true;
                } else if m == 0xEE && seg.len >= 12 && b.len() >= 12 && b.starts_with(b"Adobe") {
                    self.adobe_transform = Some(b[11]);
                }
                self.pos += seg.size();
            }
            other => return Err(format!("Unsupported marker type 0x{other:02x}").into()),
        }
        Ok(Step::Next)
    }

    fn parse_sof(&self, seg: &Seg, progressive: bool) -> Result<Frame> {
        let precision = seg.byte(0)?;
        let hw = seg.bytes(1..5)?;
        let height = usize::from(u16::from_be_bytes([hw[0], hw[1]]));
        let width = usize::from(u16::from_be_bytes([hw[2], hw[3]]));
        let n = usize::from(seg.byte(5)?);
        if height == 0 || width == 0 || n == 0 {
            return Err("Empty JPEG image (DNL not supported)".into());
        }
        if seg.len != 6 + 3 * n as isize {
            return Err(BAD_LENGTH.into());
        }
        // Pillow avviser selv alt annet enn 8 bit og 1/3/4 komponenter.
        if precision != 8 || !matches!(n, 1 | 3 | 4) {
            return Err(format!("cannot handle {precision}-bit, {n}-component JPEG").into());
        }
        let mut comps = Vec::with_capacity(n);
        for c in seg.bytes(6..6 + 3 * n)?.as_chunks::<3>().0 {
            let (h, v) = (usize::from(c[1] >> 4), usize::from(c[1] & 15));
            if !(1..=4).contains(&h) || !(1..=4).contains(&v) {
                return Err("Bogus sampling factors".into());
            }
            comps.push(Component {
                id: c[0],
                h,
                v,
                tq: usize::from(c[2]),
                dw: 0,
                dh: 0,
                wib: 0,
                hib: 0,
                bw: 0,
                coefs: Vec::new(),
                quant: None,
                coef_bits: [-1; 64],
            });
        }
        let hmax = comps.iter().map(|c| c.h).max().unwrap_or(1);
        let vmax = comps.iter().map(|c| c.v).max().unwrap_or(1);
        // Bare heltallige forhold (ellers JERR_FRACT_SAMPLE_NOTIMPL).
        if comps.iter().any(|c| hmax % c.h != 0 || vmax % c.v != 0) {
            return Err("Fractional sampling not implemented yet".into());
        }
        let (mcux, mcuy) = (width.div_ceil(hmax * 8), height.div_ceil(vmax * 8));
        for c in &mut comps {
            c.dw = (width * c.h).div_ceil(hmax);
            c.dh = (height * c.v).div_ceil(vmax);
            c.wib = c.dw.div_ceil(8);
            c.hib = c.dh.div_ceil(8);
            c.bw = mcux * c.h;
            c.coefs = vec![0; c.bw * mcuy * c.v * 64];
        }
        Ok(Frame {
            progressive,
            width,
            height,
            hmax,
            vmax,
            mcux,
            mcuy,
            comps,
        })
    }

    /// `get_dht`.
    fn parse_dht(&mut self, seg: &Seg) -> Result<()> {
        let (mut length, mut off) = (seg.len, 0);
        while length > 16 {
            let index = seg.byte(off)?;
            let bits: [u8; 16] = seg.bytes(off + 1..off + 17)?.try_into().expect("16 byte");
            let count: usize = bits.iter().map(|&b| usize::from(b)).sum();
            length -= 17;
            if count > 256 || count as isize > length {
                return Err("Bogus Huffman table definition".into());
            }
            let table = RawTable::new(bits, seg.bytes(off + 17..off + 17 + count)?);
            length -= count as isize;
            off += 17 + count;
            let id = usize::from(index & 0x0F);
            match index & 0xF0 {
                0x10 if id < 4 => self.ac[id] = Some(table),
                0x00 if id < 4 => self.dc[id] = Some(table),
                _ => return Err(format!("Bogus DHT index {index}").into()),
            }
        }
        if length != 0 {
            return Err(BAD_LENGTH.into());
        }
        Ok(())
    }

    /// `get_dqt`.
    fn parse_dqt(&mut self, seg: &Seg) -> Result<()> {
        let (mut length, mut off) = (seg.len, 0);
        while length > 0 {
            let n = seg.byte(off)?;
            let (prec, id) = (n >> 4, usize::from(n & 0x0F));
            if id >= 4 {
                return Err(format!("Bogus DQT index {id}").into());
            }
            let size = if prec != 0 { 128 } else { 64 };
            let body = seg.bytes(off + 1..off + 1 + size)?;
            let mut q = [0u16; 64];
            for (i, &pos) in NATURAL[..64].iter().enumerate() {
                q[pos] = if prec != 0 {
                    u16::from_be_bytes([body[2 * i], body[2 * i + 1]])
                } else {
                    u16::from(body[i])
                };
            }
            self.quant[id] = Some(q);
            length -= 1 + size as isize;
            off += 1 + size;
        }
        if length != 0 {
            return Err(BAD_LENGTH.into());
        }
        Ok(())
    }

    /// `get_sos`.
    fn parse_sos(&mut self, seg: &Seg) -> Result<Scan> {
        let frame = self.frame.as_mut().ok_or("SOS før SOF")?;
        let n = usize::from(seg.byte(0)?);
        if seg.len != 4 + 2 * n as isize || !(1..=4).contains(&n) {
            return Err(BAD_LENGTH.into());
        }
        let mut comps = Vec::with_capacity(n);
        for c in seg.bytes(1..1 + 2 * n)?.as_chunks::<2>().0 {
            let ci = frame
                .comps
                .iter()
                .position(|k| k.id == c[0])
                .ok_or_else(|| format!("Invalid component ID {} in SOS", c[0]))?;
            if comps.iter().any(|&(k, _, _)| k == ci) {
                return Err(format!("Invalid component ID {} in SOS", c[0]).into());
            }
            comps.push((ci, usize::from(c[1] >> 4), usize::from(c[1] & 15)));
        }
        let t = seg.bytes(1 + 2 * n..4 + 2 * n)?;
        let scan = Scan {
            comps,
            ss: usize::from(t[0]),
            se: usize::from(t[1]),
            ah: u32::from(t[2] >> 4),
            al: u32::from(t[2] & 15),
        };
        if n > 1
            && scan
                .comps
                .iter()
                .map(|&(ci, _, _)| frame.comps[ci].h * frame.comps[ci].v)
                .sum::<usize>()
                > 10
        {
            return Err("Sampling factors too large for interleaved scan".into());
        }
        // Lås kvantiseringstabellene ved komponentens første scan.
        for &(ci, _, _) in &scan.comps {
            let c = &mut frame.comps[ci];
            if c.quant.is_none() {
                let q =
                    self.quant.get(c.tq).copied().flatten().ok_or_else(|| {
                        format!("Quantization table 0x{:02x} was not defined", c.tq)
                    })?;
                c.quant = Some(q);
            }
        }
        Ok(scan)
    }

    /// Dekod ett scan; returnerer markøren som fulgte etter dataene.
    fn decode_scan(&mut self, scan: &Scan) -> Result<Option<u8>> {
        let frame = self
            .frame
            .as_mut()
            .expect("SOF finnes (sjekket i parse_sos)");
        enum Kind {
            Sequential,
            DcFirst,
            DcRefine,
            AcFirst,
            AcRefine,
        }
        let kind = if !frame.progressive {
            Kind::Sequential
        } else {
            let dc_band = scan.ss == 0;
            let bad = if dc_band {
                scan.se != 0
            } else {
                scan.ss > scan.se || scan.se >= 64 || scan.comps.len() != 1
            } || (scan.ah != 0 && scan.al + 1 != scan.ah)
                || scan.al > 13;
            if bad {
                return Err(format!(
                    "Invalid progressive parameters Ss={} Se={} Ah={} Al={}",
                    scan.ss, scan.se, scan.ah, scan.al
                )
                .into());
            }
            for &(ci, _, _) in &scan.comps {
                let bits = &mut frame.comps[ci].coef_bits;
                for b in &mut bits[scan.ss..=scan.se] {
                    *b = scan.al as i32;
                }
            }
            match (dc_band, scan.ah) {
                (true, 0) => Kind::DcFirst,
                (true, _) => Kind::DcRefine,
                (false, 0) => Kind::AcFirst,
                (false, _) => Kind::AcRefine,
            }
        };

        let progressive = frame.progressive;
        let mut dc_tables = Vec::new();
        let mut ac_tables = Vec::new();
        for &(_, td, ta) in &scan.comps {
            let need_dc = matches!(kind, Kind::Sequential | Kind::DcFirst);
            let need_ac = matches!(kind, Kind::Sequential | Kind::AcFirst | Kind::AcRefine);
            dc_tables.push(if need_dc {
                Some(huffman(&self.dc, td, true, progressive)?)
            } else {
                None
            });
            ac_tables.push(if need_ac {
                Some(huffman(&self.ac, ta, false, progressive)?)
            } else {
                None
            });
        }

        // MCU-rekkefølge: ikke-flettet (én komponent) går over komponentens egne blokker.
        let mut units: Vec<(usize, usize, usize)> = Vec::new(); // (scan-indeks, dx, dy) per blokk i MCU
        let (mcus_x, mcus_y) = if scan.comps.len() == 1 {
            units.push((0, 0, 0));
            let c = &frame.comps[scan.comps[0].0];
            (c.wib, c.hib)
        } else {
            for (si, &(ci, _, _)) in scan.comps.iter().enumerate() {
                let c = &frame.comps[ci];
                for dy in 0..c.v {
                    for dx in 0..c.h {
                        units.push((si, dx, dy));
                    }
                }
            }
            (frame.mcux, frame.mcuy)
        };
        let interleaved = scan.comps.len() > 1;

        let mut bits = Bits::new(self.data, self.pos);
        let mut last_dc = vec![0i32; scan.comps.len()];
        let mut eobrun = 0u32;
        let mut restarts_to_go = self.restart_interval;
        let mut next_rst = 0u8;
        let p1: i32 = 1 << scan.al;
        let m1: i32 = -1 << scan.al;

        for my in 0..mcus_y {
            for mx in 0..mcus_x {
                if self.restart_interval > 0 {
                    if restarts_to_go == 0 {
                        // `process_restart`: kast bitbufferet, finn RSTn, nullstill prediksjon.
                        restart(&mut bits, next_rst);
                        last_dc.iter_mut().for_each(|d| *d = 0);
                        eobrun = 0;
                        restarts_to_go = self.restart_interval;
                        next_rst = (next_rst + 1) & 7;
                    }
                    restarts_to_go -= 1;
                }
                if bits.insufficient {
                    continue; // «just leave the MCU set to zeroes»
                }
                for &(si, dx, dy) in &units {
                    let ci = scan.comps[si].0;
                    let comp = &mut frame.comps[ci];
                    let (bx, by) = if interleaved {
                        (mx * comp.h + dx, my * comp.v + dy)
                    } else {
                        (mx, my)
                    };
                    let block = comp.block(bx, by);
                    match kind {
                        Kind::Sequential => {
                            let dct = dc_tables[si].as_ref().expect("DC-tabell");
                            let act = ac_tables[si].as_ref().expect("AC-tabell");
                            let s = u32::from(bits.decode(dct));
                            let diff = bits.receive_extend(s);
                            last_dc[si] = last_dc[si].wrapping_add(diff);
                            block[0] = last_dc[si] as i16;
                            let mut k = 1;
                            while k < 64 {
                                let rs = bits.decode(act);
                                let (r, s) = (usize::from(rs >> 4), u32::from(rs & 15));
                                if s != 0 {
                                    k += r;
                                    let v = bits.receive_extend(s);
                                    block[NATURAL[k]] = v as i16;
                                } else {
                                    if r != 15 {
                                        break;
                                    }
                                    k += 15;
                                }
                                k += 1;
                            }
                        }
                        Kind::DcFirst => {
                            let dct = dc_tables[si].as_ref().expect("DC-tabell");
                            let s = u32::from(bits.decode(dct));
                            let diff = bits.receive_extend(s);
                            last_dc[si] = last_dc[si].wrapping_add(diff);
                            block[0] = (last_dc[si] << scan.al) as i16;
                        }
                        Kind::DcRefine => {
                            if bits.bits(1) != 0 {
                                block[0] |= p1 as i16;
                            }
                        }
                        Kind::AcFirst => {
                            if eobrun > 0 {
                                eobrun -= 1;
                                continue;
                            }
                            let act = ac_tables[si].as_ref().expect("AC-tabell");
                            let mut k = scan.ss;
                            while k <= scan.se {
                                let rs = bits.decode(act);
                                let (r, s) = (u32::from(rs >> 4), u32::from(rs & 15));
                                if s != 0 {
                                    k += r as usize;
                                    let v = bits.receive_extend(s);
                                    block[NATURAL[k]] = ((v as u32) << scan.al) as i16;
                                } else if r == 15 {
                                    k += 15;
                                } else {
                                    eobrun = (1 << r) + bits.bits(r) - 1;
                                    break;
                                }
                                k += 1;
                            }
                        }
                        Kind::AcRefine => {
                            let act = ac_tables[si].as_ref().expect("AC-tabell");
                            let mut k = scan.ss;
                            let refine = |bits: &mut Bits, c: &mut i16| {
                                if bits.bits(1) != 0 && i32::from(*c) & p1 == 0 {
                                    let add = if *c >= 0 { p1 } else { m1 };
                                    *c = c.wrapping_add(add as i16);
                                }
                            };
                            if eobrun == 0 {
                                while k <= scan.se {
                                    let rs = bits.decode(act);
                                    let (mut r, s) = (i32::from(rs >> 4), u32::from(rs & 15));
                                    let mut newval = 0i32;
                                    if s != 0 {
                                        newval = if bits.bits(1) != 0 { p1 } else { m1 };
                                    } else if r != 15 {
                                        eobrun = 1 << r;
                                        if r != 0 {
                                            eobrun += bits.bits(r as u32);
                                        }
                                        break;
                                    }
                                    while k <= scan.se {
                                        let c = &mut block[NATURAL[k]];
                                        if *c != 0 {
                                            refine(&mut bits, c);
                                        } else {
                                            r -= 1;
                                            if r < 0 {
                                                break;
                                            }
                                        }
                                        k += 1;
                                    }
                                    if newval != 0 {
                                        block[NATURAL[k]] = newval as i16;
                                    }
                                    k += 1;
                                }
                            }
                            if eobrun > 0 {
                                while k <= scan.se {
                                    let c = &mut block[NATURAL[k]];
                                    if *c != 0 {
                                        refine(&mut bits, c);
                                    }
                                    k += 1;
                                }
                                eobrun -= 1;
                            }
                        }
                    }
                }
            }
        }
        if bits.eof {
            return Err(JpegError::Truncated);
        }
        let marker = bits.next_marker();
        self.pos = bits.pos;
        Ok(marker)
    }

    /// IDCT, oppsampling og fargekonvertering etter siste scan.
    fn finish(&self, frame: Frame) -> Result<Option<RgbImage>> {
        // Merk: libjpeg glatter progressive bilder der noen av de første AC-koeffisientene
        // ikke er fullstendig sendt (`smoothing_ok`/`decompress_smooth_data`). Det skjer bare
        // med ufullstendige/ødelagte progressive filer og etterlignes ikke; da blir pikslene
        // her de ikke-glattede.
        let (w, h) = (frame.width, frame.height);
        let planes: Vec<Plane> = frame
            .comps
            .iter()
            .map(|c| Plane::idct(c, frame.hmax / c.h, frame.vmax / c.v))
            .collect();

        // `default_decompress_parms`: tre komponenter er YCbCr med JFIF; ellers avgjør
        // Adobe-transform (0 ⇒ RGB) eller komponent-id-ene ('R','G','B' ⇒ RGB). Fire er
        // CMYK, eller YCCK med Adobe-transform ≠ 0.
        let ids: Vec<u8> = frame.comps.iter().map(|c| c.id).collect();
        let space = match planes.len() {
            1 => Space::Gray,
            3 if !self.saw_jfif && self.adobe_transform.map_or(ids == b"RGB", |t| t == 0) => {
                Space::Rgb
            }
            3 => Space::YCbCr,
            _ if self.adobe_transform.is_some_and(|t| t != 0) => Space::Ycck,
            _ => Space::Cmyk,
        };

        // Rad for rad (som libjpeg): oppsampling til radbuffere, så fargekonvertering.
        let mut out = vec![0u8; w * h * 3];
        render_fast(&planes, space, &ColorTables::new(), &mut out, w);
        Ok(RgbImage::from_raw(w as u32, h as u32, out))
    }
}

/// JPEG-ens fargerom (`jpeg_color_space`).
#[derive(Clone, Copy)]
enum Space {
    Gray,
    Rgb,
    YCbCr,
    Cmyk,
    Ycck,
}

/// Oppsampling og fargekonvertering av alle rader inn i `out` (RGB, `w` bred).
#[inline(always)]
fn render(planes: &[Plane], space: Space, tables: &ColorTables, out: &mut [u8], w: usize) {
    let mut bufs: Vec<Vec<u8>> = planes.iter().map(|_| vec![0u8; 2 * w + 16]).collect();
    let mut sums = vec![0u32; w + 8];
    for (y, out_row) in out.chunks_exact_mut(w * 3).enumerate() {
        let rows: Vec<&[u8]> = planes
            .iter()
            .zip(bufs.iter_mut())
            .map(|(p, buf)| p.row(y, buf, &mut sums, w))
            .collect();
        match (space, rows.as_slice()) {
            (Space::Gray, [g]) => {
                for (o, &v) in out_row.as_chunks_mut::<3>().0.iter_mut().zip(*g) {
                    o.fill(v);
                }
            }
            (Space::Rgb, [a, b, c]) => {
                for (((o, &r), &g), &bl) in out_row
                    .as_chunks_mut::<3>()
                    .0
                    .iter_mut()
                    .zip(*a)
                    .zip(*b)
                    .zip(*c)
                {
                    *o = [r, g, bl];
                }
            }
            (Space::YCbCr, [yy, cb, cr]) => tables.convert(yy, cb, cr, out_row),
            (Space::Cmyk | Space::Ycck, [a, b, c, k]) => {
                for (x, o) in out_row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                    let (p0, p1, p2) = match space {
                        // `ycck_cmyk_convert`: CMY = 255 − (YCbCr → RGB), K uendret.
                        Space::Ycck => {
                            let rgb = tables.pixel(a[x], b[x], c[x]);
                            (255 - rgb[0], 255 - rgb[1], 255 - rgb[2])
                        }
                        _ => (a[x], b[x], c[x]),
                    };
                    // Pillow leser CMYK som `CMYK;I` (inverterte kanaler) og konverterer med
                    // `cmyk2rgb`: R = nk − round(C·nk/255), nk = 255 − K.
                    let nk = i32::from(k[x]); // = 255 − (255 − k)
                    let muldiv = |v: u8| {
                        let t = (255 - i32::from(v)) * nk + 128;
                        ((t >> 8) + t) >> 8
                    };
                    o[0] = (nk - muldiv(p0)).clamp(0, 255) as u8;
                    o[1] = (nk - muldiv(p1)).clamp(0, 255) as u8;
                    o[2] = (nk - muldiv(p2)).clamp(0, 255) as u8;
                }
            }
            _ => unreachable!("komponentantallet passer fargerommet"),
        }
    }
}

/// Alle ekte blokker i en komponent → `data` (radvis plan med `stride`).
#[inline(always)]
fn idct_blocks(c: &Component, q: &[i32; 64], data: &mut [u8], stride: usize) {
    for (by, band) in data.chunks_exact_mut(stride * 8).enumerate() {
        for bx in 0..c.wib {
            let i = (by * c.bw + bx) * 64;
            let coefs: &[i16; 64] = c.coefs[i..i + 64].try_into().expect("64 koeffisienter");
            idct_islow(coefs, q, &mut band[bx * 8..], stride);
        }
    }
}

avx2_dispatch!(fn idct_blocks_fast(c: &Component, q: &[i32; 64], data: &mut [u8], stride: usize) => idct_blocks);
avx2_dispatch!(fn render_fast(planes: &[Plane], space: Space, tables: &ColorTables, out: &mut [u8], w: usize) => render);

/// `ycc_rgb_convert`-tabellene (SCALEBITS 16).
struct ColorTables {
    cr_r: [i32; 256],
    cb_b: [i32; 256],
    cr_g: [i32; 256],
    cb_g: [i32; 256],
}

impl ColorTables {
    fn new() -> Self {
        const ONE_HALF: i32 = 1 << 15;
        let fix = |x: f64| (x * 65536.0 + 0.5) as i32;
        let mut t = Self {
            cr_r: [0; 256],
            cb_b: [0; 256],
            cr_g: [0; 256],
            cb_g: [0; 256],
        };
        for i in 0..256 {
            let x = i as i32 - 128;
            t.cr_r[i] = (fix(1.40200) * x + ONE_HALF) >> 16;
            t.cb_b[i] = (fix(1.77200) * x + ONE_HALF) >> 16;
            t.cr_g[i] = -fix(0.71414) * x;
            t.cb_g[i] = -fix(0.34414) * x + ONE_HALF;
        }
        t
    }

    /// Én piksel YCbCr → RGB.
    #[inline(always)]
    fn pixel(&self, y: u8, cb: u8, cr: u8) -> [u8; 3] {
        let clamp = |v: i32| v.clamp(0, 255) as u8;
        let (y, cb, cr) = (i32::from(y), usize::from(cb), usize::from(cr));
        [
            clamp(y + self.cr_r[cr]),
            clamp(y + ((self.cb_g[cb] + self.cr_g[cr]) >> 16)),
            clamp(y + self.cb_b[cb]),
        ]
    }

    #[inline(always)]
    fn convert(&self, y: &[u8], cb: &[u8], cr: &[u8], out: &mut [u8]) {
        let clamp = |v: i32| v.clamp(0, 255) as u8;
        for (((o, &y), &cb), &cr) in out.as_chunks_mut::<3>().0.iter_mut().zip(y).zip(cb).zip(cr) {
            let (y, cb, cr) = (i32::from(y), usize::from(cb), usize::from(cr));
            o[0] = clamp(y + self.cr_r[cr]);
            o[1] = clamp(y + ((self.cb_g[cb] + self.cr_g[cr]) >> 16));
            o[2] = clamp(y + self.cb_b[cb]);
        }
    }
}

// ---------------------------------------------------------------------------
// IDCT og oppsampling
// ---------------------------------------------------------------------------

/// En komponent etter IDCT, i nedsamplet oppløsning, med oppsamplingsfaktorer.
struct Plane {
    data: Vec<u8>,
    stride: usize,
    dw: usize,
    dh: usize,
    fx: usize,
    fy: usize,
}

impl Plane {
    /// Alle komponentens ekte blokker gjennom `jpeg_idct_islow`.
    fn idct(c: &Component, fx: usize, fy: usize) -> Self {
        let stride = c.wib * 8;
        let mut data = vec![0u8; stride * c.hib * 8];
        // Mangler tabellen (komponenten kom aldri i et scan), er multiplikatorene 0 → grått.
        let q: [i32; 64] = c.quant.map(|q| q.map(i32::from)).unwrap_or([0; 64]);
        idct_blocks_fast(c, &q, &mut data, stride);
        Self {
            data,
            stride,
            dw: c.dw,
            dh: c.dh,
            fx,
            fy,
        }
    }

    /// Nedsamplet rad `r`, med libjpegs kontekstrader: over første rad gjentas første
    /// rad, under siste (`downsampled_height`) gjentas siste.
    #[inline]
    fn src(&self, r: isize) -> &[u8] {
        let r = r.clamp(0, self.dh as isize - 1) as usize;
        &self.data[r * self.stride..r * self.stride + self.dw]
    }

    /// Utdata-rad `y` i full oppløsning (`jdsample.c`), minst `w` piksler.
    #[inline(always)]
    fn row<'s>(&'s self, y: usize, buf: &'s mut [u8], sums: &mut [u32], w: usize) -> &'s [u8] {
        let (dw, fx, fy) = (self.dw, self.fx, self.fy);
        let r = (y / fy) as isize;
        match (fx, fy) {
            (1, 1) => &self.src(r)[..w],
            (2, 1) if dw > 2 => {
                h2v1_fancy(self.src(r), &mut buf[..2 * dw]);
                &buf[..w]
            }
            (1, 2) => {
                // `h1v2_fancy_upsample`: 3/4 nærmeste rad + 1/4 neste, skjevhet 1 / 2.
                let (far, bias) = if y.is_multiple_of(2) {
                    (self.src(r - 1), 1)
                } else {
                    (self.src(r + 1), 2)
                };
                for ((o, &n), &f) in buf[..dw].iter_mut().zip(self.src(r)).zip(far) {
                    *o = ((u32::from(n) * 3 + u32::from(f) + bias) >> 2) as u8;
                }
                &buf[..w]
            }
            (2, 2) if dw > 2 => {
                let far = if y.is_multiple_of(2) {
                    self.src(r - 1)
                } else {
                    self.src(r + 1)
                };
                for ((s, &n), &f) in sums[..dw].iter_mut().zip(self.src(r)).zip(far) {
                    *s = u32::from(n) * 3 + u32::from(f);
                }
                h2v2_fancy_row(&sums[..dw], &mut buf[..2 * dw]);
                &buf[..w]
            }
            _ => {
                // Boks/heltallig replikering (`int_upsample`, `h2v1_upsample`, `h2v2_upsample`).
                let src = self.src(r);
                for (x, o) in buf[..w].iter_mut().enumerate() {
                    *o = src[x / fx];
                }
                &buf[..w]
            }
        }
    }
}

/// `jpeg_idct_islow` (jidctint.c) med 32-bits mellomregning og metning til 0..=255, som
/// libjpeg-turbos SIMD-variant (identisk med C-koden for alle gyldige data). Regnes med
/// åtte kolonner (pass 1) og åtte rader (pass 2) som parallelle «baner», så kompilatoren
/// kan vektorisere. libjpegs snarveier for nullkolonner/-rader gir samme tall som full
/// regning og trengs derfor ikke, bortsett fra den rene DC-blokka.
#[inline(always)]
fn idct_islow(coefs: &[i16; 64], q: &[i32; 64], out: &mut [u8], stride: usize) {
    const CONST_BITS: u32 = 13;
    const PASS1_BITS: u32 = 2;
    #[inline(always)]
    fn clamp(v: i32) -> u8 {
        (v + 128).clamp(0, 255) as u8
    }

    // Bare DC (vanlig i jevne flater): hele blokka får samme verdi.
    if coefs[1..].iter().all(|&c| c == 0) {
        let dc = i32::from(coefs[0]).wrapping_mul(q[0]) << PASS1_BITS;
        let v = clamp(dc.wrapping_add(1 << (PASS1_BITS + 2)) >> (PASS1_BITS + 3));
        for row in out.chunks_mut(stride).take(8) {
            row[..8].fill(v);
        }
        return;
    }

    // Pass 1: rad r av inndata som vektor over kolonnene.
    let d: [Lanes; 8] = std::array::from_fn(|r| {
        std::array::from_fn(|c| i32::from(coefs[r * 8 + c]).wrapping_mul(q[r * 8 + c]))
    });
    let ws = kernel(d).map(|v| descale(v, CONST_BITS - PASS1_BITS));
    // Pass 2: kolonne c av mellomresultatet som vektor over radene.
    let t: [Lanes; 8] = std::array::from_fn(|c| std::array::from_fn(|r| ws[r][c]));
    let o = kernel(t).map(|v| descale(v, CONST_BITS + PASS1_BITS + 3));
    for (r, row) in out.chunks_mut(stride).take(8).enumerate() {
        for (c, px) in row[..8].iter_mut().enumerate() {
            *px = clamp(o[c][r]);
        }
    }
}

type Lanes = [i32; 8];

#[inline(always)]
fn lanes(f: impl Fn(usize) -> i32) -> Lanes {
    std::array::from_fn(f)
}

#[inline(always)]
fn add(a: Lanes, b: Lanes) -> Lanes {
    lanes(|i| a[i].wrapping_add(b[i]))
}

#[inline(always)]
fn sub(a: Lanes, b: Lanes) -> Lanes {
    lanes(|i| a[i].wrapping_sub(b[i]))
}

#[inline(always)]
fn mul(a: Lanes, k: i32) -> Lanes {
    lanes(|i| a[i].wrapping_mul(k))
}

/// `DESCALE`: høyreskift med avrunding.
#[inline(always)]
fn descale(a: Lanes, n: u32) -> Lanes {
    lanes(|i| a[i].wrapping_add(1 << (n - 1)) >> n)
}

/// Felles 1D-kjerne i `jpeg_idct_islow` (LL&M) over åtte baner: innverdiene v[0..8]
/// (frekvens) → utverdiene (posisjon), før nedskalering.
#[inline(always)]
fn kernel(v: [Lanes; 8]) -> [Lanes; 8] {
    const CONST_BITS: u32 = 13;
    const FIX_0_298631336: i32 = 2446;
    const FIX_0_390180644: i32 = 3196;
    const FIX_0_541196100: i32 = 4433;
    const FIX_0_765366865: i32 = 6270;
    const FIX_0_899976223: i32 = 7373;
    const FIX_1_175875602: i32 = 9633;
    const FIX_1_501321110: i32 = 12299;
    const FIX_1_847759065: i32 = 15137;
    const FIX_1_961570560: i32 = 16069;
    const FIX_2_053119869: i32 = 16819;
    const FIX_2_562915447: i32 = 20995;
    const FIX_3_072711026: i32 = 25172;
    let shl = |a: Lanes| lanes(|i| a[i].wrapping_shl(CONST_BITS));

    // Partallsdelen.
    let z1 = mul(add(v[2], v[6]), FIX_0_541196100);
    let tmp2 = sub(z1, mul(v[6], FIX_1_847759065));
    let tmp3 = add(z1, mul(v[2], FIX_0_765366865));
    let tmp0 = shl(add(v[0], v[4]));
    let tmp1 = shl(sub(v[0], v[4]));
    let (tmp10, tmp13) = (add(tmp0, tmp3), sub(tmp0, tmp3));
    let (tmp11, tmp12) = (add(tmp1, tmp2), sub(tmp1, tmp2));

    // Oddetallsdelen.
    let (t0, t1, t2, t3) = (v[7], v[5], v[3], v[1]);
    let z1 = add(t0, t3);
    let z2 = add(t1, t2);
    let z3 = add(t0, t2);
    let z4 = add(t1, t3);
    let z5 = mul(add(z3, z4), FIX_1_175875602);
    let t0 = mul(t0, FIX_0_298631336);
    let t1 = mul(t1, FIX_2_053119869);
    let t2 = mul(t2, FIX_3_072711026);
    let t3 = mul(t3, FIX_1_501321110);
    let z1 = mul(z1, -FIX_0_899976223);
    let z2 = mul(z2, -FIX_2_562915447);
    let z3 = add(mul(z3, -FIX_1_961570560), z5);
    let z4 = add(mul(z4, -FIX_0_390180644), z5);
    let t0 = add(add(t0, z1), z3);
    let t1 = add(add(t1, z2), z4);
    let t2 = add(add(t2, z2), z3);
    let t3 = add(add(t3, z1), z4);
    [
        add(tmp10, t3),
        add(tmp11, t2),
        add(tmp12, t1),
        add(tmp13, t0),
        sub(tmp13, t0),
        sub(tmp12, t1),
        sub(tmp11, t2),
        sub(tmp10, t3),
    ]
}

/// `h2v1_fancy_upsample`: 3/4 nærmeste + 1/4 neste, avrunding 1/2 vekselvis.
#[inline(always)]
fn h2v1_fancy(input: &[u8], out: &mut [u8]) {
    let n = input.len();
    let v = |i: usize| u32::from(input[i]);
    out[0] = input[0];
    out[1] = ((v(0) * 3 + v(1) + 2) >> 2) as u8;
    for (i, pair) in out[2..2 * n - 2]
        .as_chunks_mut::<2>()
        .0
        .iter_mut()
        .enumerate()
    {
        let c = v(i + 1) * 3;
        pair[0] = ((c + v(i) + 1) >> 2) as u8;
        pair[1] = ((c + v(i + 2) + 2) >> 2) as u8;
    }
    out[2 * n - 2] = ((v(n - 1) * 3 + v(n - 2) + 1) >> 2) as u8;
    out[2 * n - 1] = input[n - 1];
}

/// Horisontal del av `h2v2_fancy_upsample` på vertikale kolonnesummer (3·nær + fjern).
#[inline(always)]
fn h2v2_fancy_row(sums: &[u32], out: &mut [u8]) {
    let n = sums.len();
    out[0] = ((sums[0] * 4 + 8) >> 4) as u8;
    out[1] = ((sums[0] * 3 + sums[1] + 7) >> 4) as u8;
    for (i, pair) in out[2..2 * n - 2]
        .as_chunks_mut::<2>()
        .0
        .iter_mut()
        .enumerate()
    {
        let c = sums[i + 1] * 3;
        pair[0] = ((c + sums[i] + 8) >> 4) as u8;
        pair[1] = ((c + sums[i + 2] + 7) >> 4) as u8;
    }
    out[2 * n - 2] = ((sums[n - 1] * 3 + sums[n - 2] + 8) >> 4) as u8;
    out[2 * n - 1] = ((sums[n - 1] * 4 + 7) >> 4) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name);
        std::fs::read(p).unwrap()
    }

    fn sha256(data: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Fasit fra Pillow 12.2 (libjpeg-turbo): SHA-256 av `Image.open(f).convert("RGB")`.
    /// `astronaut_prog422_rst.jpg` er progressiv, 4:2:2, med restart-markører, optimaliserte
    /// Huffman-tabeller og odde mål (203×149).
    #[test]
    fn dekoding_er_bitlik_pillow() {
        for (name, size, want) in [
            (
                "astronaut.jpg",
                (512, 512),
                "bd372f2684514c03f650a80d14b2cbf47f8a58238544087f1c61845c96c8c01a",
            ),
            (
                "astronaut_prog422_rst.jpg",
                (203, 149),
                "955389d6d6eaadae0a2107ea9914cedb27a9408d0f9052bee6c7be5aed419c4a",
            ),
        ] {
            let img = decode(&fixture(name)).unwrap().expect(name);
            assert_eq!(img.dimensions(), size, "{name}");
            assert_eq!(sha256(img.as_raw()), want, "{name}");
        }
    }

    #[test]
    fn rundtur_med_egen_koder() {
        let img = RgbImage::from_fn(37, 21, |x, y| {
            image::Rgb([(x * 6) as u8, (y * 11) as u8, 128])
        });
        let back = decode(&crate::jpegenc::encode(&img, 95)).unwrap().unwrap();
        assert_eq!(back.dimensions(), (37, 21));
        let max = back
            .as_raw()
            .iter()
            .zip(img.as_raw())
            .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
            .max()
            .unwrap();
        assert!(max <= 12, "maks avvik {max}");
    }

    #[test]
    fn eksotiske_varianter_overlates_til_reserven() {
        let img = RgbImage::from_pixel(16, 16, image::Rgb([10, 20, 30]));
        let mut jpeg = crate::jpegenc::encode(&img, 85);
        let sof = jpeg.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        jpeg[sof + 1] = 0xC9; // aritmetisk koding
        assert_eq!(decode(&jpeg).unwrap(), None);
    }

    #[test]
    fn manglende_huffman_tabell_er_fatal() {
        let img = RgbImage::from_pixel(16, 16, image::Rgb([10, 20, 30]));
        let mut jpeg = crate::jpegenc::encode(&img, 85);
        let sos = jpeg.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
        jpeg[sos + 6] = 0x22; // Y bruker DC/AC-tabell 2, som ikke finnes
        let err = decode(&jpeg).unwrap_err();
        assert_eq!(
            err,
            JpegError::Broken("Huffman table 0x02 was not defined".into())
        );
    }

    /// Fasit fra Pillow 12.2 for avkuttede og ødelagte filer.
    #[test]
    fn avkutting_og_markorer_som_pillow() {
        let img = RgbImage::from_fn(64, 64, |x, y| image::Rgb([(x * 4) as u8, (y * 4) as u8, 9]));
        let jpeg = crate::jpegenc::encode(&img, 85);
        let no_eoi = &jpeg[..jpeg.len() - 2];
        // Mangler EOI: bildedataene er ikke avsluttet av en markør → «truncated».
        assert_eq!(decode(no_eoi).unwrap_err(), JpegError::Truncated);
        assert_eq!(
            decode(&jpeg[..jpeg.len() / 2]).unwrap_err(),
            JpegError::Truncated
        );
        // En markør og så EOF etter det eneste scannet er greit; rusk etter EOI også.
        assert!(
            decode(&[no_eoi, &[0xFF, 0xC4, 0x00, 0x02]].concat())
                .unwrap()
                .is_some()
        );
        assert!(
            decode(&[jpeg.as_slice(), b"rusk"].concat())
                .unwrap()
                .is_some()
        );
        // Ukjent markør etter dataene → «broken data stream».
        let bad = [no_eoi, &[0xFF, 0x78, 0xFF, 0xD9]].concat();
        assert_eq!(
            decode(&bad).unwrap_err(),
            JpegError::Broken("Unsupported marker type 0x78".into())
        );
        // Progressiv uten EOI: libjpeg leser hele fila før utdata → «truncated».
        let prog = fixture("astronaut_prog422_rst.jpg");
        assert_eq!(
            decode(&prog[..prog.len() - 2]).unwrap_err(),
            JpegError::Truncated
        );
    }

    #[test]
    fn markor_midt_i_dataene_gir_gratt_resten() {
        // libjpeg: «premature end of data segment» er en advarsel; resten blir grått.
        let img = RgbImage::from_fn(64, 64, |x, y| {
            image::Rgb([(x * 4) as u8, (y * 4) as u8, 200])
        });
        let jpeg = crate::jpegenc::encode(&img, 85);
        let sos = jpeg.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
        let cut = sos + 14 + 40;
        let broken = [&jpeg[..cut], &[0xFF, 0xD9]].concat();
        let out = decode(&broken).unwrap().unwrap();
        assert_eq!(out.dimensions(), (64, 64));
        assert_eq!(out.get_pixel(63, 63).0, [128, 128, 128]);
    }
}
