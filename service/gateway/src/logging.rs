use std::fs;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

const LOG_FILE_PREFIX: &str = "templar-gateway";

fn standard_log_dir() -> Option<std::path::PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|dir| dir.join("templar").join("gateway"))
}

pub fn init() -> Option<WorkerGuard> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let stdout_layer = fmt::layer()
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true);

    if let Some(log_dir) = standard_log_dir() {
        let _ = fs::create_dir_all(&log_dir);

        let file_appender = tracing_appender::rolling::daily(log_dir, LOG_FILE_PREFIX);
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        let file_layer = fmt::layer()
            .with_ansi(false)
            .with_writer(non_blocking)
            .with_target(true)
            .with_thread_ids(true)
            .with_file(true)
            .with_line_number(true);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(stdout_layer)
            .with(file_layer)
            .init();

        Some(guard)
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(stdout_layer)
            .init();

        None
    }
}
