use std::io::Write;

use clap::Args;
use serde::Serialize;

#[derive(Args, Debug, Clone, Copy, Default)]
pub struct OutputArgs {
    /// Output machine-readable JSON instead of human-formatted text
    #[arg(long)]
    pub json: bool,
    #[arg(long, requires = "json")]
    pub pretty: bool,
}

pub trait OutputStyle: Serialize {
    fn human(&self, out: &mut dyn Write) -> anyhow::Result<()>;
}

impl OutputArgs {
    pub fn print(&self, item: &impl OutputStyle) -> anyhow::Result<()> {
        let mut out = std::io::stdout();
        if self.json {
            if self.pretty {
                writeln!(out, "{}", serde_json::to_string_pretty(item)?)?;
            } else {
                writeln!(out, "{}", serde_json::to_string(item)?)?;
            }
            Ok(())
        } else {
            item.human(&mut out)
        }
    }

    pub fn print_optional<T: OutputStyle>(
        &self,
        item: Option<&T>,
        human_none: impl FnOnce(&mut dyn Write) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let mut out = std::io::stdout();
        if self.json {
            if self.pretty {
                writeln!(out, "{}", serde_json::to_string_pretty(&item)?)?;
            } else {
                writeln!(out, "{}", serde_json::to_string(&item)?)?;
            }
            Ok(())
        } else if let Some(item) = item {
            item.human(&mut out)
        } else {
            human_none(&mut out)
        }
    }
}
