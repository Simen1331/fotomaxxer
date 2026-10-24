//! Innebygde dialoger: GTK på Linux (på hovedtråden, samme GTK som tao bruker; rfd ville
//! startet sin egen GTK-tråd), rfd (`MessageBoxW`) på Windows. Uten skjerm skrives
//! meldingen til stderr.

const TITLE: &str = "FotoMaxxer";

/// Teksten og knappene i lukkedialogen.
pub const CLOSE_TEXT: &str = "FotoMaxxer behandler fortsatt bilder. Vil du avbryte jobben og lukke? \
Det som er ferdig, er lagret, og neste kjøring fortsetter der jobben stoppet.";
pub const CANCEL_AND_CLOSE: &str = "Avbryt jobben og lukk";
pub const KEEP_USING: &str = "Fortsett å bruke appen";

#[cfg(target_os = "linux")]
mod imp {
    use gtk::prelude::*;

    fn run(
        kind: gtk::MessageType,
        text: &str,
        buttons: &[(&str, gtk::ResponseType)],
    ) -> Option<gtk::ResponseType> {
        if gtk::init().is_err() {
            eprintln!("{}: {text}", super::TITLE);
            return None;
        }
        let dialog = gtk::MessageDialog::new(
            None::<&gtk::Window>,
            gtk::DialogFlags::MODAL,
            kind,
            gtk::ButtonsType::None,
            text,
        );
        dialog.set_title(super::TITLE);
        for (label, response) in buttons {
            dialog.add_button(label, *response);
        }
        dialog.set_keep_above(true);
        let response = dialog.run();
        dialog.close();
        // SAFETY: dialogen eies av oss og brukes ikke etterpå.
        unsafe { dialog.destroy() };
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        Some(response)
    }

    pub fn error(text: &str) {
        run(
            gtk::MessageType::Error,
            text,
            &[("OK", gtk::ResponseType::Ok)],
        );
    }

    pub fn info(text: &str) -> bool {
        run(
            gtk::MessageType::Info,
            text,
            &[("OK", gtk::ResponseType::Ok)],
        )
        .is_some()
    }

    pub fn confirm_close() -> bool {
        let answer = run(
            gtk::MessageType::Question,
            super::CLOSE_TEXT,
            &[
                (super::KEEP_USING, gtk::ResponseType::Reject),
                (super::CANCEL_AND_CLOSE, gtk::ResponseType::Accept),
            ],
        );
        answer == Some(gtk::ResponseType::Accept)
    }
}

#[cfg(windows)]
mod imp {
    use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

    fn dialog(level: MessageLevel, text: &str) -> MessageDialog {
        MessageDialog::new()
            .set_title(super::TITLE)
            .set_description(text)
            .set_level(level)
    }

    pub fn error(text: &str) {
        dialog(MessageLevel::Error, text)
            .set_buttons(MessageButtons::Ok)
            .show();
    }

    pub fn info(text: &str) -> bool {
        dialog(MessageLevel::Info, text)
            .set_buttons(MessageButtons::Ok)
            .show();
        true
    }

    /// `MessageBoxW` har bare standardknapper (egne tekster krever comctl32 v6 og et
    /// manifest), så valgene står i teksten: Ja = avbryt og lukk, Nei = fortsett.
    pub fn confirm_close() -> bool {
        let text = format!(
            "{}\n\nJa: {}.\nNei: {}.",
            super::CLOSE_TEXT,
            super::CANCEL_AND_CLOSE,
            super::KEEP_USING
        );
        dialog(MessageLevel::Warning, &text)
            .set_buttons(MessageButtons::YesNo)
            .show()
            == MessageDialogResult::Yes
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod imp {
    pub fn error(text: &str) {
        eprintln!("{}: {text}", super::TITLE);
    }
    pub fn info(text: &str) -> bool {
        eprintln!("{}: {text}", super::TITLE);
        false
    }
    pub fn confirm_close() -> bool {
        true
    }
}

/// Feilmelding (blokkerer til brukeren trykker OK).
pub fn error(text: &str) {
    imp::error(text);
}

/// Informasjon med OK (blokkerer). `false` hvis dialogen ikke kunne vises (ingen skjerm).
pub fn info(text: &str) -> bool {
    imp::info(text)
}

/// Lukking mens en jobb kjører: `true` = avbryt jobben og lukk, `false` = fortsett.
pub fn confirm_close() -> bool {
    imp::confirm_close()
}
