//! `np.argsort(a)` for `float64` slik numpy 2.4 gjør det på x86-64 med AVX2.
//!
//! sklearn sin HDBSCAN sorterer MST-kantene med `np.argsort(distance)` (standard
//! `kind="quicksort"`). numpy sender det videre til x86-simd-sort (`avx2_argsort`),
//! som **ikke** er stabil: like avstander havner i en rekkefølge bestemt av
//! sorteringsnettverkene og partisjoneringen. Like avstander er vanlige i MST-en
//! (gjensidige k-te naboer gir samme kjerneavstand), og rekkefølgen påvirker
//! enkeltlenke-treet og dermed etikettene. For å få identiske klynger må vi derfor
//! etterligne algoritmen nøyaktig, ikke bare sortere riktig.
//!
//! Dette er en skalar etterligning av x86-simd-sort på commit `5adb334` (vendret i
//! numpy 2.4.6): bitoniske nøkkel/indeks-nettverk for ≤ 256 elementer (vektorer med
//! 4 lanes), ellers median-av-4-pivot og venstre/høyre-partisjonering med 4 × 4 lanes.
//! Hver compare-exchange bytter bare når nøklene er strengt i feil rekkefølge, som
//! `_mm256_min_pd`/`_mm256_max_pd` + likhetsmaske i originalen.
//!
//! Avgrensning: numpy på en maskin med AVX-512 bruker 8 lanes og gir en annen
//! rekkefølge for like verdier, og uten AVX2 brukes numpys egen introsort. Vi følger
//! alltid AVX2-varianten (vanligst, og den fasiten er laget med). NaN støttes ikke
//! (avstander er aldri NaN); fallback-grenen når quicksort ikke gjør fremskritt
//! (`std::sort` i libstdc++) er erstattet med en stabil sortering.

const LANES: usize = 4;
/// Største lengde som sorteres direkte med nettverk (`argsort_n<…, 256>`).
const NETWORK_MAX: usize = 256;
const UNROLL: usize = 4;

type Keys = [f64; LANES];
type Idx = [usize; LANES];

/// Permutasjoner fra `avx2_64bit_swizzle_ops` (lane `l` henter fra `perm[l]`).
const SWAP2: [usize; 4] = [1, 0, 3, 2];
const REV4: [usize; 4] = [3, 2, 1, 0];
const SWAP4: [usize; 4] = [2, 3, 0, 1];

/// Indeksene som sorterer `a` stigende, i samme rekkefølge som numpy (AVX2).
pub fn argsort(a: &[f64]) -> Vec<usize> {
    let n = a.len();
    let mut arg: Vec<usize> = (0..n).collect();
    if n <= 1 {
        return arg;
    }
    if a.iter().any(|v| v.is_nan()) {
        // numpy faller til std::sort med NaN sist; brukes ikke av HDBSCAN.
        arg.sort_by(|&x, &y| a[x].total_cmp(&a[y]));
        return arg;
    }
    // Tidlig retur for allerede sorterte tabeller (`std::is_sorted` med `<`; uten NaN
    // er «ingen b < a» det samme som a <= b).
    if a.windows(2).all(|w| w[0] <= w[1]) {
        return arg;
    }
    let max_iters = 2 * n.ilog2() as usize;
    argsort_rec(a, &mut arg, 0, n - 1, max_iters);
    arg
}

/// `argsort_<vtype, argtype>` — `right` er inklusiv.
fn argsort_rec(arr: &[f64], arg: &mut [usize], left: usize, right: usize, max_iters: usize) {
    if max_iters == 0 {
        // Originalen bruker std::sort (introsort) her; nås bare ved svært skjeve
        // partisjoner. Stabil sortering er et bevisst, dokumentert avvik.
        arg[left..=right].sort_by(|&x, &y| arr[x].total_cmp(&arr[y]));
        return;
    }
    if right + 1 - left <= NETWORK_MAX {
        argsort_n(arr, &mut arg[left..=right]);
        return;
    }
    let pivot = pivot_median_of_4(arr, arg, left, right);
    let (pivot_index, smallest, biggest) = partition_unrolled(arr, arg, left, right + 1, pivot);
    if pivot != smallest {
        argsort_rec(arr, arg, left, pivot_index - 1, max_iters - 1);
    }
    if pivot != biggest {
        argsort_rec(arr, arg, pivot_index, right, max_iters - 1);
    }
}

