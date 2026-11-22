use serde::{Deserialize, Deserializer};

pub use sm_domain::{FilterId, GmailAttachmentId, HistoryId, PageToken};

pub(crate) fn string_u64<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    use serde::de::Error as _;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Number(u64),
        Text(String),
    }
    match Raw::deserialize(d)? {
        Raw::Number(n) => Ok(n),
        Raw::Text(s) => s.parse().map_err(D::Error::custom),
    }
}
