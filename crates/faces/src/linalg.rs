//! Vektorregning for klyngingen, bit-likt med numpy/OpenBLAS i Python-versjonen.
//!
//! `faces.py` regner sentroider og likheter i `float32`: `embs.mean(axis=0)`,
//! `np.linalg.norm(v)` (= `sqrt(v.dot(v))`) og `a @ b`. Prikkproduktene går til
//! OpenBLAS `cblas_sdot`, som på x86-64 med AVX2 (Haswell/Zen-kjernen) summerer med
//! fire FMA-akkumulatorer à 8 lanes og en fast reduksjonsrekkefølge. En naiv sum gir
//! avvik i siste bit, og det kan i sjeldne tilfeller vippe en tilordning som ligger
//! akkurat på terskelen. Derfor etterlignes rekkefølgen her.

/// `cblas_sdot(x, y)` slik OpenBLAS 0.3.x regner den med `sdot_microk_haswell-2.c`
/// (samme verdi som numpy `x @ y` for to `float32`-vektorer).
pub fn sdot(x: &[f32], y: &[f32]) -> f32 {
    assert_eq!(x.len(), y.len(), "sdot: ulik lengde");
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("fma") {
        // SAFETY: CPU-en støtter FMA (sjekket over).
        return unsafe { sdot_fma(x, y) };
    }
    sdot_impl(x, y)
}

/// Samme kode kompilert med maskinvare-FMA (resultatet er identisk: `mul_add` er
/// alltid korrekt avrundet, men programvarevarianten er treg).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "fma")]
unsafe fn sdot_fma(x: &[f32], y: &[f32]) -> f32 {
    sdot_impl(x, y)
}

#[inline(always)]
fn sdot_impl(x: &[f32], y: &[f32]) -> f32 {
    let n = x.len();
    let n1 = n & !31;
    let mut mydot = 0.0f32;
    if n1 > 0 {
        // ymm4..ymm7: vfmadd231ps acc += x·y, 32 elementer per runde.
        let mut acc = [[0.0f32; 8]; 4];
        for base in (0..n1).step_by(32) {
            for (a, lanes) in acc.iter_mut().enumerate() {
                for (l, v) in lanes.iter_mut().enumerate() {
                    let k = base + a * 8 + l;
                    *v = x[k].mul_add(y[k], *v);
                }
            }
        }
        // vextractf128 + vaddps (høy + lav halvdel), så 4+5, 6+7, (4+5)+(6+7), 2× vhaddps.
        let half = |v: &[f32; 8]| [v[0] + v[4], v[1] + v[5], v[2] + v[6], v[3] + v[7]];
        let [h0, h1, h2, h3] = [half(&acc[0]), half(&acc[1]), half(&acc[2]), half(&acc[3])];
        let s: [f32; 4] = std::array::from_fn(|l| (h0[l] + h1[l]) + (h2[l] + h3[l]));
        mydot = (s[0] + s[1]) + (s[2] + s[3]);
    }
    // Resten (n % 32) summeres i double med f32-produkter, så legges kjernen til.
    let mut dot = 0.0f64;
    for k in n1..n {
        dot += (y[k] * x[k]) as f64;
    }
    dot += mydot as f64;
    dot as f32
}

/// `v / np.linalg.norm(v)` (uendret hvis normen er 0), som `faces._normalize`.
pub fn normalize(v: &[f32]) -> Vec<f32> {
    let norm = sdot(v, v).sqrt();
    if norm > 0.0 {
        v.iter().map(|x| x / norm).collect()
    } else {
        v.to_vec()
    }
}

/// Re-normalisert gjennomsnitt (`faces._centroid`): `embs.mean(axis=0)` summerer rad
/// for rad i `float32` og deler på antallet, så normaliseres resultatet.
pub fn centroid(members: &[&[f32]]) -> Vec<f32> {
    let Some(first) = members.first() else {
        return Vec::new();
    };
    let mut sum = first.to_vec();
    for m in &members[1..] {
        for (s, v) in sum.iter_mut().zip(m.iter()) {
            *s += v;
        }
    }
    let count = members.len() as f32;
    let mean: Vec<f32> = sum.iter().map(|s| s / count).collect();
    normalize(&mean)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdot_er_prikkprodukt() {
        let x: Vec<f32> = (0..70).map(|i| i as f32 * 0.5).collect();
        let y: Vec<f32> = (0..70).map(|i| 1.0 - i as f32 * 0.01).collect();
        let naive: f64 = x.iter().zip(&y).map(|(a, b)| *a as f64 * *b as f64).sum();
        assert!((sdot(&x, &y) as f64 - naive).abs() < 1e-3 * naive.abs());
        assert_eq!(sdot(&[3.0], &[4.0]), 12.0);
        assert_eq!(sdot(&[], &[]), 0.0);
    }

    #[test]
    fn sentroide_er_normalisert_snitt() {
        let a = [1.0f32, 0.0, 0.0];
        let b = [0.0f32, 1.0, 0.0];
        let c = centroid(&[&a, &b]);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert!((c[0] - h).abs() < 1e-6 && (c[1] - h).abs() < 1e-6 && c[2] == 0.0);
        assert_eq!(normalize(&[0.0, 0.0]), [0.0, 0.0]);
    }
}
