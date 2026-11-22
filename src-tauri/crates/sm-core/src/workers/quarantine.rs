use std::path::Path;

use sm_domain::Millis;

const ATTRIBUTE: &str = "com.apple.quarantine";

const FLAGS: &str = "0081";

const AGENT: &str = "Supermonkey";

fn value(now: Millis) -> String {
    let seconds = now.0.div_euclid(1000).max(0);
    let event = uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase();
    format!("{FLAGS};{seconds:08x};{AGENT};{event}")
}

pub(crate) fn quarantine(path: &Path, now: Millis) -> Result<(), std::io::Error> {
    xattr::set(path, ATTRIBUTE, value(now).as_bytes())
}