/// `get_pivot_64bit` for 4 lanes: tredje minste av fire jevnt fordelte prøver.
fn pivot_median_of_4(arr: &[f64], arg: &[usize], left: usize, right: usize) -> f64 {
    if right - left >= LANES {
        let size = (right - left) / 4;
        let mut v = [1, 2, 3, 4].map(|m| arr[arg[left + m * size]]);
        v.sort_by(f64::total_cmp);
        v[2]
    } else {
        arr[arg[right]]
    }
}

// --- Sorteringsnettverk (≤ 256 elementer) -----------------------------------

/// `cmp_merge<vtype1, vtype2>`: hver lane sammenlignes med `perm`-partneren og tar
/// maks der `mask` har bit satt, ellers min; indeksen følger med bare når nøkkelen
/// faktisk byttes (lik nøkkel ⇒ egen indeks beholdes).
fn cmp_merge(k: &mut Keys, i: &mut Idx, perm: [usize; 4], mask: u8) {
    let pk = perm.map(|p| k[p]);
    let pi = perm.map(|p| i[p]);
    let (mut nk, mut ni) = ([0.0; LANES], [0; LANES]);
    for l in 0..LANES {
        let (other, own) = (pk[l], k[l]);
        // _mm256_min_pd(other, own) / _mm256_max_pd(other, own)
        let min = if other < own { other } else { own };
        let max = if other > own { other } else { own };
        let v = if (mask >> l) & 1 == 1 { max } else { min };
        nk[l] = v;
        ni[l] = if v == own { i[l] } else { pi[l] };
    }
    *k = nk;
    *i = ni;
}

/// `sort_reg_4lanes` (nøkkel + indeks).
fn sort_reg(k: &mut Keys, i: &mut Idx) {
    cmp_merge(k, i, SWAP2, 0xA);
    cmp_merge(k, i, REV4, 0xC);
    cmp_merge(k, i, SWAP2, 0xA);
}

/// `bitonic_merge_reg_4lanes`.
fn merge_reg(k: &mut Keys, i: &mut Idx) {
    cmp_merge(k, i, SWAP4, 0xC);
    cmp_merge(k, i, SWAP2, 0xA);
}

/// `COEX` mellom to vektorer: lane for lane, bytt bare når `a > b`.
fn coex(keys: &mut [Keys], idx: &mut [Idx], a: usize, b: usize) {
    for l in 0..LANES {
        if keys[a][l] > keys[b][l] {
            let (ka, kb) = (keys[a][l], keys[b][l]);
            keys[a][l] = kb;
            keys[b][l] = ka;
            let (ia, ib) = (idx[a][l], idx[b][l]);
            idx[a][l] = ib;
            idx[b][l] = ia;
        }
    }
}

fn reverse(keys: &mut [Keys], idx: &mut [Idx], v: usize) {
    keys[v].reverse();
    idx[v].reverse();
}

/// `bitonic_merge_n_vec<numVecs>` på vektorene `base..base + n`.
fn merge_n_vec(keys: &mut [Keys], idx: &mut [Idx], base: usize, n: usize) {
    if n >= 2 {
        for i in 0..n / 2 {
            let j = base + n - i - 1;
            reverse(keys, idx, j);
            coex(keys, idx, base + i, j);
            reverse(keys, idx, j);
        }
    }
    // bitonic_clean_n_vec
    let mut num = n / 2;
    while num >= 2 {
        for j in (0..n).step_by(num) {
            for i in 0..num / 2 {
                coex(keys, idx, base + i + j, base + i + j + num / 2);
            }
        }
        num /= 2;
    }
    for v in base..base + n {
        merge_reg(&mut keys[v], &mut idx[v]);
    }
}

