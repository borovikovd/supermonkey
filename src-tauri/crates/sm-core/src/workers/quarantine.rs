//! The macOS quarantine flag on files written from mail. A file a stranger
//! sent opens with its default app (SPEC Reading), so it must get the
//! Gatekeeper check a browser download gets: without the flag, an
//! attachment such as a `.terminal`, `.fileloc`, or `.jar` file would run
//! with no warning.

use std::path::Path;

use sm_domain::Millis;

/// The extended attribute Gatekeeper reads.
const ATTRIBUTE: &str = "com.apple.quarantine";

/// The flags Mail and browsers write: downloaded, and not yet approved by
/// the user, so the first open is checked.
const FLAGS: &str = "0081";

/// The agent the Gatekeeper dialog names.
const AGENT: &str = "Supermonkey";

/// The attribute's value, as Mail writes it: flags, the time as hex Unix
/// seconds, the agent, and an event id.
fn value(now: Millis) -> String {
    let seconds = now.0.div_euclid(1000).max(0);
    let event = uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase();
    format!("{FLAGS};{seconds:08x};{AGENT};{event}")
}

/// Marks `path` as downloaded from mail at `now`. Call it before the file
/// is anywhere it can be opened.
pub(crate) fn quarantine(path: &Path, now: Millis) -> Result<(), std::io::Error> {
    xattr::set(path, ATTRIBUTE, value(now).as_bytes())
}
