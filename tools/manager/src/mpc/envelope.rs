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
        anyhow::ensure!(
            !text.contains(DESCRIPTION_MARKER),
            "the description must not contain `{DESCRIPTION_MARKER}`; that line is the payload"
        );
        let json = serde_json::to_string(self).context("render the payload envelope")?;
        Ok(format!("{}\n\n{DESCRIPTION_MARKER}{json}", text.trim_end()))
    }

    /// The envelope embedded in a proposal description, if any.
    pub fn from_description(description: &str) -> anyhow::Result<Option<Self>> {
        let mut lines = description
            .lines()
            .map(str::trim)
            .filter_map(|line| line.strip_prefix(DESCRIPTION_MARKER));
        let Some(line) = lines.next() else {
            return Ok(None);
        };
        anyhow::ensure!(
            lines.next().is_none(),
            "the description carries more than one `{DESCRIPTION_MARKER}` line"
        );
        let envelope: Self = serde_json::from_str(line)
            .context("the description's tmplrmgr-mpc line is not a payload envelope")?;
        envelope.check_version().map(Some)
    }

    pub fn read_file(path: &Path) -> anyhow::Result<Self> {
        let envelope: Self = crate::commands::load_json_file(path)?;
        envelope.check_version()
    }

    pub fn write_file(&self, path: &Path) -> anyhow::Result<()> {
        let rendered = serde_json::to_string_pretty(self).context("render the payload envelope")?;
        crate::commands::write_atomically(path, format!("{rendered}\n").as_bytes())
            .with_context(|| format!("write the payload file at {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use templar_gateway_types::Base64Bytes;

    fn envelope() -> Envelope {
        Envelope::new(SignablePayload::Transaction {
            bytes: Base64Bytes(vec![1, 2, 3]),
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

    /// One marker means one payload: an operator's text cannot smuggle a decoy
    /// in front of the appended one, and a reader never has to pick.
    #[test]
    fn a_second_marker_is_refused_on_both_sides() {
        let decoy = format!("Rotate the owner\n{DESCRIPTION_MARKER}{{}}");
        assert!(envelope().render_description(&decoy).is_err());

        let rendered = envelope().render_description("x").expect("renders");
        let doubled = format!("{rendered}\n{DESCRIPTION_MARKER}{{}}");
        let error = Envelope::from_description(&doubled).expect_err("two markers");
        assert!(error.to_string().contains("more than one"), "{error}");
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
        assert_eq!(
            std::fs::read_dir(&dir).expect("list").count(),
            1,
            "no temp file is left behind"
        );
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    /// The destination's own name is never the temp file, so `--out x.tmp`
    /// and a pre-existing sibling `x.tmp` are both safe.
    #[test]
    fn writing_never_truncates_a_sibling_or_the_destination_itself() {
        let dir =
            std::env::temp_dir().join(format!("tmplrmgr-mpc-envelope-tmp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let sibling = dir.join("payload.tmp");
        std::fs::write(&sibling, "keep me").expect("sibling");

        envelope()
            .write_file(&dir.join("payload.json"))
            .expect("writes");
        assert_eq!(std::fs::read_to_string(&sibling).expect("read"), "keep me");

        envelope()
            .write_file(&sibling)
            .expect("overwrites the destination by rename");
        assert_eq!(Envelope::read_file(&sibling).expect("reads"), envelope());
        std::fs::remove_dir_all(dir).expect("cleanup");
    }
}
