mod cli;
mod commands;
mod context;
mod dispatch;
mod proxy;
mod resolve;
mod spec;

#[cfg(test)]
mod tests;

use clap::Parser;

use cli::Cli;

pub async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.console_level());
    tracing::info!(network = %cli.network, "Connecting");
    let ctx = context::build_context(&cli)?;
    dispatch::dispatch(ctx, cli.command).await
}

fn init_tracing(console_default: tracing::level_filters::LevelFilter) {
    use tracing::level_filters::LevelFilter;
    use tracing_subscriber::{
        fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer,
    };

    let console_filter = EnvFilter::builder()
        .with_default_directive(console_default.into())
        .from_env_lossy();
    // Logs are diagnostics; keep stdout clean for machine-readable JSON results.
    let console_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(console_filter);
    let registry = tracing_subscriber::registry().with(console_layer);

    // Best-effort daily-rotating file log under the OS state dir; console-only
    // if it can't be set up.
    let file_layer = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|dir| dir.join(env!("CARGO_PKG_NAME")).join("logs"))
        .and_then(|log_dir| {
            tracing_appender::rolling::RollingFileAppender::builder()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix("log")
                .build(&log_dir)
                .ok()
        })
        .map(|file_appender| {
            fmt::layer()
                .with_ansi(false)
                .with_writer(file_appender)
                .with_filter(LevelFilter::DEBUG)
        });

    if let Some(file_layer) = file_layer {
        registry.with(file_layer).init();
    } else {
        registry.init();
    }
}
