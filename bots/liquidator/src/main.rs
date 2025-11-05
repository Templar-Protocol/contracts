use templar_liquidator::{Args, LiquidatorService};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

// Include NEAR VM stubs for native builds
#[cfg(not(target_arch = "wasm32"))]
mod near_stubs;

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_target(false)
                .with_thread_ids(false)
                .with_line_number(false)
                .with_file(false),
        )
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,templar_liquidator=debug")),
        )
        .init();

    // Parse arguments and build configuration
    let args = Args::parse_args();
    args.log_startup();

    let config = args.build_config();

    // Create and run service
    let service = LiquidatorService::new(config);
    service.run().await;
}
