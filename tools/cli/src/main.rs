//! AxiomVault CLI - Command line interface for vault operations.
//!
//! This tool provides a command-line interface for creating, managing,
//! and operating on encrypted vaults.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

use axiomvault_common::{VaultId, VaultPath};
use axiomvault_crypto::KdfParams;
use axiomvault_storage::gdrive::{AuthConfig, AuthManager, GDriveConfig, Tokens};
use axiomvault_sync::{ConflictStrategy, SyncConfig, SyncEngine, SyncMode, SyncState};
use axiomvault_vault::{VaultManager, VaultOperations};

#[derive(Parser)]
#[command(name = "axiomvault")]
#[command(about = "AxiomVault - Encrypted vault management")]
#[command(version)]
struct Cli {
    /// Enable verbose logging.
    #[arg(short, long)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new vault.
    Create {
        /// Vault name/identifier.
        #[arg(short, long)]
        name: String,

        /// Path to store the vault.
        #[arg(short, long)]
        path: PathBuf,

        /// KDF strength: "interactive", "moderate", or "sensitive".
        #[arg(short, long, default_value = "moderate")]
        strength: String,
    },

    /// Open an existing vault and start interactive session.
    Open {
        /// Path to the vault.
        #[arg(short, long)]
        path: PathBuf,
    },

    /// List contents of a vault directory.
    List {
        /// Path to the vault.
        #[arg(short = 'p', long)]
        vault_path: PathBuf,

        /// Directory within vault (default: root).
        #[arg(short, long, default_value = "/")]
        dir: String,
    },

    /// Add a file to the vault.
    Add {
        /// Path to the vault.
        #[arg(short = 'p', long)]
        vault_path: PathBuf,

        /// Source file to add.
        #[arg(short, long)]
        source: PathBuf,

        /// Destination path in vault.
        #[arg(short, long)]
        dest: String,
    },

    /// Extract a file from the vault.
    Extract {
        /// Path to the vault.
        #[arg(short = 'p', long)]
        vault_path: PathBuf,

        /// Source path in vault.
        #[arg(short, long)]
        source: String,

        /// Destination file path.
        #[arg(short, long)]
        dest: PathBuf,
    },

    /// Create a directory in the vault.
    Mkdir {
        /// Path to the vault.
        #[arg(short = 'p', long)]
        vault_path: PathBuf,

        /// Directory path to create.
        #[arg(short, long)]
        dir: String,
    },

    /// Remove a file from the vault.
    Remove {
        /// Path to the vault.
        #[arg(short = 'p', long)]
        vault_path: PathBuf,

        /// Path to remove.
        #[arg(short = 'f', long)]
        file: String,
    },

    /// Show vault information.
    Info {
        /// Path to the vault.
        #[arg(short, long)]
        path: PathBuf,
    },

    /// Change vault password.
    ChangePassword {
        /// Path to the vault.
        #[arg(short, long)]
        path: PathBuf,
    },