/// `argsort_n<vtype, argtype, 256>`: sorter `arg` (≤ 256 indekser) etter `arr`.
fn argsort_n(arr: &[f64], arg: &mut [usize]) {
    let n = arg.len();
    let mut num_vecs = NETWORK_MAX / LANES;
    while num_vecs > 1 && n * 2 <= num_vecs * LANES {
        num_vecs /= 2;
    }
    // Ubrukte lanes fylles med +inf og maks-indeks, som de maskerte lastene.
    let mut keys = vec![[f64::INFINITY; LANES]; num_vecs];
    let mut idx = vec![[usize::MAX; LANES]; num_vecs];
    for (p, &a) in arg.iter().enumerate() {
        keys[p / LANES][p % LANES] = arr[a];
        idx[p / LANES][p % LANES] = a;
    }
    for v in 0..num_vecs {
        sort_reg(&mut keys[v], &mut idx[v]);
    }
    let mut per = 2;
    while per <= num_vecs {
        for g in 0..num_vecs / per {
            merge_n_vec(&mut keys, &mut idx, g * per, per);
        }
        per *= 2;
    }
    for (p, a) in arg.iter_mut().enumerate() {
        *a = idx[p / LANES][p % LANES];
    }
}

// --- Partisjonering (> 256 elementer) ----------------------------------------

/// `partition_vec_avx2` + `avx2_double_compressstore64`: lanes `< pivot` legges først
/// i rekkefølge, lanes `>= pivot` sist i omvendt rekkefølge, og hele vektoren skrives
/// både ved `l_store` og ved `r_store`. Returnerer antall `>= pivot`.
fn partition_vec(
    arr: &[f64],
    arg: &mut [usize],
    l_store: usize,
    r_store: usize,
    vec: Idx,
    pivot: f64,
) -> usize {
    let mut temp = [0usize; LANES];
    let (mut lo, mut hi) = (0, LANES);
    for &a in &vec {
        if arr[a] >= pivot {
            hi -= 1;
            temp[hi] = a;
        } else {
            temp[lo] = a;
            lo += 1;
        }
    }
    arg[l_store..l_store + LANES].copy_from_slice(&temp);
    arg[r_store..r_store + LANES].copy_from_slice(&temp);
    LANES - lo
}

fn load(arg: &[usize], at: usize) -> Idx {
    [arg[at], arg[at + 1], arg[at + 2], arg[at + 3]]
}

/// Skalar forhåndsrunde som gjør lengden delelig med `chunk` (felles for begge varianter).
fn scalar_prefix(
    arr: &[f64],
    arg: &mut [usize],
    left: &mut usize,
    right: &mut usize,
    chunk: usize,
    pivot: f64,
) {
    for _ in 0..(*right - *left) % chunk {
        if arr[arg[*left]] >= pivot {
            // !(x < pivot) uten NaN
            *right -= 1;
            arg.swap(*left, *right);
        } else {
            *left += 1;
        }
    }
}

/// `argpartition<vtype, argtype>` — `right` eksklusiv. Returnerer pivotindeksen.
fn partition(
    arr: &[f64],
    arg: &mut [usize],
    mut left: usize,
    mut right: usize,
    pivot: f64,
) -> usize {
    scalar_prefix(arr, arg, &mut left, &mut right, LANES, pivot);
    if left == right {
        return left;
    }
    if right - left == LANES {
        let v = load(arg, left);
        let ge = partition_vec(arr, arg, left, left, v, pivot);
        return left + (LANES - ge);
    }
    let vec_left = load(arg, left);
    let vec_right = load(arg, right - LANES);
    let mut r_store = right - LANES;
    let mut l_store = left;
    left += LANES;
    right -= LANES;
    while right != left {
        let v = if (r_store + LANES) - right < left - l_store {
            right -= LANES;
            load(arg, right)
        } else {
            let v = load(arg, left);
            left += LANES;
            v
        };
        let ge = partition_vec(arr, arg, l_store, r_store, v, pivot);
        r_store -= ge;
        l_store += LANES - ge;
    }
    let ge = partition_vec(arr, arg, l_store, r_store, vec_left, pivot);
    l_store += LANES - ge;
    let ge = partition_vec(arr, arg, l_store, l_store, vec_right, pivot);
    l_store += LANES - ge;
    l_store
}

