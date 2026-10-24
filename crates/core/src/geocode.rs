//! Reverse-geocoding via Geonorge/Kartverket adresse-punktsøk.
//!
//! Gratis, ingen nøkkel. Dekker Norge — utenlandske koordinater gir tomt
//! `place_name`. Cache på avrundede koordinater (3 desimaler) for å unngå tusenvis
//! av identiske kall. Feilende geocoding blokkerer aldri ingest: ved feil eller treff
//! utenfor Norge blir `place_name` bare `None`.

use std::collections::HashMap;
use std::time::Duration;

pub const PUNKTSOK_URL: &str = "https://ws.geonorge.no/adresser/v1/punktsok";

pub struct Geocoder {
    radius: u32,
    precision: usize,
    agent: ureq::Agent,
    cache: HashMap<(String, String), Option<String>>,
}

impl Default for Geocoder {
    fn default() -> Self {
        Self::new(500, 3, Duration::from_secs(5))
    }
}

impl Geocoder {
    pub fn new(radius: u32, precision: usize, timeout: Duration) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .build()
            .into();
        Self {
            radius,
            precision,
            agent,
            cache: HashMap::new(),
        }
    }

    pub fn lookup(&mut self, lat: Option<f64>, lon: Option<f64>) -> Option<String> {
        let (lat, lon) = (lat?, lon?);
        // Korrekt desimalavrunding som Pythons `round(x, 3)`.
        let key = (
            format!("{lat:.*}", self.precision),
            format!("{lon:.*}", self.precision),
        );
        if let Some(hit) = self.cache.get(&key) {
            return hit.clone();
        }
        let place = self.fetch(lat, lon);
        self.cache.insert(key, place.clone());
        place
    }

    fn fetch(&self, lat: f64, lon: f64) -> Option<String> {
        let body: serde_json::Value = self
            .agent
            .get(PUNKTSOK_URL)
            .query("lat", lat.to_string())
            .query("lon", lon.to_string())
            .query("radius", self.radius.to_string())
            .query("treffPerSide", "1")
            .query("side", "0")
            .call()
            .ok()?
            .body_mut()
            .read_json()
            .ok()?;
        let nearest = body.get("adresser")?.as_array()?.first()?;
        let text = |key: &str| {
            nearest
                .get(key)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string()
        };
        format_place(&text("poststed"), &text("kommunenavn"))
    }
}

/// `"SANDNES", "SANDNES"` → `"Sandnes"`; ulike → `"Poststed, Kommune"`.
pub fn format_place(poststed: &str, kommune: &str) -> Option<String> {
    let (p, k) = (py_title(poststed), py_title(kommune));
    if !p.is_empty() && !k.is_empty() && p.to_lowercase() != k.to_lowercase() {
        return Some(format!("{p}, {k}"));
    }
    [p, k].into_iter().find(|s| !s.is_empty())
}

/// Pythons `str.title()`: stor forbokstav etter hvert tegn som ikke er en bokstav.
pub fn py_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        let cased = c.is_lowercase() || c.is_uppercase();
        if cased && !prev_cased {
            out.extend(c.to_uppercase());
        } else if cased {
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
        prev_cased = cased;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_som_python() {
        assert_eq!(py_title("SANDNES"), "Sandnes");
        assert_eq!(py_title("BØ I TELEMARK"), "Bø I Telemark");
        assert_eq!(py_title("ÅL"), "Ål");
        assert_eq!(py_title("NORD-FRON"), "Nord-Fron");
        assert_eq!(py_title("o'neil 2nd"), "O'Neil 2Nd");
    }

    #[test]
    fn stedsnavn_formatering() {
        assert_eq!(
            format_place("SANDNES", "SANDNES").as_deref(),
            Some("Sandnes")
        );
        assert_eq!(
            format_place("FORNEBU", "BÆRUM").as_deref(),
            Some("Fornebu, Bærum")
        );
        assert_eq!(format_place("", "BÆRUM").as_deref(), Some("Bærum"));
        assert_eq!(format_place("", ""), None);
    }

    #[test]
    fn mangler_koordinat_gir_ingen_oppslag() {
        let mut g = Geocoder::default();
        assert_eq!(g.lookup(None, Some(10.0)), None);
        assert!(g.cache.is_empty());
    }
}
