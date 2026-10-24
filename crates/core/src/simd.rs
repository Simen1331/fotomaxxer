//! Kjøring av vektoriserbare bildeløkker med AVX2 når CPU-en har det.
//!
//! Løkkene i `jpegdec`/`resample` er skrevet så kompilatoren kan vektorisere dem, men
//! standardmålet for x86-64 er bare SSE2. [`avx2_dispatch!`] lager en funksjon som kaller
//! en AVX2-kompilert kopi av den samme koden når prosessoren støtter det (sjekket ved
//! kjøring), ellers den vanlige. Det er nøyaktig samme heltallsregning — bare bredere
//! vektorer — så resultatet er bit for bit det samme.

/// `avx2_dispatch!(fn navn(a: A, b: B) -> R => indre);` definerer `navn`, som kaller
/// `indre(a, b)` kompilert for AVX2 hvis mulig. `indre` bør være `#[inline(always)]`,
/// slik at løkkene havner i AVX2-kopien.
macro_rules! avx2_dispatch {
    ($(#[$meta:meta])* $vis:vis fn $name:ident($($arg:ident: $ty:ty),* $(,)?) $(-> $ret:ty)? => $inner:path) => {
        $(#[$meta])*
        $vis fn $name($($arg: $ty),*) $(-> $ret)? {
            #[cfg(target_arch = "x86_64")]
            {
                #[target_feature(enable = "avx2")]
                fn avx2($($arg: $ty),*) $(-> $ret)? {
                    $inner($($arg),*)
                }
                if std::arch::is_x86_feature_detected!("avx2") {
                    // SAFETY: CPU-en støtter AVX2 (sjekket rett over).
                    return unsafe { avx2($($arg),*) };
                }
            }
            $inner($($arg),*)
        }
    };
}

pub(crate) use avx2_dispatch;
