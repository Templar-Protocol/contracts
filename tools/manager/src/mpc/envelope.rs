//! Where the unsigned bytes travel: appended to the Sputnik proposal
//! description (public), or kept in a local file (blind). Same JSON either way,
//! so a file written in public mode is interchangeable with the description.

use std::path::Path;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use super::payload::SignablePayload;

/// Prefix of the description line carrying the envelope.
pub const DESCRIPTION_MARKER: &str = "tmplrmgr-mpc:";

const CURRENT_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u8,
    #[serde(flatten)]
    pub payload: SignablePayload,
}

impl Envelope {
    pub const fn new(payload: SignablePayload) -> Self {
        Self {
            v: CURRENT_VERSION,
            payload,
        }
    }

    fn check_version(self) -> anyhow::Result<Self> {
        anyhow::ensure!(
            self.v == CURRENT_VERSION,
            "payload envelope version {} is not supported (this build reads v{CURRENT_VERSION})",
            self.v
        );
        Ok(self)
    }

    /// `text` followed by the envelope on its own marked line.
    pub fn render_description(&self, text: &str) -> anyhow::Result<String> {
        let json = serde_json::to_string(self).context("render the payload envelope")?;
        Ok(format!("{}\n\n{DESCRIPTION_MARKER}{json}", text.trim_end()))
    }

    /// The envelope embedded in a proposal description, if any.
    pub fn from_description(description: &str) -> anyhow::Result<Option<Self>> {
        let Some(line) = description
            .lines()
            .map(str::trim)
            .find_map(|line| line.strip_prefix(DESCRIPTION_MARKER))
        else {
            return Ok(None);
        };
        let envelope: Self = serde_json::from_str(line)
            .context("the description's tmplrmgr-mpc line is not a payload envelope")?;
        envelope.check_version().map(Some)
    }

    pub fn read_file(path: &Path) -> anyhow::Result<Self> {
        let envelope: Self = crate::commands::load_json_file(path)?;
        envelope.check_version()
    }

    /// Written through a temp file and renamed, so a crash mid-write cannot
    /// leave a truncated payload where the operator will look for it.
    pub fn write_file(&self, path: &Path) -> anyhow::Result<()> {
        let rendered = serde_json::to_string_pretty(self).context("render the payload envelope")?;
        let temporary = path.with_extension("tmp");
        {
            use std::io::Write as _;
            let mut file = std::fs::File::create(&temporary)
                .with_context(|| format!("create {}", temporary.display()))?;
            file.write_all(format!("{rendered}\n").as_bytes())
                .with_context(|| format!("write {}", temporary.display()))?;
            file.sync_all()
                .with_context(|| format!("flush {}", temporary.display()))?;
        }
        std::fs::rename(&temporary, path)
            .with_context(|| format!("replace the payload file at {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use templar_gateway_types::Base64Bytes;

    fn envelope() -> Envelope {
        Envelope::new(SignablePayload::Transaction {
            bytes: Base64Bytes(vec![1, 2, 3]),
            block_height: 77,
        })
    }

    #[test]
    fn description_round_trips_and_keeps_the_operator_text() {
        let rendered = envelope()
            .render_description("Rotate the market owner\n")
            .expect("renders");
        assert!(rendered.starts_with("Rotate the market owner\n\ntmplrmgr-mpc:{"));
        assert_eq!(
            Envelope::from_description(&rendered).expect("parses"),
            Some(envelope())
        );
    }

    #[test]
    fn a_description_without_an_envelope_is_not_an_error() {
        assert_eq!(
            Envelope::from_description("just words").expect("parses"),
            None
        );
    }

    #[test]
    fn a_garbled_envelope_line_is_an_error() {
        assert!(Envelope::from_description("tmplrmgr-mpc:{nope").is_err());
    }

    #[test]
    fn an_unknown_version_is_refused() {
        let mut json = serde_json::to_value(envelope()).expect("serializes");
        json["v"] = serde_json::json!(2);
        let error = Envelope::from_description(&format!("{DESCRIPTION_MARKER}{json}"))
            .expect_err("v2 is unknown");
        assert!(error.to_string().contains("version 2"), "{error}");
    }

    #[test]
    fn file_and_description_carry_identical_json() {
        let dir =
            std::env::temp_dir().join(format!("tmplrmgr-mpc-envelope-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("payload.json");
        envelope().write_file(&path).expect("writes");

        let from_file = Envelope::read_file(&path).expect("reads");
        let rendered = envelope().render_description("x").expect("renders");
        assert_eq!(
            Some(from_file),
            Envelope::from_description(&rendered).expect("parses")
        );
        assert!(!path.with_extension("tmp").exists());
        std::fs::remove_dir_all(dir).expect("cleanup");
    }
}
