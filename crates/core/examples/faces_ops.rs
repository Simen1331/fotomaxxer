//! Kjør en liste ansikts-/personoperasjoner mot en database (Rust-siden av
//! `tools/golden/compare_faces_ops.py`). Én linje ut per operasjon:
//! `ok`, `ok <verdi>` eller `feil: <melding>`.
//!
//! ```text
//! cargo run --release -p fotomaxxer-core --example faces_ops -- <fotomaxxer.db> <ops.json>
//! ```
//!
//! Operasjoner (JSON-lister): `["cluster", full, threshold|null]`, `["reassign", face, cid|null]`,
//! `["lock", face, bool]`, `["create", [faces], name|null]`, `["merge", src, into]`,
//! `["delete", cid]`, `["ignore", cid]`, `["rename", cid, name|null]`, `["sql", "…"]`.

use anyhow::{Context, Result, bail};
use fotomaxxer_core::{db, faces};
use serde_json::Value;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [db_path, ops_path] = args.as_slice() else {
        bail!("bruk: faces_ops <fotomaxxer.db> <ops.json>");
    };
    let conn = db::connect(std::path::Path::new(db_path))?;
    db::init_db(&conn)?;
    let ops: Vec<Vec<Value>> = serde_json::from_slice(&std::fs::read(ops_path)?)?;
    for op in ops {
        let name = op[0].as_str().context("operasjonsnavn")?;
        let int = |i: usize| op[i].as_i64().context("heltall");
        let opt_int = |i: usize| op[i].as_i64();
        let opt_str = |i: usize| op[i].as_str();
        let result: fotomaxxer_core::Result<String> = match name {
            "cluster" => faces::cluster(&conn, op[1].as_bool() == Some(true), op[2].as_f64())
                .map(|s| s.summary()),
            "reassign" => faces::reassign_face(&conn, int(1)?, opt_int(2)).map(|()| String::new()),
            "lock" => faces::set_face_locked(&conn, int(1)?, op[2].as_bool() == Some(true))
                .map(|()| String::new()),
            "create" => {
                let ids: Vec<i64> = op[1]
                    .as_array()
                    .context("liste")?
                    .iter()
                    .filter_map(Value::as_i64)
                    .collect();
                faces::create_person_from_faces(&conn, &ids, opt_str(2)).map(|id| id.to_string())
            }
            "merge" => faces::merge_persons(&conn, int(1)?, int(2)?).map(|()| String::new()),
            "delete" => faces::delete_person(&conn, int(1)?).map(|()| String::new()),
            "ignore" => faces::ignore_person(&conn, int(1)?).map(|n| n.to_string()),
            "rename" => faces::set_person_name(&conn, int(1)?, opt_str(2)).map(|()| String::new()),
            "sql" => conn
                .execute_batch(op[1].as_str().context("sql")?)
                .map(|()| String::new())
                .map_err(Into::into),
            other => bail!("ukjent operasjon {other}"),
        };
        match result {
            Ok(v) if v.is_empty() => println!("ok"),
            Ok(v) => println!("ok {v}"),
            Err(e) => println!("feil: {e}"),
        }
    }
    Ok(())
}