    /// Authenticate with Google Drive and get tokens.
    GdriveAuth {
        /// Optional custom client ID.
        #[arg(long)]
        client_id: Option<String>,

        /// Optional custom client secret.
        #[arg(long)]
        client_secret: Option<String>,

        /// Path to save tokens (JSON file).
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Create a vault on Google Drive.
    GdriveCreate {
        /// Vault name/identifier.
        #[arg(short, long)]
        name: String,

        /// Google Drive folder ID where vault will be stored.
        #[arg(short, long)]
        folder_id: String,

        /// Path to tokens file.
        #[arg(short, long)]
        tokens: PathBuf,

        /// KDF strength: "interactive", "moderate", or "sensitive".
        #[arg(short, long, default_value = "moderate")]
        strength: String,
    },

    /// Open a vault on Google Drive.
    GdriveOpen {
        /// Google Drive folder ID where vault is stored.
        #[arg(short, long)]
        folder_id: String,

        /// Path to tokens file.
        #[arg(short, long)]
        tokens: PathBuf,
    },

    /// Sync vault with remote storage.
    Sync {
        /// Path to the vault.
        #[arg(short = 'p', long)]
        vault_path: PathBuf,

        /// Conflict resolution strategy: "keep-both", "prefer-local", "prefer-remote".
        #[arg(short, long, default_value = "keep-both")]
        strategy: String,
    },

    /// Show sync status for the vault.
    SyncStatus {
        /// Path to the vault.
        #[arg(short = 'p', long)]
        vault_path: PathBuf,
    },

    /// List sync conflicts.
    SyncConflicts {
        /// Path to the vault.
        #[arg(short = 'p', long)]
        vault_path: PathBuf,
    },

    /// Resolve a sync conflict for a specific file.
    SyncResolve {
        /// Path to the vault.
        #[arg(short = 'p', long)]
        vault_path: PathBuf,

        /// File path in vault to resolve.
        #[arg(short, long)]
        file: String,

        /// Resolution strategy: "keep-both", "prefer-local", "prefer-remote".
        #[arg(short, long)]
        strategy: String,
    },

    /// Configure sync mode for the vault.
    SyncConfigure {
        /// Path to the vault.
        #[arg(short = 'p', long)]
        vault_path: PathBuf,

        /// Sync mode: "manual", "on-demand", "periodic", "hybrid".
        #[arg(short, long)]
        mode: String,

        /// Interval in seconds for periodic sync (required for periodic/hybrid modes).
        #[arg(short, long)]
        interval: Option<u64>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Setup logging
    let level = if cli.verbose {
        Level::DEBUG
    } else {
        Level::INFO
    };

    let subscriber = FmtSubscriber::builder()
        .with_max_level(level)
        .with_target(false)
        .compact()
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    match cli.command {
        Commands::Create {
            name,
            path,
            strength,
        } => cmd_create(&name, &path, &strength).await,

        Commands::Open { path } => cmd_open(&path).await,

        Commands::List { vault_path, dir } => cmd_list(&vault_path, &dir).await,

        Commands::Add {
            vault_path,
            source,
            dest,
        } => cmd_add(&vault_path, &source, &dest).await,

        Commands::Extract {
            vault_path,
            source,
            dest,
        } => cmd_extract(&vault_path, &source, &dest).await,

        Commands::Mkdir { vault_path, dir } => cmd_mkdir(&vault_path, &dir).await,

        Commands::Remove { vault_path, file } => cmd_remove(&vault_path, &file).await,

        Commands::Info { path } => cmd_info(&path).await,

        Commands::ChangePassword { path } => cmd_change_password(&path).await,

        Commands::GdriveAuth {
            client_id,
            client_secret,
            output,
        } => cmd_gdrive_auth(client_id, client_secret, &output).await,

        Commands::GdriveCreate {
            name,
            folder_id,
            tokens,
            strength,
        } => cmd_gdrive_create(&name, &folder_id, &tokens, &strength).await,

        Commands::GdriveOpen { folder_id, tokens } => {
            cmd_gdrive_open(&folder_id, &tokens).await
        }

        Commands::Sync {
            vault_path,
            strategy,
        } => cmd_sync(&vault_path, &strategy).await,

        Commands::SyncStatus { vault_path } => cmd_sync_status(&vault_path).await,

        Commands::SyncConflicts { vault_path } => cmd_sync_conflicts(&vault_path).await,

        Commands::SyncResolve {
            vault_path,
            file,
            strategy,
        } => cmd_sync_resolve(&vault_path, &file, &strategy).await,

        Commands::SyncConfigure {
            vault_path,
            mode,
            interval,
        } => cmd_sync_configure(&vault_path, &mode, interval).await,
    }
}

/// Prompt for password securely.
fn prompt_password(prompt: &str) -> Result<Vec<u8>> {
    let password = rpassword::prompt_password(prompt).context("Failed to read password")?;
    Ok(password.into_bytes())
}

/// Create a new vault.
async fn cmd_create(name: &str, path: &PathBuf, strength: &str) -> Result<()> {
    info!("Creating new vault: {}", name);

    let kdf_params = match strength {
        "interactive" => KdfParams::interactive(),
        "moderate" => KdfParams::moderate(),
        "sensitive" => KdfParams::sensitive(),
        _ => {
            anyhow::bail!("Invalid strength. Use: interactive, moderate, or sensitive");
        }
    };

    let password = prompt_password("Enter password: ")?;
    let confirm = prompt_password("Confirm password: ")?;

    if password != confirm {
        anyhow::bail!("Passwords do not match");
    }

    if password.is_empty() {
        anyhow::bail!("Password cannot be empty");
    }

    let vault_id = VaultId::new(name).context("Invalid vault name")?;
    let vault_path = path.to_string_lossy().to_string();

    let manager = VaultManager::new();
    let provider_config = serde_json::json!({
        "root": vault_path
    });

    let session = manager
        .create_vault(vault_id, &password, "local", provider_config, kdf_params)
        .await
        .context("Failed to create vault")?;

    println!("Vault created successfully!");
    println!("  ID: {}", session.vault_id());
    println!("  Location: {}", path.display());
    println!("  Provider: {}", session.config().provider_type);

    Ok(())
}

/// Open vault for interactive session.
async fn cmd_open(path: &PathBuf) -> Result<()> {
    info!("Opening vault at: {}", path.display());

    let password = prompt_password("Enter password: ")?;
    let vault_path = path.to_string_lossy().to_string();

    let manager = VaultManager::new();
    let provider_config = serde_json::json!({
        "root": vault_path
    });

    let session = manager
        .open_vault("local", provider_config, &password)
        .await
        .context("Failed to open vault")?;

    println!("Vault opened successfully!");
    println!("  ID: {}", session.vault_id());
    println!("  Session: {}", session.handle().as_str());

    // Interactive session would go here
    // For now, just show that vault is accessible
    println!("\nVault is ready for operations.");

    Ok(())
}

/// List directory contents.
async fn cmd_list(vault_path: &PathBuf, dir: &str) -> Result<()> {
    let password = prompt_password("Enter password: ")?;
    let path_str = vault_path.to_string_lossy().to_string();

    let manager = VaultManager::new();
    let provider_config = serde_json::json!({
        "root": path_str
    });

    let session = manager
        .open_vault("local", provider_config, &password)
        .await
        .context("Failed to open vault")?;

    let ops = VaultOperations::new(&session).context("Failed to create operations handler")?;
    let vault_dir = VaultPath::parse(dir).context("Invalid directory path")?;

    let contents = ops
        .list_directory(&vault_dir)
        .await
        .context("Failed to list directory")?;

    if contents.is_empty() {
        println!("Directory is empty.");
    } else {
        println!("Contents of {}:", dir);
        for (name, is_dir, size) in contents {
            if is_dir {
                println!("  [DIR]  {}/", name);
            } else {
                let size_str = size.map(|s| format!("{} bytes", s)).unwrap_or_default();
                println!("  [FILE] {} ({})", name, size_str);
            }
        }
    }

    Ok(())
}

/// Add a file to the vault.
async fn cmd_add(vault_path: &PathBuf, source: &PathBuf, dest: &str) -> Result<()> {
    info!("Adding file {} to vault as {}", source.display(), dest);

    let password = prompt_password("Enter password: ")?;
    let path_str = vault_path.to_string_lossy().to_string();

    // Read source file
    let content = tokio::fs::read(source)
        .await
        .context("Failed to read source file")?;

    let manager = VaultManager::new();
    let provider_config = serde_json::json!({
        "root": path_str
    });

    let session = manager
        .open_vault("local", provider_config, &password)
        .await
        .context("Failed to open vault")?;

    let ops = VaultOperations::new(&session)?;
    let dest_path = VaultPath::parse(dest).context("Invalid destination path")?;

    ops.create_file(&dest_path, &content)
        .await
        .context("Failed to add file")?;

    println!("File added successfully: {} ({} bytes)", dest, content.len());

    Ok(())
}

/// Extract a file from the vault.
async fn cmd_extract(vault_path: &PathBuf, source: &str, dest: &PathBuf) -> Result<()> {
    info!("Extracting {} from vault to {}", source, dest.display());

    let password = prompt_password("Enter password: ")?;
    let path_str = vault_path.to_string_lossy().to_string();

    let manager = VaultManager::new();
    let provider_config = serde_json::json!({
        "root": path_str
    });

    let session = manager
        .open_vault("local", provider_config, &password)
        .await
        .context("Failed to open vault")?;

    let ops = VaultOperations::new(&session)?;
    let source_path = VaultPath::parse(source).context("Invalid source path")?;

    let content = ops
        .read_file(&source_path)
        .await
        .context("Failed to read file from vault")?;

    tokio::fs::write(dest, &content)
        .await
        .context("Failed to write output file")?;

    println!(
        "File extracted successfully: {} ({} bytes)",
        dest.display(),
        content.len()
    );

    Ok(())
}

/// Create a directory in the vault.
async fn cmd_mkdir(vault_path: &PathBuf, dir: &str) -> Result<()> {
    info!("Creating directory: {}", dir);

    let password = prompt_password("Enter password: ")?;
    let path_str = vault_path.to_string_lossy().to_string();

    let manager = VaultManager::new();
    let provider_config = serde_json::json!({
        "root": path_str
    });

    let session = manager
        .open_vault("local", provider_config, &password)
        .await
        .context("Failed to open vault")?;

    let ops = VaultOperations::new(&session)?;
    let dir_path = VaultPath::parse(dir).context("Invalid directory path")?;

    ops.create_directory(&dir_path)
        .await
        .context("Failed to create directory")?;

    println!("Directory created: {}", dir);

    Ok(())
}

/// Remove a file from the vault.
async fn cmd_remove(vault_path: &PathBuf, file: &str) -> Result<()> {
    info!("Removing: {}", file);

    let password = prompt_password("Enter password: ")?;
    let path_str = vault_path.to_string_lossy().to_string();

    let manager = VaultManager::new();
    let provider_config = serde_json::json!({
        "root": path_str
    });

    let session = manager
        .open_vault("local", provider_config, &password)
        .await
        .context("Failed to open vault")?;

    let ops = VaultOperations::new(&session)?;
    let file_path = VaultPath::parse(file).context("Invalid file path")?;

    ops.delete_file(&file_path)
        .await
        .context("Failed to remove file")?;

    println!("File removed: {}", file);

    Ok(())
}

/// Show vault information.
async fn cmd_info(path: &PathBuf) -> Result<()> {
    info!("Getting vault info: {}", path.display());

    let password = prompt_password("Enter password: ")?;
    let path_str = path.to_string_lossy().to_string();

    let manager = VaultManager::new();
    let provider_config = serde_json::json!({
        "root": path_str
    });

    let session = manager
        .open_vault("local", provider_config, &password)
        .await
        .context("Failed to open vault")?;

    let config = session.config();

    println!("Vault Information:");
    println!("  ID: {}", config.id);
    println!("  Version: {}.{}", config.version.major, config.version.minor);
    println!("  Provider: {}", config.provider_type);
    println!("  Created: {}", config.created_at);
    println!("  Modified: {}", config.modified_at);
    println!("  KDF Parameters:");
    println!("    Memory: {} KiB", config.kdf_params.memory_cost);
    println!("    Time: {} iterations", config.kdf_params.time_cost);
    println!("    Parallelism: {}", config.kdf_params.parallelism);

    Ok(())
}

/// Change vault password.
async fn cmd_change_password(path: &PathBuf) -> Result<()> {
    info!("Changing vault password");

    let old_password = prompt_password("Enter current password: ")?;
    let new_password = prompt_password("Enter new password: ")?;
    let confirm = prompt_password("Confirm new password: ")?;

    if new_password != confirm {
        anyhow::bail!("New passwords do not match");
    }

    if new_password.is_empty() {
        anyhow::bail!("Password cannot be empty");
    }

    let path_str = path.to_string_lossy().to_string();

    let manager = VaultManager::new();
    let provider_config = serde_json::json!({
        "root": path_str
    });

    let mut session = manager
        .open_vault("local", provider_config, &old_password)
        .await
        .context("Failed to open vault")?;

    session
        .change_password(&old_password, &new_password)
        .context("Failed to change password")?;

    // Save updated config
    manager.save_config(&session).await?;

    println!("Password changed successfully!");

    Ok(())
}

/// Authenticate with Google Drive and save tokens.
async fn cmd_gdrive_auth(
    client_id: Option<String>,
    client_secret: Option<String>,
    output: &PathBuf,
) -> Result<()> {
    info!("Starting Google Drive authentication");

    let auth_config = if let (Some(id), Some(secret)) = (client_id, client_secret) {
        AuthConfig {
            client_id: id,
            client_secret: secret,
            redirect_url: "http://localhost:8080/callback".to_string(),
        }
    } else {
        AuthConfig::default()
    };

    let auth_manager = AuthManager::new(auth_config).context("Failed to create auth manager")?;

    let (auth_url, _csrf_token) = auth_manager.authorization_url();

    println!("Please visit this URL to authorize AxiomVault:");
    println!();
    println!("  {}", auth_url);
    println!();
    println!("After authorization, you will be redirected to a URL like:");
    println!("  http://localhost:8080/callback?code=AUTHORIZATION_CODE&state=...");
    println!();
    println!("Copy the 'code' parameter value from the URL.");
    println!();

    let code = rpassword::prompt_password("Enter the authorization code: ")
        .context("Failed to read code")?;

    info!("Exchanging authorization code for tokens");

    let tokens = auth_manager
        .exchange_code(&code)
        .await
        .context("Failed to exchange authorization code")?;

    // Save tokens to file
    let tokens_json =
        serde_json::to_string_pretty(&tokens).context("Failed to serialize tokens")?;

    tokio::fs::write(output, tokens_json)
        .await
        .context("Failed to write tokens file")?;

    println!("Authentication successful!");
    println!("  Tokens saved to: {}", output.display());
    println!("  Expires at: {}", tokens.expires_at);

    Ok(())
}

/// Create a vault on Google Drive.
async fn cmd_gdrive_create(
    name: &str,
    folder_id: &str,
    tokens_path: &PathBuf,
    strength: &str,
) -> Result<()> {
    info!("Creating new vault on Google Drive: {}", name);

    let kdf_params = match strength {
        "interactive" => KdfParams::interactive(),
        "moderate" => KdfParams::moderate(),
        "sensitive" => KdfParams::sensitive(),
        _ => {
            anyhow::bail!("Invalid strength. Use: interactive, moderate, or sensitive");
        }
    };

    let password = prompt_password("Enter password: ")?;
    let confirm = prompt_password("Confirm password: ")?;

    if password != confirm {
        anyhow::bail!("Passwords do not match");
    }

    if password.is_empty() {
        anyhow::bail!("Password cannot be empty");
    }

    // Load tokens
    let tokens_json = tokio::fs::read_to_string(tokens_path)
        .await
        .context("Failed to read tokens file")?;
    let tokens: Tokens =
        serde_json::from_str(&tokens_json).context("Failed to parse tokens file")?;

    let vault_id = VaultId::new(name).context("Invalid vault name")?;

    let manager = VaultManager::new();

    let gdrive_config = GDriveConfig {
        folder_id: folder_id.to_string(),
        tokens,
        auth_config: None,
    };

    let provider_config =
        serde_json::to_value(gdrive_config).context("Failed to serialize GDrive config")?;

    let session = manager
        .create_vault(vault_id, &password, "gdrive", provider_config, kdf_params)
        .await
        .context("Failed to create vault on Google Drive")?;

    println!("Vault created successfully on Google Drive!");
    println!("  ID: {}", session.vault_id());
    println!("  Folder ID: {}", folder_id);
    println!("  Provider: {}", session.config().provider_type);

    Ok(())
}

/// Open a vault on Google Drive.
async fn cmd_gdrive_open(folder_id: &str, tokens_path: &PathBuf) -> Result<()> {
    info!("Opening vault on Google Drive");

    let password = prompt_password("Enter password: ")?;

    // Load tokens
    let tokens_json = tokio::fs::read_to_string(tokens_path)
        .await
        .context("Failed to read tokens file")?;
    let tokens: Tokens =
        serde_json::from_str(&tokens_json).context("Failed to parse tokens file")?;

    let gdrive_config = GDriveConfig {
        folder_id: folder_id.to_string(),
        tokens,
        auth_config: None,
    };

    let provider_config =
        serde_json::to_value(gdrive_config).context("Failed to serialize GDrive config")?;

    let manager = VaultManager::new();

    let session = manager
        .open_vault("gdrive", provider_config, &password)
        .await
        .context("Failed to open vault on Google Drive")?;

    println!("Vault opened successfully from Google Drive!");
    println!("  ID: {}", session.vault_id());
    println!("  Session: {}", session.handle().as_str());
    println!("\nVault is ready for operations.");

    Ok(())
}

/// Parse conflict strategy from string.
fn parse_conflict_strategy(strategy: &str) -> Result<ConflictStrategy> {
    match strategy {
        "keep-both" => Ok(ConflictStrategy::KeepBoth),
        "prefer-local" => Ok(ConflictStrategy::PreferLocal),
        "prefer-remote" => Ok(ConflictStrategy::PreferRemote),
        _ => anyhow::bail!(
            "Invalid strategy. Use: keep-both, prefer-local, or prefer-remote"
        ),
    }
}

/// Parse sync mode from string.
fn parse_sync_mode(mode: &str, interval: Option<u64>) -> Result<SyncMode> {
    match mode {
        "manual" => Ok(SyncMode::Manual),
        "on-demand" => Ok(SyncMode::OnDemand),
        "periodic" => {
            let secs = interval.ok_or_else(|| {
                anyhow::anyhow!("Interval required for periodic mode")
            })?;
            Ok(SyncMode::Periodic {
                interval: std::time::Duration::from_secs(secs),
            })
        }
        "hybrid" => {
            let secs = interval.ok_or_else(|| {
                anyhow::anyhow!("Interval required for hybrid mode")
            })?;
            Ok(SyncMode::Hybrid {
                interval: std::time::Duration::from_secs(secs),
            })
        }
        _ => anyhow::bail!("Invalid mode. Use: manual, on-demand, periodic, or hybrid"),
    }
}

/// Sync vault with remote storage.
async fn cmd_sync(vault_path: &PathBuf, strategy: &str) -> Result<()> {
    info!("Starting vault sync");

    let conflict_strategy = parse_conflict_strategy(strategy)?;
    let password = prompt_password("Enter password: ")?;
    let path_str = vault_path.to_string_lossy().to_string();

    let manager = VaultManager::new();
    let provider_config = serde_json::json!({
        "root": path_str
    });

    let session = manager
        .open_vault("local", provider_config, &password)
        .await
        .context("Failed to open vault")?;

    let sync_config = SyncConfig {
        conflict_strategy,
        auto_resolve_conflicts: true,
        ..Default::default()
    };

    let staging_dir = vault_path.join(".axiom_sync");
    let sync_engine: SyncEngine<dyn axiomvault_storage::StorageProvider> =
        SyncEngine::from_arc(session.provider(), &staging_dir, sync_config)
            .await
            .context("Failed to create sync engine")?;

    println!("Starting sync...");
    let result = sync_engine
        .sync_full()
        .await
        .context("Sync failed")?;

    println!("Sync completed!");
    println!("  Files synced: {}", result.files_synced);
    println!("  Files failed: {}", result.files_failed);
    println!("  Conflicts found: {}", result.conflicts_found);
    println!("  Duration: {:?}", result.duration);

    Ok(())
}

/// Show sync status for the vault.
async fn cmd_sync_status(vault_path: &PathBuf) -> Result<()> {
    info!("Getting sync status");

    let staging_dir = vault_path.join(".axiom_sync");
    let state_file = staging_dir.join("sync_state.json");

    if !state_file.exists() {
        println!("No sync state found. Vault has not been synced yet.");
        return Ok(());
    }

    let state_json = tokio::fs::read_to_string(&state_file)
        .await
        .context("Failed to read sync state")?;

    let state: SyncState =
        serde_json::from_str(&state_json).context("Failed to parse sync state")?;

    println!("Sync Status:");
    if let Some(last_sync) = state.last_full_sync {
        println!("  Last full sync: {}", last_sync);
    } else {
        println!("  Last full sync: Never");
    }

    let counts = state.count_by_status();
    println!("  Files tracked: {}", state.entries().count());

    for (status, count) in counts {
        let status_str = match status {
            axiomvault_sync::SyncStatus::Synced => "Synced",
            axiomvault_sync::SyncStatus::LocalModified => "Local modified",
            axiomvault_sync::SyncStatus::RemoteModified => "Remote modified",
            axiomvault_sync::SyncStatus::Conflicted => "Conflicted",
            axiomvault_sync::SyncStatus::Syncing => "Syncing",
            axiomvault_sync::SyncStatus::Failed => "Failed",
        };
        println!("    {}: {}", status_str, count);
    }

    if state.has_pending_changes() {
        println!("\n  Status: Has pending changes");
    } else {
        println!("\n  Status: All synced");
    }

    Ok(())
}

/// List sync conflicts.
async fn cmd_sync_conflicts(vault_path: &PathBuf) -> Result<()> {
    info!("Listing sync conflicts");

    let staging_dir = vault_path.join(".axiom_sync");
    let state_file = staging_dir.join("sync_state.json");

    if !state_file.exists() {
        println!("No sync state found. Vault has not been synced yet.");
        return Ok(());
    }

    let state_json = tokio::fs::read_to_string(&state_file)
        .await
        .context("Failed to read sync state")?;

    let state: SyncState =
        serde_json::from_str(&state_json).context("Failed to parse sync state")?;

    let conflicts = state.entries_with_status(axiomvault_sync::SyncStatus::Conflicted);

    if conflicts.is_empty() {
        println!("No conflicts found.");
    } else {
        println!("Sync Conflicts:");
        for entry in conflicts {
            println!("\n  Path: {}", entry.path);
            println!("    Local etag: {:?}", entry.local_etag);
            println!("    Remote etag: {:?}", entry.remote_etag);
            println!("    Local modified: {}", entry.local_modified);
            if let Some(remote_mod) = entry.remote_modified {
                println!("    Remote modified: {}", remote_mod);
            }
        }
        println!("\nUse 'axiomvault sync-resolve' to resolve conflicts.");
    }

    Ok(())
}

/// Resolve a sync conflict for a specific file.
async fn cmd_sync_resolve(vault_path: &PathBuf, file: &str, strategy: &str) -> Result<()> {
    info!("Resolving sync conflict for {}", file);

    let conflict_strategy = parse_conflict_strategy(strategy)?;
    let password = prompt_password("Enter password: ")?;
    let path_str = vault_path.to_string_lossy().to_string();

    let manager = VaultManager::new();
    let provider_config = serde_json::json!({
        "root": path_str
    });

    let session = manager
        .open_vault("local", provider_config, &password)
        .await
        .context("Failed to open vault")?;

    let sync_config = SyncConfig {
        conflict_strategy,
        ..Default::default()
    };

    let staging_dir = vault_path.join(".axiom_sync");
    let sync_engine: SyncEngine<dyn axiomvault_storage::StorageProvider> =
        SyncEngine::from_arc(session.provider(), &staging_dir, sync_config)
            .await
            .context("Failed to create sync engine")?;

    let file_path = VaultPath::parse(file).context("Invalid file path")?;

    // Read local file content for resolution
    let ops = VaultOperations::new(&session)?;
    let local_data = ops
        .read_file(&file_path)
        .await
        .context("Failed to read local file")?;

    sync_engine
        .resolve_conflict(&file_path, local_data, conflict_strategy)
        .await
        .context("Failed to resolve conflict")?;

    println!("Conflict resolved for {} using strategy: {}", file, strategy);

    Ok(())
}

/// Configure sync mode for the vault.
async fn cmd_sync_configure(vault_path: &PathBuf, mode: &str, interval: Option<u64>) -> Result<()> {
    info!("Configuring sync mode: {}", mode);

    let sync_mode = parse_sync_mode(mode, interval)?;

    let staging_dir = vault_path.join(".axiom_sync");
    tokio::fs::create_dir_all(&staging_dir)
        .await
        .context("Failed to create sync directory")?;

    let config_file = staging_dir.join("sync_config.json");

    let config = SyncConfig {
        sync_mode: sync_mode.clone(),
        ..Default::default()
    };

    let config_json =
        serde_json::to_string_pretty(&config).context("Failed to serialize config")?;

    tokio::fs::write(&config_file, config_json)
        .await
        .context("Failed to write config")?;

    let mode_str = match sync_mode {
        SyncMode::Manual => "Manual".to_string(),
        SyncMode::OnDemand => "On-demand".to_string(),
        SyncMode::Periodic { interval } => format!("Periodic (every {:?})", interval),
        SyncMode::Hybrid { interval } => format!("Hybrid (every {:?})", interval),
    };

    println!("Sync configuration updated!");
    println!("  Mode: {}", mode_str);
    println!("  Config saved to: {}", config_file.display());

    Ok(())
}
