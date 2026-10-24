//! Bygg api-craten på nytt når `web/dist` endres, så release-binæren alltid bærer den
//! siste frontend-bygget (rust-embed sporer ellers bare filer som fantes ved forrige bygg).
//!
//! Finnes ikke `web/dist`, kjøres skriptet hver gang til mappa dukker opp. Det koster
//! bare en rekompilering av denne craten.

fn main() {
    println!("cargo:rerun-if-changed=../../web/dist");
    println!("cargo:rerun-if-changed=build.rs");
}
