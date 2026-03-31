use std::path::Path;

use anyhow::Context;

pub fn load_text(
    input: Option<&str>,
    input_file: Option<&Path>,
    kind: &str,
) -> anyhow::Result<String> {
    match (input, input_file) {
        (Some(text), None) => Ok(text.to_owned()),
        (None, Some(path)) => std::fs::read_to_string(path)
            .with_context(|| format!("read {kind} file `{}`", path.display())),
        _ => anyhow::bail!("one of --{kind} or --{kind}-file must be provided"),
    }
}
