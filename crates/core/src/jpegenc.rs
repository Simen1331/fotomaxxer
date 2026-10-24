//! JPEG-koder som gir samme fil som Pillows `im.save(path, "JPEG", quality=q)`.
//!
//! Pillow koder med libjpeg-turbo og standardinnstillinger: baseline (SOF0), YCbCr med
//! 4:2:0 (boksnitt med vekslende avrunding, `h2v2_downsample`), heltalls-DCT
//! (`jpeg_fdct_islow`), Annex K-tabeller skalert med `jpeg_quality_scaling`, standard
//! Huffman-tabeller, JFIF 1.01 uten DPI, ingen ICC/EXIF. Dette er en direkte port av den
//! veien, inkludert kantutfylling og «dummy-blokker» utenfor bildet, så samme piksler
//! inn gir byte-identisk fil ut.

use image::RgbImage;

/// `jpeg_natural_order`: sikksakk-indeks → naturlig (radvis) indeks.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// `std_luminance_quant_tbl` / `std_chrominance_quant_tbl` (naturlig rekkefølge).
const LUMA_QUANT: [u32; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];
const CHROMA_QUANT: [u32; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

/// Standard Huffman-tabeller (Annex K.3): antall koder per lengde 1–16, og verdiene.
const DC_LUMA_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_CHROMA_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const DC_VALUES: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const AC_LUMA_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
const AC_LUMA_VALUES: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
    0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5,
    0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
    0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];
const AC_CHROMA_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
const AC_CHROMA_VALUES: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
    0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0,
    0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26,
    0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5,
    0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
    0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda,
    0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];

/// `jpeg_add_quant_table` med `jpeg_quality_scaling` og `force_baseline` (klem til 255).
fn quant_table(basic: &[u32; 64], quality: u8) -> [u32; 64] {
    let q = u32::from(quality.clamp(1, 100));
    let scale = if q < 50 { 5000 / q } else { 200 - q * 2 };
    basic.map(|v| ((v * scale + 50) / 100).clamp(1, 255))
}

/// Kodet Huffman-tabell: (kode, lengde) per symbol (`jpeg_make_c_derived_tbl`).
struct Huffman {
    code: [u16; 256],
    size: [u8; 256],
}

impl Huffman {
    fn new(bits: &[u8; 16], values: &[u8]) -> Self {
        let (mut code, mut size) = ([0u16; 256], [0u8; 256]);
        let mut next = 0u16;
        let mut k = 0;
        for (len, &count) in bits.iter().enumerate() {
            for _ in 0..count {
                code[values[k] as usize] = next;
                size[values[k] as usize] = len as u8 + 1;
                next += 1;
                k += 1;
            }
            next <<= 1;
        }
        Self { code, size }
    }
}

/// Bitskriver med 0xFF-stuffing (`emit_bits`/`flush_bits`).
struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    #[inline]
    fn put(&mut self, value: u32, size: u32) {
        if size == 0 {
            return;
        }
        self.acc = (self.acc << size) | (value & ((1 << size) - 1));
        self.n += size;
        while self.n >= 8 {
            self.n -= 8;
            let byte = (self.acc >> self.n) as u8;
            self.out.push(byte);
            if byte == 0xFF {
                self.out.push(0);
            }
        }
        self.acc &= (1 << self.n) - 1;
    }

    /// Fyll siste byte med 1-bits.
    fn flush(&mut self) {
        self.put(0x7F, 7);
        self.acc = 0;
        self.n = 0;
    }
}

/// Lengden i bits til en koeffisient (antall bits i `|v|`).
#[inline]
fn nbits(v: i32) -> u32 {
    32 - v.unsigned_abs().leading_zeros()
}

