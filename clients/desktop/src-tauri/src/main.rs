//! AxiomVault Desktop Client
//!
//! Tauri-based desktop application for managing encrypted vaults
//! with FUSE filesystem mounting support.

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod commands;
mod local_index;
mod state;

use std::sync::Arc;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::state::AppState;

fn main() {
    // Initialize logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting AxiomVault Desktop");

    // Set up application data directory
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("axiomvault");

    if !data_dir.exists() {
        std::fs::create_dir_all(&data_dir).expect("Failed to create data directory");
    }

    info!("Data directory: {:?}", data_dir);

    // Create application state
    let app_state = Arc::new(AppState::new(data_dir));

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::create_vault,
            commands::unlock_vault,
            commands::lock_vault,
            commands::mount_vault,
            commands::unmount_vault,
            commands::list_files,
            commands::create_file,
            commands::read_file,
            commands::update_file,
            commands::delete_file,
            commands::create_directory,
            commands::delete_directory,
            commands::get_fuse_info,
            commands::list_vaults,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