/// `argpartition_unrolled<vtype, argtype, 4>` — `right` eksklusiv.
/// Returnerer (pivotindeks, minste, største) for området.
fn partition_unrolled(
    arr: &[f64],
    arg: &mut [usize],
    mut left: usize,
    mut right: usize,
    pivot: f64,
) -> (usize, f64, f64) {
    // `smallest`/`biggest` er bare min/maks av verdiene i området (ingen NaN).
    let (smallest, biggest) = arg[left..right]
        .iter()
        .map(|&a| arr[a])
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v), hi.max(v))
        });
    if right - left <= 8 * UNROLL * LANES {
        return (partition(arr, arg, left, right, pivot), smallest, biggest);
    }
    scalar_prefix(arr, arg, &mut left, &mut right, UNROLL * LANES, pivot);
    if left == right {
        return (left, smallest, biggest);
    }
    let vec_left: [Idx; UNROLL] = std::array::from_fn(|ii| load(arg, left + LANES * ii));
    let vec_right: [Idx; UNROLL] =
        std::array::from_fn(|ii| load(arg, right - LANES * (UNROLL - ii)));
    let mut r_store = right - LANES;
    let mut l_store = left;
    left += UNROLL * LANES;
    right -= UNROLL * LANES;
    while right != left {
        let vecs: [Idx; UNROLL] = if (r_store + LANES) - right < left - l_store {
            right -= UNROLL * LANES;
            std::array::from_fn(|ii| load(arg, right + ii * LANES))
        } else {
            let v = std::array::from_fn(|ii| load(arg, left + ii * LANES));
            left += UNROLL * LANES;
            v
        };
        for v in vecs {
            let ge = partition_vec(arr, arg, l_store, r_store, v, pivot);
            l_store += LANES - ge;
            r_store -= ge;
        }
    }
    for v in vec_left.into_iter().chain(vec_right) {
        let ge = partition_vec(arr, arg, l_store, r_store, v, pivot);
        l_store += LANES - ge;
        r_store -= ge;
    }
    (l_store, smallest, biggest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorterer_riktig_for_mange_lengder() {
        // Enkel LCG for deterministiske data med mange like verdier.
        let mut s: u64 = 42;
        let mut next = || {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (s >> 33) as u32
        };
        for n in [
            0usize, 1, 2, 3, 5, 8, 17, 64, 255, 256, 257, 300, 513, 1000, 4097,
        ] {
            let levels = 1 + n as u32 / 3;
            let a: Vec<f64> = (0..n)
                .map(|_| (next() % levels.max(1)) as f64 * 0.37)
                .collect();
            let order = argsort(&a);
            let mut seen = order.clone();
            seen.sort_unstable();
            assert_eq!(seen, (0..n).collect::<Vec<_>>(), "permutasjon, n={n}");
            assert!(
                order.windows(2).all(|w| a[w[0]] <= a[w[1]]),
                "sortert, n={n}"
            );
        }
    }

    #[test]
    fn like_verdier_som_numpy() {
        // np.argsort([1, .5, 1, .5, 1, .5, 2, .5]) på AVX2 (numpy 2.4.6).
        let a = [1.0, 0.5, 1.0, 0.5, 1.0, 0.5, 2.0, 0.5];
        assert_eq!(argsort(&a), [1, 3, 7, 5, 2, 0, 4, 6]);
        // Allerede sortert ⇒ identitet (tidlig retur), også med like verdier.
        assert_eq!(argsort(&[0.0, 0.0, 1.0, 1.0]), [0, 1, 2, 3]);
    }
}
