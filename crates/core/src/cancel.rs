//! Avbrudd ved trygge punkter (brukes av desktop-modus når vinduet lukkes under en jobb).
//!
//! Jobbkjøreren i API-et kjører jobben inne i [`scoped`] med et delt flagg. Pipeline-
//! stegene (scan, derive, tag, faces detect og «Oppdater alt»-kjeden) sjekker
//! [`requested`]/[`check`] på jobbtråden mellom bilder eller batcher, **etter** at det
//! som er ferdig er committet, og gir da [`Error::Cancelled`]. Arbeidstrådene gjør
//! ferdig bildet de holder på med, så ingenting står halvskrevet, og
//! `processing_state` gjør at neste kjøring fortsetter der denne slapp.
//!
//! Flagget ligger i en tråd-lokal variabel, så CLI-en og tester (som aldri kaller
//! [`scoped`]) oppfører seg nøyaktig som før.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{Error, Result};

thread_local! {
    static FLAG: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Kjør `f` med `flag` som avbruddsflagg for denne tråden.
pub fn scoped<R>(flag: Arc<AtomicBool>, f: impl FnOnce() -> R) -> R {
    struct Reset(Option<Arc<AtomicBool>>);
    impl Drop for Reset {
        fn drop(&mut self) {
            let old = self.0.take();
            FLAG.with(|c| *c.borrow_mut() = old);
        }
    }
    let _reset = Reset(FLAG.with(|c| c.borrow_mut().replace(flag)));
    f()
}

/// Er avbrudd bedt om for jobben på denne tråden?
pub fn requested() -> bool {
    FLAG.with(|c| {
        c.borrow()
            .as_ref()
            .is_some_and(|f| f.load(Ordering::Relaxed))
    })
}

/// `Err(Error::Cancelled)` hvis avbrudd er bedt om.
pub fn check() -> Result<()> {
    if requested() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flagget_gjelder_bare_inne_i_scoped() {
        assert!(!requested());
        let flag = Arc::new(AtomicBool::new(false));
        scoped(flag.clone(), || {
            assert!(check().is_ok());
            flag.store(true, Ordering::Relaxed);
            assert!(matches!(check(), Err(Error::Cancelled)));
            // Andre tråder ser ikke flagget.
            std::thread::spawn(|| assert!(!requested())).join().unwrap();
        });
        assert!(!requested());
    }
}
