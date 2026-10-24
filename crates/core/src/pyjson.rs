//! JSON skrevet byte-likt med Pythons `json.dumps` (standard skilletegn `", "` og `": "`).
//!
//! Databasen lagrer JSON-tekst (`faces.bbox`, `tagging_profiles.categories`,
//! `ai_tags.raw_json`). Samme tekst som Python-versjonen skrev gjør at databaser
//! fra de to versjonene kan sammenlignes direkte, og at ingenting endres bare av å
//! lagre en rad på nytt. Ikke-ASCII skrives som det er (`ensure_ascii=False`).
//! Flyttall skrives som Pythons `repr(float)` (`1e-05`, `1e+16`), ikke som ryu.
//! `dumps_ascii` er standardvarianten (`ensure_ascii=True`) som `requests` bruker
//! for `json=`-kropper. `dumps_compact` er FastAPIs `JSONResponse`
//! (`separators=(",", ":")`, `ensure_ascii=False`), og `dumps_pydantic` er det samme med
//! pydantic-cores flyttallsformat (`0.0000535`, `1e-7`) — det FastAPI bruker for svar med
//! `response_model`. Begge brukes av HTTP-API-et.

use std::io;

use serde::Serialize;
use serde_json::ser::Formatter;

struct PyFormatter {
    ensure_ascii: bool,
    /// `separators=(",", ":")` i stedet for standarden `(", ", ": ")`.
    compact: bool,
    /// Flyttall som pydantic-core i stedet for Pythons `repr`.
    pydantic_floats: bool,
}

impl PyFormatter {
    fn item_separator(&self) -> &'static [u8] {
        if self.compact { b"," } else { b", " }
    }
}

impl Formatter for PyFormatter {
    fn begin_array_value<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            writer.write_all(self.item_separator())
        }
    }

    fn begin_object_key<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            writer.write_all(self.item_separator())
        }
    }

    fn begin_object_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        writer.write_all(if self.compact { b":" } else { b": " })
    }

    fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        let text = if self.pydantic_floats {
            float_pydantic(value)
        } else {
            float_repr(value)
        };
        writer.write_all(text.as_bytes())
    }

    fn write_string_fragment<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        if !self.ensure_ascii {
            return writer.write_all(fragment.as_bytes());
        }
        // Som Pythons `ensure_ascii=True`: alt utenfor ' '..='~' blir `\uXXXX`
        // (små hex-sifre, surrogatpar over U+FFFF). Kontrolltegn, `"` og `\`
        // escapes allerede av serde_json på samme måte som Python.
        for c in fragment.chars() {
            if (' '..='~').contains(&c) {
                writer.write_all(&[c as u8])?;
            } else {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    write!(writer, "\\u{unit:04x}")?;
                }
            }
        }
        Ok(())
    }
}

/// Pythons `repr(float)`: korteste sifre som gir samme tall tilbake, desimalform når
/// tierpotensen er i `-4..16`, ellers `d.ddde±XX` (minst to sifre i eksponenten).
pub fn float_repr(value: f64) -> String {
    format_float(value, -4, 2)
}

/// pydantic-cores JSON-format for `float` (pydantic 2.13): som `repr`, men desimalform
/// helt ned til `1e-5` (`0.0000535`), og eksponenten uten utfylling (`1e-7`, `1e+16`).
/// Ikke-endelige tall blir `null` (`ser_json_inf_nan='null'`).
pub fn float_pydantic(value: f64) -> String {
    if !value.is_finite() {
        return "null".into();
    }
    format_float(value, -5, 1)
}