/// `encode_one_block`: DC som differanse mot forrige blokk, AC som (nullrekke, lengde).
fn encode_block(bits: &mut Bits, block: &[i32; 64], last_dc: &mut i32, dc: &Huffman, ac: &Huffman) {
    let diff = block[0] - *last_dc;
    *last_dc = block[0];
    let n = nbits(diff);
    bits.put(
        u32::from(dc.code[n as usize]),
        u32::from(dc.size[n as usize]),
    );
    // Negative verdier skrives som (v - 1) i de laveste n bitene.
    bits.put((if diff < 0 { diff - 1 } else { diff }) as u32, n);

    let mut run = 0u32;
    for &pos in &ZIGZAG[1..] {
        let v = block[pos];
        if v == 0 {
            run += 1;
            continue;
        }
        while run > 15 {
            bits.put(u32::from(ac.code[0xF0]), u32::from(ac.size[0xF0]));
            run -= 16;
        }
        let n = nbits(v);
        let sym = ((run << 4) + n) as usize;
        bits.put(u32::from(ac.code[sym]), u32::from(ac.size[sym]));
        bits.put((if v < 0 { v - 1 } else { v }) as u32, n);
        run = 0;
    }
    if run > 0 {
        bits.put(u32::from(ac.code[0]), u32::from(ac.size[0]));
    }
}

/// `jpeg_fdct_islow`: heltalls-DCT (LL&M), resultatet skalert opp med 8.
fn fdct_islow(data: &mut [i32; 64]) {
    const CONST_BITS: i32 = 13;
    const PASS1_BITS: i32 = 2;
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
    fn descale(x: i32, n: i32) -> i32 {
        (x + (1 << (n - 1))) >> n
    }

    // Pass 1 (rader, skalert opp med 2^PASS1_BITS) og pass 2 (kolonner) er like
    // bortsett fra skaleringen av radresultatene.
    for pass in 0..2 {
        let (stride, step) = if pass == 0 { (1, 8) } else { (8, 1) };
        for line in 0..8 {
            let base = line * step;
            let at = |i: usize| base + i * stride;
            let d = |i: usize| data[at(i)];
            let tmp0 = d(0) + d(7);
            let tmp7 = d(0) - d(7);
            let tmp1 = d(1) + d(6);
            let tmp6 = d(1) - d(6);
            let tmp2 = d(2) + d(5);
            let tmp5 = d(2) - d(5);
            let tmp3 = d(3) + d(4);
            let tmp4 = d(3) - d(4);

            let tmp10 = tmp0 + tmp3;
            let tmp13 = tmp0 - tmp3;
            let tmp11 = tmp1 + tmp2;
            let tmp12 = tmp1 - tmp2;

            let (even, odd) = if pass == 0 {
                (CONST_BITS - PASS1_BITS, CONST_BITS - PASS1_BITS)
            } else {
                (CONST_BITS + PASS1_BITS, CONST_BITS + PASS1_BITS)
            };
            let (out0, out4) = if pass == 0 {
                ((tmp10 + tmp11) << PASS1_BITS, (tmp10 - tmp11) << PASS1_BITS)
            } else {
                (
                    descale(tmp10 + tmp11, PASS1_BITS),
                    descale(tmp10 - tmp11, PASS1_BITS),
                )
            };
            let z1 = (tmp12 + tmp13) * FIX_0_541196100;
            let out2 = descale(z1 + tmp13 * FIX_0_765366865, even);
            let out6 = descale(z1 - tmp12 * FIX_1_847759065, even);

            let z1 = tmp4 + tmp7;
            let z2 = tmp5 + tmp6;
            let z3 = tmp4 + tmp6;
            let z4 = tmp5 + tmp7;
            let z5 = (z3 + z4) * FIX_1_175875602;
            let tmp4 = tmp4 * FIX_0_298631336;
            let tmp5 = tmp5 * FIX_2_053119869;
            let tmp6 = tmp6 * FIX_3_072711026;
            let tmp7 = tmp7 * FIX_1_501321110;
            let z1 = -z1 * FIX_0_899976223;
            let z2 = -z2 * FIX_2_562915447;
            let z3 = -z3 * FIX_1_961570560 + z5;
            let z4 = -z4 * FIX_0_390180644 + z5;

            // Pass 1 lagres i DCTELEM (16 bit) i libjpeg-turbo; verdiene får plass.
            let store = |v: i32| if pass == 0 { i32::from(v as i16) } else { v };
            data[at(0)] = store(out0);
            data[at(4)] = store(out4);
            data[at(2)] = store(out2);
            data[at(6)] = store(out6);
            data[at(7)] = store(descale(tmp4 + z1 + z3, odd));
            data[at(5)] = store(descale(tmp5 + z2 + z4, odd));
            data[at(3)] = store(descale(tmp6 + z2 + z3, odd));
            data[at(1)] = store(descale(tmp7 + z1 + z4, odd));
        }
    }
}

