//! The recipient check against the case table the UI's copy of it is also
//! held to (`e2e/recipients.spec.ts`, RCP-11): the composer marks a chip
//! invalid exactly when the core would refuse to send to it.

use serde::Deserialize;
use sm_mime::is_valid_email;

#[derive(Deserialize)]
struct Case {
    address: String,
    valid: bool,
    why: String,
}

#[test]
fn every_address_in_the_shared_table_is_judged_as_listed() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/email-addresses.json")).unwrap();
    assert!(cases.iter().any(|c| c.valid) && cases.iter().any(|c| !c.valid));
    let wrong: Vec<String> = cases
        .iter()
        .filter(|c| is_valid_email(&c.address) != c.valid)
        .map(|c| format!("{} ({}): expected valid = {}", c.address, c.why, c.valid))
        .collect();
    assert!(wrong.is_empty(), "{wrong:#?}");
}
