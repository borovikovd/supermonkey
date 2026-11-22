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