/// Én komponentplan, allerede utfylt til hele blokker.
struct Plane {
    data: Vec<u8>,
    width: usize,
}

impl Plane {
    /// DCT + kvantisering av 8×8-blokka med øvre venstre hjørne i blokk (bx, by).
    fn block(&self, bx: usize, by: usize, divisors: &[i32; 64]) -> [i32; 64] {
        let mut b = [0i32; 64];
        for y in 0..8 {
            let row = &self.data[(by * 8 + y) * self.width + bx * 8..][..8];
            for x in 0..8 {
                b[y * 8 + x] = i32::from(row[x]) - 128;
            }
        }
        fdct_islow(&mut b);
        for (v, &d) in b.iter_mut().zip(divisors) {
            // Avrunding som klassisk libjpeg (libjpeg-turbos resiproke variant er lik).
            *v = if *v < 0 {
                -((-*v + d / 2) / d)
            } else {
                (*v + d / 2) / d
            };
        }
        b
    }
}

/// Kod et RGB-bilde som Pillow/libjpeg-turbo gjør med `quality`.
pub fn encode(img: &RgbImage, quality: u8) -> Vec<u8> {
    encode_with_comment(img, quality, None)
}

/// Som [`encode`], med en `COM`-markør rett etter JFIF-hodet (Pillows `comment=`).
/// Kommentaren må være høyst 65533 bytes (én markør).
pub fn encode_with_comment(img: &RgbImage, quality: u8, comment: Option<&[u8]>) -> Vec<u8> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    assert!(
        w > 0 && h > 0 && w <= 65535 && h <= 65535,
        "ugyldig JPEG-størrelse {w}x{h}"
    );
    let (mcu_cols, mcu_rows) = (w.div_ceil(16), h.div_ceil(16));
    // Blokker med ekte data per komponent (`width_in_blocks`/`height_in_blocks`).
    let (y_bw, y_bh) = (w.div_ceil(8), h.div_ceil(8));

    // RGB → YCbCr (`rgb_ycc_convert`) i full oppløsning. Kolonnene fylles ut til høyre ved
    // å gjenta siste piksel (`expand_right_edge`): Y til hele blokker, Cb/Cr til hele MCU-er.
    let (yw, cw2) = (y_bw * 8, mcu_cols * 16);
    let mut y_full = vec![0u8; yw * h];
    let mut cb_full = vec![0u8; cw2 * h];
    let mut cr_full = vec![0u8; cw2 * h];
    const CBCR: i32 = (128 << 16) + 32767;
    for (row, px_row) in img.as_raw().chunks_exact(w * 3).enumerate() {
        let (yr, cbr, crr) = (
            &mut y_full[row * yw..(row + 1) * yw],
            &mut cb_full[row * cw2..(row + 1) * cw2],
            &mut cr_full[row * cw2..(row + 1) * cw2],
        );
        for (x, px) in px_row.as_chunks::<3>().0.iter().enumerate() {
            let (r, g, b) = (i32::from(px[0]), i32::from(px[1]), i32::from(px[2]));
            yr[x] = ((19595 * r + 38470 * g + 7471 * b + 32768) >> 16) as u8;
            cbr[x] = ((-11059 * r - 21709 * g + 32768 * b + CBCR) >> 16) as u8;
            crr[x] = ((32768 * r - 27439 * g - 5329 * b + CBCR) >> 16) as u8;
        }
        let (ly, lcb, lcr) = (yr[w - 1], cbr[w - 1], crr[w - 1]);
        yr[w..].fill(ly);
        cbr[w..].fill(lcb);
        crr[w..].fill(lcr);
    }

    // Y: rader under bildet gjentar siste rad, opp til hele blokker.
    let mut y_data = y_full;
    y_data.resize(yw * y_bh * 8, 0);
    for row in h..y_bh * 8 {
        y_data.copy_within((h - 1) * yw..h * yw, row * yw);
    }
    let y_plane = Plane {
        data: y_data,
        width: yw,
    };

    // Cb/Cr: 2×2-snitt med skjevhet 1,2,1,2… per kolonne (`h2v2_downsample`); odde høyde
    // dobler siste rad. Deretter gjentas siste nedsamplede rad opp til hel MCU-høyde.
    let cw = mcu_cols * 8;
    let downsample = |full: &[u8]| {
        let mut out = vec![0u8; cw * mcu_rows * 8];
        let rows = h.div_ceil(2);
        for r in 0..rows {
            let r0 = &full[2 * r * cw2..][..cw2];
            let r1 = &full[(2 * r + 1).min(h - 1) * cw2..][..cw2];
            for (c, o) in out[r * cw..(r + 1) * cw].iter_mut().enumerate() {
                let sum = u32::from(r0[2 * c])
                    + u32::from(r0[2 * c + 1])
                    + u32::from(r1[2 * c])
                    + u32::from(r1[2 * c + 1]);
                *o = ((sum + 1 + (c as u32 & 1)) >> 2) as u8;
            }
        }
        for r in rows..mcu_rows * 8 {
            out.copy_within((rows - 1) * cw..rows * cw, r * cw);
        }
        Plane {
            data: out,
            width: cw,
        }
    };
    let cb_plane = downsample(&cb_full);
    let cr_plane = downsample(&cr_full);

    let (lq, cq) = (
        quant_table(&LUMA_QUANT, quality),
        quant_table(&CHROMA_QUANT, quality),
    );
    let ldiv = lq.map(|q| (q * 8) as i32);
    let cdiv = cq.map(|q| (q * 8) as i32);
    let (dc_l, ac_l) = (
        Huffman::new(&DC_LUMA_BITS, &DC_VALUES),
        Huffman::new(&AC_LUMA_BITS, &AC_LUMA_VALUES),
    );
    let (dc_c, ac_c) = (
        Huffman::new(&DC_CHROMA_BITS, &DC_VALUES),
        Huffman::new(&AC_CHROMA_BITS, &AC_CHROMA_VALUES),
    );

    let mut out = Vec::with_capacity(w * h / 4 + 1024);
    write_headers(&mut out, w as u16, h as u16, &lq, &cq, comment);
    let mut bits = Bits { out, acc: 0, n: 0 };
    let mut last_dc = [0i32; 3];
    for my in 0..mcu_rows {
        for mx in 0..mcu_cols {
            // Fire Y-blokker (2×2). Blokker utenfor bildet er «dummy»: bare DC, kopiert fra
            // forrige blokk i MCU-en (høyre kant) eller siste blokk i raden over (bunn).
            let mut ys = [[0i32; 64]; 4];
            for yi in 0..2 {
                let by = my * 2 + yi;
                for xi in 0..2 {
                    let bx = mx * 2 + xi;
                    let n = yi * 2 + xi;
                    if by < y_bh && bx < y_bw {
                        ys[n] = y_plane.block(bx, by, &ldiv);
                    } else {
                        let prev = if by < y_bh { n - 1 } else { yi * 2 - 1 };
                        ys[n][0] = ys[prev][0];
                    }
                }
            }
            for b in &ys {
                encode_block(&mut bits, b, &mut last_dc[0], &dc_l, &ac_l);
            }
            encode_block(
                &mut bits,
                &cb_plane.block(mx, my, &cdiv),
                &mut last_dc[1],
                &dc_c,
                &ac_c,
            );
            encode_block(
                &mut bits,
                &cr_plane.block(mx, my, &cdiv),
                &mut last_dc[2],
                &dc_c,
                &ac_c,
            );
        }
    }
    bits.flush();
    let mut out = bits.out;
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