/// Felles formatering: desimalform når `min_decpt < decpt <= 16`, ellers vitenskapelig
/// med minst `exp_digits` sifre i eksponenten.
fn format_float(value: f64, min_decpt: i32, exp_digits: usize) -> String {
    if value.is_nan() {
        return "nan".into();
    }
    if value.is_infinite() {
        return if value > 0.0 {
            "inf".into()
        } else {
            "-inf".into()
        };
    }
    let sign = if value.is_sign_negative() { "-" } else { "" };
    // `{:e}` gir de korteste rundtur-sifrene, f.eks. `1.2345e-5`.
    let sci = format!("{:e}", value.abs());
    let (mantissa, exp) = sci.split_once('e').expect("{:e} har alltid eksponent");
    let exp: i32 = exp.parse().expect("gyldig eksponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let decpt = exp + 1; // posisjonen til desimalpunktet relativt til sifrene
    let body = if !(min_decpt < decpt && decpt <= 16) {
        let (first, rest) = digits.split_at(1);
        let frac = if rest.is_empty() {
            String::new()
        } else {
            format!(".{rest}")
        };
        let esign = if exp < 0 { '-' } else { '+' };
        format!("{first}{frac}e{esign}{:0exp_digits$}", exp.abs())
    } else if decpt <= 0 {
        format!("0.{}{digits}", "0".repeat(decpt.unsigned_abs() as usize))
    } else {
        let decpt = decpt as usize;
        if decpt >= digits.len() {
            format!("{digits}{}.0", "0".repeat(decpt - digits.len()))
        } else {
            format!("{}.{}", &digits[..decpt], &digits[decpt..])
        }
    };
    format!("{sign}{body}")
}

/// `json.dumps(value, ensure_ascii=False)`.
pub fn dumps<T: Serialize + ?Sized>(value: &T) -> String {
    dumps_with(value, false, false, false)
}

/// `json.dumps(value)` (standard `ensure_ascii=True`): all ikke-ASCII som `\uXXXX`.
pub fn dumps_ascii<T: Serialize + ?Sized>(value: &T) -> String {
    dumps_with(value, true, false, false)
}

/// `json.dumps(value, ensure_ascii=False, separators=(",", ":"))` — kroppen FastAPIs
/// `JSONResponse` sender for `dict`-svar og feil (`HTTPException`, 422).
pub fn dumps_compact<T: Serialize + ?Sized>(value: &T) -> String {
    dumps_with(value, false, true, false)
}

/// pydantics `model_dump_json()` — kroppen FastAPI sender for svar med `response_model`
/// (kompakt, UTF-8, flyttall som [`float_pydantic`]).
pub fn dumps_pydantic<T: Serialize + ?Sized>(value: &T) -> String {
    dumps_with(value, false, true, true)
}

fn dumps_with<T: Serialize + ?Sized>(
    value: &T,
    ensure_ascii: bool,
    compact: bool,
    pydantic_floats: bool,
) -> String {
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(
        &mut out,
        PyFormatter {
            ensure_ascii,
            compact,
            pydantic_floats,
        },
    );
    value
        .serialize(&mut ser)
        .expect("serialisering til minnet feiler ikke");
    String::from_utf8(out).expect("serde_json skriver gyldig UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn liste_med_mellomrom_som_python() {
        assert_eq!(dumps(&[1, 2, 3, 4]), "[1, 2, 3, 4]");
        assert_eq!(dumps(&Vec::<String>::new()), "[]");
        assert_eq!(
            dumps(&["materiale", "teknikk"]),
            r#"["materiale", "teknikk"]"#
        );
    }

    #[test]
    fn objekt_og_ikke_ascii() {
        let v = json!({"navn": "Bjørn", "tags": ["vår", "høst"], "n": null});
        assert_eq!(
            dumps(&v),
            r#"{"navn": "Bjørn", "tags": ["vår", "høst"], "n": null}"#
        );
    }

    #[test]
    fn kompakt_som_fastapi() {
        let v = json!({"navn": "Bjørn", "tall": [1, 2.5, 1e-5], "n": null, "o": {}});
        assert_eq!(
            dumps_compact(&v),
            r#"{"navn":"Bjørn","tall":[1,2.5,1e-05],"n":null,"o":{}}"#
        );
    }

    #[test]
    fn flyttall_som_pydantic() {
        // Fasit: pydantic.TypeAdapter(float).dump_json(v) (pydantic 2.13).
        for (v, want) in [
            (5.35e-05, "0.0000535"),
            (1e-05, "0.00001"),
            (1.2e-05, "0.000012"),
            (9.99e-06, "9.99e-6"),
            (1e-06, "1e-6"),
            (6.666666666666667e-06, "6.666666666666667e-6"),
            (1e-07, "1e-7"),
            (0.0001, "0.0001"),
            (1e15, "1000000000000000.0"),
            (9999999999999998.0, "9999999999999998.0"),
            (1e16, "1e+16"),
            (1.2345678901234568e16, "1.2345678901234568e+16"),
            (1e100, "1e+100"),
            (1.5e-100, "1.5e-100"),
            (5e-324, "5e-324"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (59.0, "59.0"),
            (0.30000000000000004, "0.30000000000000004"),
            (f64::NAN, "null"),
        ] {
            assert_eq!(float_pydantic(v), want, "{v:e}");
        }
        assert_eq!(
            dumps_pydantic(&json!({"total": 0.000107, "avg": 5.35e-05, "n": 2})),
            r#"{"total":0.000107,"avg":0.0000535,"n":2}"#
        );
    }

    #[test]
    fn ensure_ascii_som_python() {
        let v = json!({"tekst": "æøå – \u{7f} 😀 \"\\\n\u{1}", "x": 0.2});
        assert_eq!(
            dumps_ascii(&v),
            r#"{"tekst": "\u00e6\u00f8\u00e5 \u2013 \u007f \ud83d\ude00 \"\\\n\u0001", "x": 0.2}"#
        );
    }

    #[test]
    fn flyttall_som_python_repr() {
        for (v, want) in [
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (0.2, "0.2"),
            (1e-4, "0.0001"),
            (1e-5, "1e-05"),
            (9.5e-5, "9.5e-05"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1.5e16, "1.5e+16"),
            (123.456, "123.456"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
        ] {
            assert_eq!(float_repr(v), want, "{v:e}");
        }
        assert_eq!(dumps(&json!({"c": 1.2e-5})), r#"{"c": 1.2e-05}"#);
    }
}
