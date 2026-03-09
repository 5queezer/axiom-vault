//! AxiomVault GTK4/libadwaita desktop client for Linux.
//!
//! This is the entry point for the Linux native client. It owns the tokio
//! runtime and connects the GTK main loop to the shared Rust application
//! facade (`AppService`).

mod app;
mod ui;

fn main() {
    // Initialize tracing before anything else.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "axiomvault=info".into()),
        )
        .init();

    tracing::info!("AxiomVault Linux client starting");

    let exit_code = app::run();
    std::process::exit(exit_code);
}