/// SOI, JFIF, DQT×2, SOF0, DHT×4, SOS — samme rekkefølge og innhold som libjpeg.
fn write_headers(
    out: &mut Vec<u8>,
    w: u16,
    h: u16,
    lq: &[u32; 64],
    cq: &[u32; 64],
    comment: Option<&[u8]>,
) {
    let dqt = |id: u8, q: &[u32; 64]| {
        let mut b = vec![id];
        b.extend(ZIGZAG.iter().map(|&i| q[i] as u8));
        b
    };
    let dht = |class_id: u8, bits: &[u8; 16], values: &[u8]| {
        let mut b = vec![class_id];
        b.extend_from_slice(bits);
        b.extend_from_slice(values);
        b
    };
    let mut sof = vec![8];
    sof.extend_from_slice(&h.to_be_bytes());
    sof.extend_from_slice(&w.to_be_bytes());
    // Komponent-id, sampling (Y 2×2, Cb/Cr 1×1), kvantiseringstabell.
    sof.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    let segments = [
        // JFIF 1.01, enhet 0, tetthet 1:1, ingen miniatyr.
        Some((0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0".to_vec())),
        // Pillow skriver kommentaren etter `jpeg_start_compress` (SOI + JFIF), før DQT.
        comment.map(|c| (0xFE, c.to_vec())),
        Some((0xDB, dqt(0, lq))),
        Some((0xDB, dqt(1, cq))),
        Some((0xC0, sof)),
        Some((0xC4, dht(0x00, &DC_LUMA_BITS, &DC_VALUES))),
        Some((0xC4, dht(0x10, &AC_LUMA_BITS, &AC_LUMA_VALUES))),
        Some((0xC4, dht(0x01, &DC_CHROMA_BITS, &DC_VALUES))),
        Some((0xC4, dht(0x11, &AC_CHROMA_BITS, &AC_CHROMA_VALUES))),
        // SOS: tre komponenter (Y: tabell 0/0, Cb/Cr: 1/1), Ss=0, Se=63, Ah/Al=0.
        Some((0xDA, vec![3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0])),
    ];
    out.extend_from_slice(&[0xFF, 0xD8]);
    for (marker, body) in segments.into_iter().flatten() {
        out.extend_from_slice(&[0xFF, marker]);
        out.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kvalitet_85_gir_libjpegs_tabeller() {
        let lq = quant_table(&LUMA_QUANT, 85);
        // Fra Pillow-fil (DQT i sikksakk): 05 03 04 04 04 03 05 …
        assert_eq!([lq[0], lq[1], lq[8], lq[16]], [5, 3, 4, 4]);
        let cq = quant_table(&CHROMA_QUANT, 85);
        assert_eq!([cq[0], cq[63]], [5, 30]);
        assert_eq!(
            quant_table(&LUMA_QUANT, 1)[63],
            255,
            "force_baseline klemmer til 255"
        );
    }

    /// Fasit fra libjpegs `jpeg_fdct_islow` (samme vektor som i jpeg-encoder sine tester).
    #[test]
    fn fdct_som_libjpeg() {
        let input: [i32; 64] = [
            -70, -71, -70, -68, -67, -67, -67, -67, -72, -73, -72, -70, -69, -69, -68, -69, -75,
            -76, -74, -73, -73, -72, -71, -70, -77, -78, -77, -75, -76, -75, -73, -71, -78, -77,
            -77, -76, -79, -77, -76, -75, -78, -78, -77, -77, -77, -77, -78, -77, -79, -79, -78,
            -78, -78, -78, -79, -78, -80, -79, -78, -78, -81, -80, -78, -76,
        ];
        let want: [i32; 64] = [
            -4786, -66, 2, -18, 12, 12, 5, -7, 223, -37, -8, 21, 8, 5, -4, 6, 60, 6, -10, 5, 0, -2,
            -1, 5, 21, 21, -15, 12, -2, -7, 1, 0, -2, -5, 16, -15, 0, 5, -4, -8, 0, -7, -4, 6, 7,
            -4, 5, 4, 3, 0, 1, -5, 0, -1, 4, 1, -5, 7, 0, -3, -6, 1, 1, -4,
        ];
        let mut data = input;
        fdct_islow(&mut data);
        assert_eq!(data, want);
    }

    #[test]
    fn huffman_kodene_er_kanoniske() {
        let dc = Huffman::new(&DC_LUMA_BITS, &DC_VALUES);
        // Annex K.3.1: kategori 0 = "00", 1 = "010", 2 = "011", … 11 = "111111110".
        assert_eq!((dc.code[0], dc.size[0]), (0b00, 2));
        assert_eq!((dc.code[1], dc.size[1]), (0b010, 3));
        assert_eq!((dc.code[11], dc.size[11]), (0b1_1111_1110, 9));
        let ac = Huffman::new(&AC_LUMA_BITS, &AC_LUMA_VALUES);
        assert_eq!((ac.code[0x00], ac.size[0x00]), (0b1010, 4)); // EOB
        assert_eq!((ac.code[0xF0], ac.size[0xF0]), (0b111_1111_1001, 11)); // ZRL
    }

    fn sha256(data: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Fasit fra Pillow 12.2: `thumbnail` + `save(…, "JPEG", quality=85)` av
    /// tests/fixtures/astronaut.png → SHA-256 av **filbytene**. Skalering og koding gir
    /// samme fil som Python-versjonen, byte for byte (også odde størrelser og kanter).
    #[test]
    fn jpeg_er_bytelik_pillow() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/astronaut.png");
        let src = image::open(path).unwrap().to_rgb8();
        let cases = [
            (
                (512, 512),
                300,
                24058,
                "481755ea75ca675050762ced63f825d7e560ac10e7068f64992be7310f7a6a0f",
            ),
            (
                (512, 512),
                100,
                5162,
                "36262e1e970fdcd2b12e351d51887d98cdbc47258f21d439db0f66b00aac06ee",
            ),
            (
                (511, 333),
                100,
                3433,
                "c5ae5b88bbc0536f74f12d008e2fbc9b9072c5049666037dac39be0618ee8572",
            ),
            (
                (333, 511),
                64,
                2045,
                "53d6d75162d9b32a51601a3629f0198db3c07a3ad70fde7d8df965a8de2e85c5",
            ),
        ];
        for ((w, h), t, len, want) in cases {
            let crop = image::imageops::crop_imm(&src, 0, 0, w, h).to_image();
            let thumb = crate::resample::thumbnail(&crop, t).unwrap();
            let jpeg = encode(&thumb, 85);
            assert_eq!(jpeg.len(), len, "{w}x{h} → {t}");
            assert_eq!(sha256(&jpeg), want, "{w}x{h} → {t}");
        }
    }

    #[test]
    fn kommentar_skrives_etter_jfif() {
        let img = RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3]));
        let plain = encode(&img, 85);
        let with = encode_with_comment(&img, 85, Some(b"hei"));
        // SOI (2) + JFIF (18), så COM med lengde 5, så resten uendret.
        assert_eq!(&with[20..27], &[0xFF, 0xFE, 0x00, 0x05, b'h', b'e', b'i']);
        assert_eq!([&with[..20], &with[27..]].concat(), plain);
    }

    #[test]
    fn koder_og_dekoder_rundt() {
        for (w, h) in [(1, 1), (17, 9), (300, 200), (33, 64), (8, 31)] {
            let img = RgbImage::from_fn(w, h, |x, y| {
                image::Rgb([
                    (x * 7 % 256) as u8,
                    (y * 5 % 256) as u8,
                    ((x + y) % 256) as u8,
                ])
            });
            let jpeg = encode(&img, 85);
            assert_eq!(&jpeg[..4], &[0xFF, 0xD8, 0xFF, 0xE0]);
            let back = image::load_from_memory(&jpeg).unwrap().to_rgb8();
            assert_eq!(back.dimensions(), (w, h));
            let mean = back
                .as_raw()
                .iter()
                .zip(img.as_raw())
                .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
                .sum::<i32>() as f64
                / img.as_raw().len() as f64;
            assert!(mean < 6.0, "{w}x{h}: snittavvik {mean}");
        }
    }
}
