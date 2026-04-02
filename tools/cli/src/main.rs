//! AxiomVault CLI - Command line interface for vault operations.
//!
//! This tool provides a command-line interface for creating, managing,
//! and operating on encrypted vaults.

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;
use url::Url;

use axiomvault_common::{VaultId, VaultPath};
use axiomvault_crypto::recovery::RecoveryKey;
use axiomvault_crypto::KdfParams;
use axiomvault_storage::gdrive::{AuthConfig, AuthManager, GDriveConfig, Tokens};
use axiomvault_storage::{
    create_default_registry, CompositeConfig, CompositeStorageProvider, HealthStatus, RaidMode,
    RaidRebuilder, RebuildConfig, RebuildResult,
};
use axiomvault_sync::{ConflictStrategy, SyncConfig, SyncEngine, SyncMode, SyncState};
use axiomvault_vault::{
    check_migration_needed, check_vault_health, check_vault_structure, MigrationRegistry,
    MigrationStatus, VaultConfig, VaultManager, VaultOperations, VaultVersion,
};

/// KDF strength level for key derivation.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum KdfStrength {
    /// Fast key derivation, lower security margin.
    Interactive,
    /// Balanced key derivation (default).
    Moderate,
    /// Slow key derivation, maximum security margin.
    Sensitive,
}

/// Conflict resolution strategy for sync operations.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum ConflictStrategyArg {
    /// Keep both local and remote versions.
    KeepBoth,
    /// Prefer the local version.
    PreferLocal,
    /// Prefer the remote version.
    PreferRemote,
}

/// Sync mode for vault synchronisation.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum SyncModeArg {
    /// Sync only when explicitly triggered.
    Manual,
    /// Sync on file changes.
    OnDemand,
    /// Sync at a fixed interval.
    Periodic,
    /// Combine on-demand and periodic sync.
    Hybrid,
}

/// RAID mode for CLI configuration.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum RaidModeArg {
    /// Mirror (RAID 1): replicate to all backends.
    Mirror,
    /// Erasure coding (RAID 5/6): Reed-Solomon sharding.
    Erasure,
}

/// Persistent RAID configuration stored at `<vault>/.axiomvault/raid.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RaidConfig {
    mode: RaidModeConfig,
    backends: Vec<BackendEntry>,
}

/// Serialised RAID mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RaidModeConfig {
    /// `"mirror"` or `"erasure"`.
    #[serde(rename = "type")]
    mode_type: String,
    /// Number of data shards (erasure only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data_shards: Option<usize>,
    /// Number of parity shards (erasure only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parity_shards: Option<usize>,
}

/// A single backend entry in the RAID config.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackendEntry {
    /// Provider type name (e.g. `"local"`, `"gdrive"`).
    provider_type: String,
    /// Provider-specific configuration (passed to `ProviderRegistry::resolve`).
    config: serde_json::Value,
}

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

        /// KDF strength level.
        #[arg(short, long, value_enum, default_value_t = KdfStrength::Moderate)]
        strength: KdfStrength,
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

    /// Show recovery key for a vault (requires password).
    ShowRecoveryKey {
        /// Path to the vault.
        #[arg(short, long)]
        path: PathBuf,
    },

    /// Reset vault password using recovery key words.
    ResetPassword {
        /// Path to the vault.
        #[arg(short, long)]
        path: PathBuf,
    },

    /// Migrate a legacy vault to support recovery keys.
    MigrateVault {
        /// Path to the vault.
        #[arg(short, long)]
        path: PathBuf,
    },

    /// Check vault health and integrity.
    Check {
        /// Path to the vault.
        #[arg(short, long)]
        path: PathBuf,

        /// Run shallow check only (no password required).
        #[arg(long)]
        shallow: bool,
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

        /// KDF strength level.
        #[arg(short, long, value_enum, default_value_t = KdfStrength::Moderate)]
        strength: KdfStrength,
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

        /// Conflict resolution strategy.
        #[arg(short, long, value_enum, default_value_t = ConflictStrategyArg::KeepBoth)]
        strategy: ConflictStrategyArg,
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

        /// Resolution strategy.
        #[arg(short, long, value_enum)]
        strategy: ConflictStrategyArg,
    },

    /// Configure sync mode for the vault.
    SyncConfigure {
        /// Path to the vault.
        #[arg(short = 'p', long)]
        vault_path: PathBuf,

        /// Sync mode.
        #[arg(short, long, value_enum)]
        mode: SyncModeArg,

        /// Interval in seconds for periodic sync (required for periodic/hybrid modes).
        #[arg(short, long)]
        interval: Option<u64>,
    },

    /// Migrate vault to the latest format version.
    Migrate {
        /// Path to the vault.
        #[arg(short, long)]
        path: PathBuf,

        /// Only show what migrations would run, without executing them.
        #[arg(long)]
        dry_run: bool,
    },

    /// Generate shell completions.
    Completions {
        /// Shell to generate completions for.
        #[arg(value_enum)]
        shell: Shell,

        /// Install completions to the standard location for the shell.
        #[arg(long)]
        install: bool,
    },

    /// Add a storage backend to the RAID pool.
    RaidAddBackend {
        /// Path to the vault.
        #[arg(short = 'p', long)]
        vault_path: PathBuf,

        /// Provider type (local, gdrive, dropbox, onedrive, icloud).
        #[arg(short = 't', long)]
        provider: String,

        /// Provider configuration as a JSON string.
        #[arg(short, long)]
        config: String,
    },

    /// Remove a storage backend from the RAID pool.
    RaidRemoveBackend {
        /// Path to the vault.
        #[arg(short = 'p', long)]
        vault_path: PathBuf,

        /// Index of the backend to remove (shown in raid-status).
        #[arg(short, long)]
        index: usize,
    },

    /// Show RAID status: mode, backends, health, and shard distribution.
    RaidStatus {
        /// Path to the vault.
        #[arg(short = 'p', long)]
        vault_path: PathBuf,
    },

    /// Rebuild missing shards on a target backend.
    RaidRebuild {
        /// Path to the vault.
        #[arg(short = 'p', long)]
        vault_path: PathBuf,

        /// Target backend index to rebuild (defaults to first degraded backend).
        #[arg(short = 't', long)]
        target: Option<usize>,
    },

    /// Configure or change the RAID mode.
    RaidConfigure {
        /// Path to the vault.
        #[arg(short = 'p', long)]
        vault_path: PathBuf,

        /// RAID mode.
        #[arg(long, value_enum)]
        mode: RaidModeArg,

        /// Number of data shards (required for erasure mode).
        #[arg(short = 'k', long)]
        data_shards: Option<usize>,

        /// Number of parity shards (required for erasure mode).
        #[arg(short = 'm', long)]
        parity_shards: Option<usize>,
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
        } => cmd_create(&name, &path, strength).await,

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

        Commands::ShowRecoveryKey { path } => cmd_show_recovery_key(&path).await,

        Commands::ResetPassword { path } => cmd_reset_password(&path).await,

        Commands::MigrateVault { path } => cmd_migrate_vault(&path).await,

        Commands::Check { path, shallow } => cmd_check(&path, shallow).await,

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
        } => cmd_gdrive_create(&name, &folder_id, &tokens, strength).await,

        Commands::GdriveOpen { folder_id, tokens } => cmd_gdrive_open(&folder_id, &tokens).await,

        Commands::Sync {
            vault_path,
            strategy,
        } => cmd_sync(&vault_path, strategy).await,

        Commands::SyncStatus { vault_path } => cmd_sync_status(&vault_path).await,

        Commands::SyncConflicts { vault_path } => cmd_sync_conflicts(&vault_path).await,

        Commands::SyncResolve {
            vault_path,
            file,
            strategy,
        } => cmd_sync_resolve(&vault_path, &file, strategy).await,

        Commands::SyncConfigure {
            vault_path,
            mode,
            interval,
        } => cmd_sync_configure(&vault_path, mode, interval).await,

        Commands::Migrate { path, dry_run } => cmd_migrate(&path, dry_run).await,

        Commands::Completions { shell, install } => {
            if install {
                install_completions(shell)?;
            } else {
                clap_complete::generate(
                    shell,
                    &mut Cli::command(),
                    "axiomvault",
                    &mut std::io::stdout(),
                );
            }
            Ok(())
        }

        Commands::RaidAddBackend {
            vault_path,
            provider,
            config,
        } => cmd_raid_add_backend(&vault_path, &provider, &config).await,

        Commands::RaidRemoveBackend { vault_path, index } => {
            cmd_raid_remove_backend(&vault_path, index).await
        }

        Commands::RaidStatus { vault_path } => cmd_raid_status(&vault_path).await,

        Commands::RaidRebuild { vault_path, target } => cmd_raid_rebuild(&vault_path, target).await,

        Commands::RaidConfigure {
            vault_path,
            mode,
            data_shards,
            parity_shards,
        } => cmd_raid_configure(&vault_path, mode, data_shards, parity_shards).await,
    }
}

/// Minimum password length enforced for new passwords, matching the UI clients.
const MIN_PASSWORD_LENGTH: usize = 8;

/// Validate that a password meets the minimum length requirement.
fn validate_password_strength(password: &[u8]) -> Result<()> {
    if password.len() < MIN_PASSWORD_LENGTH {
        anyhow::bail!(
            "Password must be at least {} characters",
            MIN_PASSWORD_LENGTH
        );
    }
    Ok(())
}

/// Prompt for password securely.
fn prompt_password(prompt: &str) -> Result<Vec<u8>> {
    // Allow non-interactive use via environment variable (useful for scripting/testing)
    if let Ok(pw) = std::env::var("AXIOMVAULT_PASSWORD") {
        if !pw.is_empty() {
            return Ok(pw.into_bytes());
        }
    }
    let password = rpassword::prompt_password(prompt).context("Failed to read password")?;
    Ok(password.into_bytes())
}

/// Display recovery words and prompt user to confirm they've saved them.
fn display_recovery_words(words: &str) {
    println!();
    println!("=== RECOVERY KEY ===");
    println!("Write down these 24 words and store them in a safe place.");
    println!("You will need them to recover your vault if you forget your password.");
    println!();
    for (i, word) in words.split_whitespace().enumerate() {
        println!("  {:>2}. {}", i + 1, word);
    }
    println!();
    println!("WARNING: This is the only time the recovery key will be shown.");
    println!("If you lose it, you will not be able to recover your vault.");
    println!();
    print!("Press Enter after you have written down the recovery key...");
    use std::io::Write;
    std::io::stdout().flush().ok();
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf).ok();
}

/// Convert KDF strength enum to crypto params.
fn kdf_params_from(strength: KdfStrength) -> KdfParams {
    match strength {
        KdfStrength::Interactive => KdfParams::interactive(),
        KdfStrength::Moderate => KdfParams::moderate(),
        KdfStrength::Sensitive => KdfParams::sensitive(),
    }
}

/// Convert conflict strategy enum to sync type.
fn conflict_strategy_from(arg: ConflictStrategyArg) -> ConflictStrategy {
    match arg {
        ConflictStrategyArg::KeepBoth => ConflictStrategy::KeepBoth,
        ConflictStrategyArg::PreferLocal => ConflictStrategy::PreferLocal,
        ConflictStrategyArg::PreferRemote => ConflictStrategy::PreferRemote,
    }
}

/// Convert sync mode enum to sync type.
fn sync_mode_from(arg: SyncModeArg, interval: Option<u64>) -> Result<SyncMode> {
    match arg {
        SyncModeArg::Manual => Ok(SyncMode::Manual),
        SyncModeArg::OnDemand => Ok(SyncMode::OnDemand),
        SyncModeArg::Periodic => {
            let secs =
                interval.ok_or_else(|| anyhow::anyhow!("Interval required for periodic mode"))?;
            Ok(SyncMode::Periodic {
                interval: std::time::Duration::from_secs(secs),
            })
        }
        SyncModeArg::Hybrid => {
            let secs =
                interval.ok_or_else(|| anyhow::anyhow!("Interval required for hybrid mode"))?;
            Ok(SyncMode::Hybrid {
                interval: std::time::Duration::from_secs(secs),
            })
        }
    }
}

/// Install shell completions to the standard location.
fn install_completions(shell: Shell) -> Result<()> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let (dir, filename) = match shell {
        Shell::Zsh => {
            let dir = PathBuf::from(&home).join(".zsh/completions");
            (dir, "_axiomvault".to_string())
        }
        Shell::Bash => {
            let dir = PathBuf::from(&home).join(".local/share/bash-completion/completions");
            (dir, "axiomvault".to_string())
        }
        Shell::Fish => {
            let dir = PathBuf::from(&home).join(".config/fish/completions");
            (dir, "axiomvault.fish".to_string())
        }
        _ => {
            anyhow::bail!(
                "Automatic installation not supported for {:?}. Use `axiomvault completions {:?}` and redirect to a file.",
                shell,
                shell,
            );
        }
    };

    std::fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;

    let dest = dir.join(&filename);
    let mut file = std::fs::File::create(&dest)
        .with_context(|| format!("Failed to create {}", dest.display()))?;
    clap_complete::generate(shell, &mut Cli::command(), "axiomvault", &mut file);

    println!("Completions installed to {}", dest.display());

    if shell == Shell::Zsh {
        println!();
        println!("Add the following to your ~/.zshrc if not already present:");
        println!();
        println!("  fpath=(~/.zsh/completions $fpath)");
        println!("  autoload -Uz compinit && compinit");
        println!();
        println!("Then restart your shell or run: exec zsh");
    }

    Ok(())
}

/// Create a new vault.
async fn cmd_create(name: &str, path: &Path, strength: KdfStrength) -> Result<()> {
    info!("Creating new vault: {}", name);

    let kdf_params = kdf_params_from(strength);

    let password = prompt_password("Enter password: ")?;
    let confirm = prompt_password("Confirm password: ")?;

    if password != confirm {
        anyhow::bail!("Passwords do not match");
    }

    validate_password_strength(&password)?;

    let vault_id = VaultId::new(name).context("Invalid vault name")?;
    let vault_path = path.to_string_lossy().to_string();

    let manager = VaultManager::new();
    let provider_config = serde_json::json!({
        "root": vault_path
    });

    let creation = manager
        .create_vault(vault_id, &password, "local", provider_config, kdf_params)
        .await
        .context("Failed to create vault")?;

    println!("Vault created successfully!");
    println!("  ID: {}", creation.session.vault_id());
    println!("  Location: {}", path.display());
    println!("  Provider: {}", creation.session.config().provider_type);
    display_recovery_words(&creation.recovery_words);

    Ok(())
}

/// Open vault for interactive session.
async fn cmd_open(path: &Path) -> Result<()> {
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
async fn cmd_list(vault_path: &Path, dir: &str) -> Result<()> {
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
async fn cmd_add(vault_path: &Path, source: &Path, dest: &str) -> Result<()> {
    info!("Adding file to vault");

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

    println!(
        "File added successfully: {} ({} bytes)",
        dest,
        content.len()
    );

    Ok(())
}

/// Extract a file from the vault.
async fn cmd_extract(vault_path: &Path, source: &str, dest: &Path) -> Result<()> {
    info!("Extracting file from vault");

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
async fn cmd_mkdir(vault_path: &Path, dir: &str) -> Result<()> {
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
async fn cmd_remove(vault_path: &Path, file: &str) -> Result<()> {
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
async fn cmd_info(path: &Path) -> Result<()> {
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
    println!(
        "  Version: {}.{}",
        config.version.major, config.version.minor
    );
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
async fn cmd_change_password(path: &Path) -> Result<()> {
    info!("Changing vault password");

    let old_password = prompt_password("Enter current password: ")?;
    let new_password = prompt_password("Enter new password: ")?;
    let confirm = prompt_password("Confirm new password: ")?;

    if new_password != confirm {
        anyhow::bail!("New passwords do not match");
    }

    validate_password_strength(&new_password)?;

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

/// Show recovery key for a vault.
async fn cmd_show_recovery_key(path: &Path) -> Result<()> {
    info!("Showing recovery key");

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

    let master_key = session.master_key().context("Session not active")?;
    let recovery_key = session
        .config()
        .decrypt_recovery_key(master_key)
        .context("Failed to decrypt recovery key. Vault may not have a recovery key.")?;

    let words = recovery_key
        .to_mnemonic()
        .context("Failed to encode recovery key")?;

    display_recovery_words(&words);

    Ok(())
}

/// Reset vault password using recovery key.
async fn cmd_reset_password(path: &Path) -> Result<()> {
    info!("Resetting vault password using recovery key");

    println!("Enter your 24-word recovery key (space-separated):");
    let mut recovery_input = String::new();
    std::io::stdin()
        .read_line(&mut recovery_input)
        .context("Failed to read recovery key")?;
    let recovery_words = recovery_input.trim();

    // Validate the recovery key format first.
    RecoveryKey::from_mnemonic(recovery_words)
        .context("Invalid recovery key. Please check your words and try again.")?;

    let new_password = prompt_password("Enter new password: ")?;
    let confirm = prompt_password("Confirm new password: ")?;

    if new_password != confirm {
        anyhow::bail!("Passwords do not match");
    }

    validate_password_strength(&new_password)?;

    let path_str = path.to_string_lossy().to_string();

    let manager = VaultManager::new();
    let provider_config = serde_json::json!({
        "root": path_str
    });

    let _session = manager
        .recover_vault("local", provider_config, recovery_words, &new_password)
        .await
        .context("Failed to reset password. Recovery key may be incorrect.")?;

    println!("Password reset successfully!");

    Ok(())
}

/// Migrate a legacy vault to support recovery keys.
async fn cmd_migrate_vault(path: &Path) -> Result<()> {
    info!("Migrating vault to v1.1 format");

    let password = prompt_password("Enter password: ")?;
    let path_str = path.to_string_lossy().to_string();

    let manager = VaultManager::new();
    let provider_config = serde_json::json!({
        "root": path_str
    });

    let mut session = manager
        .open_vault("local", provider_config, &password)
        .await
        .context("Failed to open vault")?;

    if !session.config().is_legacy_format() {
        println!("Vault is already in v1.1 format with recovery key support.");
        return Ok(());
    }

    let recovery_words = session
        .config_mut()
        .migrate_to_v1_1(&password)
        .context("Failed to migrate vault")?;

    // Save updated config.
    manager
        .save_config(&session)
        .await
        .context("Failed to save migrated config")?;

    println!("Vault migrated successfully to v1.1 format!");
    display_recovery_words(&recovery_words);

    Ok(())
}

/// Check vault health and integrity.
async fn cmd_check(path: &Path, shallow: bool) -> Result<()> {
    let path_str = path.to_string_lossy().to_string();

    let provider_config = serde_json::json!({
        "root": path_str
    });

    let manager = VaultManager::new();
    let provider = manager
        .registry()
        .resolve("local", provider_config.clone())
        .context("Failed to create storage provider")?;

    if shallow {
        info!("Running shallow vault check (no password required)");
        let report = check_vault_structure(provider.as_ref(), &path_str)
            .await
            .context("Failed to run shallow health check")?;

        print_health_report(&report);
        return Ok(());
    }

    info!("Running full vault health check");
    let password = prompt_password("Enter password: ")?;

    let session = manager
        .open_vault("local", provider_config, &password)
        .await
        .context("Failed to open vault")?;

    let master_key = session.master_key().context("Session not active")?;

    let report = check_vault_health(provider.as_ref(), session.config(), master_key, &path_str)
        .await
        .context("Failed to run health check")?;

    print_health_report(&report);

    Ok(())
}

/// Print a health report to stdout.
fn print_health_report(report: &axiomvault_vault::HealthReport) {
    println!("Vault Health Report: {}", report.vault_path);
    println!("{}", "=".repeat(50));

    for result in &report.results {
        let icon = match result.severity {
            axiomvault_vault::Severity::Info => "[OK]  ",
            axiomvault_vault::Severity::Warning => "[WARN]",
            axiomvault_vault::Severity::Error => "[ERR] ",
        };
        println!("  {} {}: {}", icon, result.check_name, result.message);
        if result.auto_fixable {
            println!("         (auto-fixable)");
        }
    }

    println!();
    if report.has_errors() {
        println!("Result: ERRORS FOUND");
    } else {
        println!("Result: HEALTHY");
    }
}

/// Authenticate with Google Drive and save tokens.
async fn cmd_gdrive_auth(
    client_id: Option<String>,
    client_secret: Option<String>,
    output: &PathBuf,
) -> Result<()> {
    info!("Starting Google Drive authentication");

    // Build auth config: CLI flags take precedence over environment variables.
    let client_id = client_id
        .or_else(|| std::env::var("AXIOMVAULT_GOOGLE_CLIENT_ID").ok())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Google OAuth2 client ID not provided. \
                 Use --client-id or set AXIOMVAULT_GOOGLE_CLIENT_ID"
            )
        })?;

    let client_secret = client_secret
        .or_else(|| std::env::var("AXIOMVAULT_GOOGLE_CLIENT_SECRET").ok())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Google OAuth2 client secret not provided. \
                 Use --client-secret or set AXIOMVAULT_GOOGLE_CLIENT_SECRET"
            )
        })?;

    let auth_config = AuthConfig {
        client_id,
        client_secret,
        redirect_url: "http://localhost:8080/callback".to_string(),
    };

    let auth_manager = AuthManager::new(auth_config).context("Failed to create auth manager")?;

    let (auth_url, csrf_token) = auth_manager.authorization_url();

    // Start local HTTP server to capture the OAuth callback
    let listener = TcpListener::bind("127.0.0.1:8080").await.context(
        "Failed to start local server on port 8080. Is another process using this port?",
    )?;

    println!("Starting Google Drive authentication...");
    println!();
    println!("Opening your browser to authorize AxiomVault...");

    // Try to open the browser automatically
    let browser_opened = open::that(&auth_url).is_ok();

    if browser_opened {
        println!("Browser opened successfully!");
    } else {
        println!("Could not open browser automatically.");
        println!("Please visit this URL to authorize:");
        println!();
        println!("  {}", auth_url);
    }

    println!();
    println!("Waiting for authorization... (Press Ctrl+C to cancel)");

    // Wait for the OAuth callback with a 5-minute timeout
    let (mut socket, _) =
        tokio::time::timeout(std::time::Duration::from_secs(300), listener.accept())
            .await
            .context("OAuth callback timed out after 5 minutes")?
            .context("Failed to accept connection")?;

    // Read the HTTP request
    let mut buffer = vec![0u8; 4096];
    let n = socket
        .read(&mut buffer)
        .await
        .context("Failed to read request")?;
    let request = String::from_utf8_lossy(&buffer[..n]);

    // Parse the request to extract the authorization code
    let first_line = request.lines().next().unwrap_or("");
    let path = first_line.split_whitespace().nth(1).unwrap_or("/");

    // Extract code and state from the callback URL
    let callback_url = format!("http://localhost:8080{}", path);
    let parsed_url = Url::parse(&callback_url).context("Failed to parse callback URL")?;

    let mut code = None;
    let mut state = None;

    for (key, value) in parsed_url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.to_string()),
            "state" => state = Some(value.to_string()),
            _ => {}
        }
    }

    let auth_code = code.ok_or_else(|| anyhow::anyhow!("No authorization code received"))?;
    let received_state = state.ok_or_else(|| anyhow::anyhow!("No state parameter received"))?;

    // Verify CSRF token
    if received_state != csrf_token {
        // Send error response
        let error_html = r#"<!DOCTYPE html>
<html>
<head><title>Authentication Failed</title></head>
<body style="font-family: sans-serif; text-align: center; padding: 50px;">
<h1 style="color: #d32f2f;">Authentication Failed</h1>
<p>Security validation failed. Please try again.</p>
</body>
</html>"#;
        let response = format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
            error_html.len(),
            error_html
        );
        socket.write_all(response.as_bytes()).await.ok();
        anyhow::bail!("CSRF token mismatch - possible security issue");
    }

    // Send success response to browser
    let success_html = r#"<!DOCTYPE html>
<html>
<head><title>Authentication Successful</title></head>
<body style="font-family: sans-serif; text-align: center; padding: 50px;">
<h1 style="color: #4caf50;">Authentication Successful!</h1>
<p>You have successfully authorized AxiomVault to access your Google Drive.</p>
<p>You can close this window and return to the terminal.</p>
</body>
</html>"#;

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
        success_html.len(),
        success_html
    );
    socket.write_all(response.as_bytes()).await.ok();

    info!("Authorization code received, exchanging for tokens");
    println!();
    println!("Authorization received! Exchanging for access tokens...");

    let tokens = auth_manager
        .exchange_code(&auth_code)
        .await
        .context("Failed to exchange authorization code")?;

    // Save tokens to file with restricted permissions
    let tokens_json =
        serde_json::to_string_pretty(&tokens).context("Failed to serialize tokens")?;

    tokio::fs::write(output, &tokens_json)
        .await
        .context("Failed to write tokens file")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(output, perms).context("Failed to set token file permissions")?;
    }

    println!();
    println!("Authentication successful!");
    println!("  Tokens saved to: {}", output.display());
    println!("  Expires at: {}", tokens.expires_at);
    println!();
    println!("You can now use 'axiomvault gdrive-create' or 'axiomvault gdrive-open'");

    Ok(())
}

/// Create a vault on Google Drive.
async fn cmd_gdrive_create(
    name: &str,
    folder_id: &str,
    tokens_path: &Path,
    strength: KdfStrength,
) -> Result<()> {
    info!("Creating new vault on Google Drive: {}", name);

    let kdf_params = kdf_params_from(strength);

    let password = prompt_password("Enter password: ")?;
    let confirm = prompt_password("Confirm password: ")?;

    if password != confirm {
        anyhow::bail!("Passwords do not match");
    }

    validate_password_strength(&password)?;

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

    let creation = manager
        .create_vault(vault_id, &password, "gdrive", provider_config, kdf_params)
        .await
        .context("Failed to create vault on Google Drive")?;

    println!("Vault created successfully on Google Drive!");
    println!("  ID: {}", creation.session.vault_id());
    println!("  Folder ID: {}", folder_id);
    println!("  Provider: {}", creation.session.config().provider_type);
    display_recovery_words(&creation.recovery_words);

    Ok(())
}

/// Open a vault on Google Drive.
async fn cmd_gdrive_open(folder_id: &str, tokens_path: &Path) -> Result<()> {
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

/// Sync vault with remote storage.
async fn cmd_sync(vault_path: &Path, strategy: ConflictStrategyArg) -> Result<()> {
    info!("Starting vault sync");

    let conflict_strategy = conflict_strategy_from(strategy);
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
    let result = sync_engine.sync_full().await.context("Sync failed")?;

    println!("Sync completed!");
    println!("  Files synced: {}", result.files_synced);
    println!("  Files failed: {}", result.files_failed);
    println!("  Conflicts found: {}", result.conflicts_found);
    println!("  Duration: {:?}", result.duration);

    Ok(())
}

/// Show sync status for the vault.
async fn cmd_sync_status(vault_path: &Path) -> Result<()> {
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
async fn cmd_sync_conflicts(vault_path: &Path) -> Result<()> {
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
async fn cmd_sync_resolve(
    vault_path: &Path,
    file: &str,
    strategy: ConflictStrategyArg,
) -> Result<()> {
    info!("Resolving sync conflict for {}", file);

    let conflict_strategy = conflict_strategy_from(strategy);
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

    println!(
        "Conflict resolved for {} using strategy: {:?}",
        file, strategy
    );

    Ok(())
}

/// Configure sync mode for the vault.
async fn cmd_sync_configure(
    vault_path: &Path,
    mode: SyncModeArg,
    interval: Option<u64>,
) -> Result<()> {
    info!("Configuring sync mode: {:?}", mode);

    let sync_mode = sync_mode_from(mode, interval)?;

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

/// Detect and run vault format migrations.
async fn cmd_migrate(path: &Path, dry_run: bool) -> Result<()> {
    info!("Checking vault migration status: {}", path.display());

    let config_path = path.join("vault.config");
    if !config_path.exists() {
        anyhow::bail!("No vault found at {}", path.display());
    }

    let config_bytes = tokio::fs::read(&config_path)
        .await
        .context("Failed to read vault config")?;
    let mut config = VaultConfig::from_bytes(&config_bytes).context("Failed to parse config")?;

    let status = check_migration_needed(&config);

    match &status {
        MigrationStatus::UpToDate => {
            println!("Vault is up to date (version {}).", config.version);
            return Ok(());
        }
        MigrationStatus::Incompatible { version } => {
            anyhow::bail!(
                "Vault version {} is incompatible with this software (current: {})",
                version,
                VaultVersion::CURRENT
            );
        }
        MigrationStatus::NeedsMigration { from, to } => {
            println!("Migration needed: {} -> {}", from, to);
        }
    }

    let registry = MigrationRegistry::default();
    let target = VaultVersion::CURRENT;

    if let Some(steps) = registry.find_path(&config.version, &target) {
        println!("Migration plan ({} step(s)):", steps.len());
        for (i, step) in steps.iter().enumerate() {
            println!(
                "  {}. {} -> {}: {}",
                i + 1,
                step.source_version(),
                step.target_version(),
                step.description()
            );
        }
    } else {
        anyhow::bail!(
            "No migration path found from {} to {}",
            config.version,
            target
        );
    }

    if dry_run {
        println!("\nDry run complete. No changes were made.");
        return Ok(());
    }

    println!("\nRunning migrations...");
    registry
        .migrate(path, &mut config, &target)
        .context("Migration failed")?;

    println!(
        "Migration completed successfully! Vault is now at version {}.",
        config.version
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// RAID helpers
// ---------------------------------------------------------------------------

/// Path to the RAID configuration file within a vault directory.
fn raid_config_path(vault_path: &Path) -> PathBuf {
    vault_path.join(".axiomvault").join("raid.json")
}

/// Load the RAID configuration from disk.  Returns `None` if not found.
async fn load_raid_config(vault_path: &Path) -> Result<Option<RaidConfig>> {
    let path = raid_config_path(vault_path);
    if !path.exists() {
        return Ok(None);
    }
    let data = tokio::fs::read_to_string(&path)
        .await
        .context("Failed to read RAID config")?;
    let cfg: RaidConfig = serde_json::from_str(&data).context("Failed to parse RAID config")?;
    Ok(Some(cfg))
}

/// Save the RAID configuration atomically (write-then-rename).
async fn save_raid_config(vault_path: &Path, config: &RaidConfig) -> Result<()> {
    let dir = vault_path.join(".axiomvault");
    tokio::fs::create_dir_all(&dir)
        .await
        .context("Failed to create .axiomvault directory")?;

    let tmp_path = dir.join("raid.json.tmp");
    let final_path = dir.join("raid.json");

    let json = serde_json::to_string_pretty(config).context("Failed to serialize RAID config")?;
    tokio::fs::write(&tmp_path, &json)
        .await
        .context("Failed to write RAID config tmp")?;
    tokio::fs::rename(&tmp_path, &final_path)
        .await
        .context("Failed to rename RAID config")?;
    Ok(())
}

/// Convert a `RaidConfig` into a `RaidMode`.
fn raid_mode_from_config(cfg: &RaidModeConfig) -> Result<RaidMode> {
    match cfg.mode_type.as_str() {
        "mirror" => Ok(RaidMode::Mirror),
        "erasure" => {
            let data_shards = cfg
                .data_shards
                .ok_or_else(|| anyhow::anyhow!("erasure mode requires data_shards"))?;
            let parity_shards = cfg
                .parity_shards
                .ok_or_else(|| anyhow::anyhow!("erasure mode requires parity_shards"))?;
            Ok(RaidMode::Erasure {
                data_shards,
                parity_shards,
            })
        }
        other => anyhow::bail!("Unknown RAID mode type: {}", other),
    }
}

/// Build a `CompositeStorageProvider` from a persisted `RaidConfig`.
fn build_composite(config: &RaidConfig) -> Result<CompositeStorageProvider> {
    let registry = create_default_registry();
    let mut backends: Vec<Arc<dyn axiomvault_storage::StorageProvider>> = Vec::new();

    for (i, entry) in config.backends.iter().enumerate() {
        let provider = registry
            .resolve(&entry.provider_type, entry.config.clone())
            .with_context(|| format!("Failed to create backend {} ({})", i, entry.provider_type))?;
        backends.push(provider);
    }

    let mode = raid_mode_from_config(&config.mode)?;
    let composite_config = CompositeConfig {
        mode,
        health: Default::default(),
    };

    let composite = CompositeStorageProvider::new(backends, composite_config)
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(composite)
}

// ---------------------------------------------------------------------------
// RAID commands
// ---------------------------------------------------------------------------

/// Add a storage backend to the RAID pool.
async fn cmd_raid_add_backend(vault_path: &Path, provider: &str, config_json: &str) -> Result<()> {
    info!("Adding backend to RAID pool: {}", provider);

    // Validate provider type and config by trying to resolve it.
    let provider_config: serde_json::Value =
        serde_json::from_str(config_json).context("Invalid JSON config")?;
    let registry = create_default_registry();
    registry
        .resolve(provider, provider_config.clone())
        .with_context(|| format!("Failed to create provider '{}'", provider))?;

    let entry = BackendEntry {
        provider_type: provider.to_string(),
        config: provider_config,
    };

    let mut raid_cfg = load_raid_config(vault_path)
        .await?
        .unwrap_or_else(|| RaidConfig {
            mode: RaidModeConfig {
                mode_type: "mirror".to_string(),
                data_shards: None,
                parity_shards: None,
            },
            backends: Vec::new(),
        });

    raid_cfg.backends.push(entry);

    // If we now have >=2 backends and a composite can be formed, sync the shard map.
    if raid_cfg.backends.len() >= 2 {
        match build_composite(&raid_cfg) {
            Ok(composite) => {
                if let Err(e) = composite.load_shard_map().await {
                    eprintln!("Warning: could not load shard map: {}", e);
                } else if let Err(e) = composite.save_shard_map().await {
                    eprintln!("Warning: could not sync shard map to new backend: {}", e);
                }
            }
            Err(e) => {
                eprintln!(
                    "Warning: cannot form composite yet ({}). \
                     You may need to run raid-configure after adding enough backends.",
                    e
                );
            }
        }
    }

    save_raid_config(vault_path, &raid_cfg).await?;

    println!("Backend added successfully!");
    println!("  Index: {}", raid_cfg.backends.len() - 1);
    println!("  Provider: {}", provider);
    println!("  Total backends: {}", raid_cfg.backends.len());

    Ok(())
}

/// Remove a storage backend from the RAID pool.
///
/// In erasure mode, shards on the removed backend are rebuilt onto remaining
/// backends before removal. In mirror mode, data exists on all other backends
/// so no migration is needed.
async fn cmd_raid_remove_backend(vault_path: &Path, index: usize) -> Result<()> {
    info!("Removing backend {} from RAID pool", index);

    let mut raid_cfg = load_raid_config(vault_path).await?.ok_or_else(|| {
        anyhow::anyhow!("No RAID configuration found. Run raid-add-backend first.")
    })?;

    if index >= raid_cfg.backends.len() {
        anyhow::bail!(
            "Backend index {} out of range (have {} backends)",
            index,
            raid_cfg.backends.len()
        );
    }

    // Check minimum redundancy.
    let remaining = raid_cfg.backends.len() - 1;
    let min_required = match raid_cfg.mode.mode_type.as_str() {
        "mirror" => 2,
        "erasure" => {
            let ds = raid_cfg.mode.data_shards.unwrap_or(0);
            let ps = raid_cfg.mode.parity_shards.unwrap_or(0);
            ds + ps
        }
        _ => 2,
    };

    if remaining < min_required {
        anyhow::bail!(
            "Cannot remove backend: {} mode requires at least {} backends, \
             but only {} would remain",
            raid_cfg.mode.mode_type,
            min_required,
            remaining
        );
    }

    if raid_cfg.mode.mode_type == "erasure" && remaining == min_required {
        eprintln!(
            "Warning: After removal, the array will have zero fault tolerance. \
             Any backend failure will result in data loss."
        );
    }

    // Migrate shards before removal if the backend holds any data.
    if raid_cfg.backends.len() >= 2 {
        let composite = build_composite(&raid_cfg)?;
        composite.load_shard_map().await?;

        let shard_map = composite.get_shard_map().await;
        let has_shards = shard_map
            .entries
            .values()
            .any(|entry| entry.shards.contains_key(&index));

        if has_shards {
            match raid_cfg.mode.mode_type.as_str() {
                "mirror" => {
                    // Mirror mode: data exists on all other backends,
                    // no migration needed. Just clean up the shard map.
                    eprintln!(
                        "Mirror mode: data is replicated on other backends, \
                         no migration needed."
                    );
                }
                "erasure" => {
                    // Erasure mode: rebuild shards onto another backend
                    // before removing this one.
                    eprintln!(
                        "Erasure mode: rebuilding shards from backend {} \
                         before removal...",
                        index
                    );

                    // Pick a rebuild target: first healthy backend that isn't
                    // the one being removed.
                    let mut rebuild_target = None;
                    for i in 0..composite.backend_count() {
                        if i == index {
                            continue;
                        }
                        if let Some(h) = composite.backend_health(i).await {
                            if h.status == HealthStatus::Healthy {
                                rebuild_target = Some(i);
                                break;
                            }
                        }
                    }

                    let target = rebuild_target.ok_or_else(|| {
                        anyhow::anyhow!(
                            "No healthy backend available to receive \
                             redistributed shards. Aborting removal."
                        )
                    })?;

                    let rebuilder =
                        RaidRebuilder::new(&composite, target, RebuildConfig::default())
                            .map_err(|e| anyhow::anyhow!("{}", e))?;

                    let result = rebuilder.rebuild().await.map_err(|e| {
                        anyhow::anyhow!("Shard migration failed, aborting removal: {}", e)
                    })?;

                    eprintln!(
                        "Migration complete: {} rebuilt, {} skipped, {} failed",
                        result.rebuilt, result.skipped, result.failed
                    );

                    if result.failed > 0 {
                        anyhow::bail!(
                            "Shard migration had {} failures. \
                             Aborting removal to prevent data loss.",
                            result.failed
                        );
                    }
                }
                _ => {}
            }
        }

        // Clone the current shard map before mutating so we can roll back.
        let shard_map_backup = composite.get_shard_map().await;

        // Compute the updated shard map: remove all references to the removed
        // backend and re-index higher backend indices.
        {
            let mut map = composite.shard_map_ref().write().await;
            for entry in map.entries.values_mut() {
                entry.shards.remove(&index);
                // Re-index: shift down any shard indices above the removed one.
                let shifted: std::collections::HashMap<usize, _> = entry
                    .shards
                    .drain()
                    .map(|(k, mut loc)| {
                        let new_idx = if k > index { k - 1 } else { k };
                        loc.shard_index = new_idx;
                        (new_idx, loc)
                    })
                    .collect();
                entry.shards = shifted;
            }
            map.version += 1;
            map.updated_at = chrono::Utc::now();
        }

        // Persist updated shard map first (step 1 of atomic commit).
        composite.save_shard_map().await?;

        // Remove the backend from config and persist (step 2 of atomic commit).
        let removed = raid_cfg.backends.remove(index);
        if let Err(e) = save_raid_config(vault_path, &raid_cfg).await {
            // Roll back the shard map to the pre-removal state.
            eprintln!("Failed to save RAID config, rolling back shard map...");
            {
                let mut map = composite.shard_map_ref().write().await;
                *map = shard_map_backup;
            }
            composite.save_shard_map().await.map_err(|rollback_err| {
                anyhow::anyhow!(
                    "CRITICAL: config save failed ({}) and shard map rollback \
                         also failed ({}). Manual recovery may be needed.",
                    e,
                    rollback_err
                )
            })?;
            // Re-insert the backend so the in-memory state is consistent.
            raid_cfg.backends.insert(index, removed);
            return Err(e);
        }

        println!("Backend removed successfully!");
        println!("  Removed: {} (index {})", removed.provider_type, index);
        println!("  Remaining backends: {}", raid_cfg.backends.len());
        return Ok(());
    }

    let removed = raid_cfg.backends.remove(index);
    save_raid_config(vault_path, &raid_cfg).await?;

    println!("Backend removed successfully!");
    println!("  Removed: {} (index {})", removed.provider_type, index);
    println!("  Remaining backends: {}", raid_cfg.backends.len());

    Ok(())
}

/// Show RAID status: mode, backends, health, and shard distribution.
async fn cmd_raid_status(vault_path: &Path) -> Result<()> {
    let raid_cfg = load_raid_config(vault_path).await?.ok_or_else(|| {
        anyhow::anyhow!("No RAID configuration found. Run raid-add-backend first.")
    })?;

    // Print RAID mode.
    println!("RAID Configuration");
    println!("{}", "=".repeat(60));
    match raid_cfg.mode.mode_type.as_str() {
        "mirror" => println!("  Mode: Mirror (RAID 1)"),
        "erasure" => println!(
            "  Mode: Erasure (RAID 5/6) — k={}, m={}",
            raid_cfg.mode.data_shards.unwrap_or(0),
            raid_cfg.mode.parity_shards.unwrap_or(0),
        ),
        other => println!("  Mode: {}", other),
    }
    println!("  Backends: {}", raid_cfg.backends.len());
    println!();

    if raid_cfg.backends.len() < 2 {
        println!("Backends:");
        for (i, entry) in raid_cfg.backends.iter().enumerate() {
            println!(
                "  [{}] {} — not enough backends for RAID",
                i, entry.provider_type
            );
        }
        return Ok(());
    }

    // Build the composite and show live health.
    let composite = build_composite(&raid_cfg)?;
    if let Err(e) = composite.load_shard_map().await {
        eprintln!("Warning: could not load shard map: {}", e);
    }

    let shard_map = composite.get_shard_map().await;

    // Count shards per backend.
    let backend_count = composite.backend_count();
    let mut shard_counts = vec![0usize; backend_count];
    for entry in shard_map.entries.values() {
        for shard in entry.shards.values() {
            if shard.shard_index < backend_count {
                shard_counts[shard.shard_index] += 1;
            }
        }
    }

    println!(
        "  {:<6} {:<12} {:<10} {:<8} Last Success",
        "Index", "Provider", "Health", "Shards"
    );
    println!("  {}", "-".repeat(54));

    for (i, shard_count) in shard_counts.iter().enumerate() {
        let health = composite.backend_health(i).await;
        let (status_str, last_success) = match &health {
            Some(h) => {
                let status = match h.status {
                    HealthStatus::Healthy => "Healthy",
                    HealthStatus::Degraded => "Degraded",
                    HealthStatus::Offline => "Offline",
                };
                let last = h
                    .last_success
                    .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "never".to_string());
                (status, last)
            }
            None => ("Unknown", "n/a".to_string()),
        };

        println!(
            "  {:<6} {:<12} {:<10} {:<8} {}",
            i, raid_cfg.backends[i].provider_type, status_str, shard_count, last_success
        );
    }

    println!();
    println!(
        "Shard Map: {} entries, version {}",
        shard_map.entries.len(),
        shard_map.version
    );

    // Redundancy summary.
    let healthy = composite.healthy_backend_count().await;
    println!("Redundancy: {}/{} backends healthy", healthy, backend_count);

    Ok(())
}

/// Rebuild missing shards on a target backend.
async fn cmd_raid_rebuild(vault_path: &Path, target: Option<usize>) -> Result<()> {
    let raid_cfg = load_raid_config(vault_path).await?.ok_or_else(|| {
        anyhow::anyhow!("No RAID configuration found. Run raid-add-backend first.")
    })?;

    if raid_cfg.backends.len() < 2 {
        anyhow::bail!("Need at least 2 backends for RAID rebuild");
    }

    let composite = build_composite(&raid_cfg)?;

    // Load the shard map so the rebuilder knows which shards need reconstruction.
    composite
        .load_shard_map()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load shard map: {}", e))?;

    // Determine target: explicit index or first non-healthy backend.
    let target_index = match target {
        Some(idx) => {
            if idx >= composite.backend_count() {
                anyhow::bail!(
                    "Target index {} out of range (have {} backends)",
                    idx,
                    composite.backend_count()
                );
            }
            idx
        }
        None => {
            // Find first degraded/offline backend.
            let mut found = None;
            for i in 0..composite.backend_count() {
                if let Some(h) = composite.backend_health(i).await {
                    if h.status != HealthStatus::Healthy {
                        found = Some(i);
                        break;
                    }
                }
            }
            found.ok_or_else(|| {
                anyhow::anyhow!(
                    "All backends are healthy. Use --target to force rebuild on a specific backend."
                )
            })?
        }
    };

    println!(
        "Rebuilding backend {} ({})...",
        target_index, raid_cfg.backends[target_index].provider_type
    );

    let rebuilder = RaidRebuilder::new(&composite, target_index, RebuildConfig::default())
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    // Run the rebuild with a progress bar by polling progress every 500ms.
    let result = rebuild_with_progress(&rebuilder).await?;

    println!("\nRebuild completed!");
    println!("  Rebuilt:  {}", result.rebuilt);
    println!("  Skipped:  {}", result.skipped);
    println!("  Failed:   {}", result.failed);
    println!("  Elapsed:  {:.1?}", result.elapsed);

    Ok(())
}

/// Run a rebuild while displaying a text progress bar on stderr.
///
/// Uses `tokio::select!` to poll `rebuilder.progress()` every 500ms while the
/// rebuild future runs concurrently on the same task.
async fn rebuild_with_progress(rebuilder: &RaidRebuilder<'_>) -> Result<RebuildResult> {
    use tokio::pin;

    let rebuild_fut = rebuilder.rebuild();
    pin!(rebuild_fut);

    let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
    // First tick fires immediately; consume it so the first real tick is at 500ms.
    interval.tick().await;

    loop {
        tokio::select! {
            result = &mut rebuild_fut => {
                // Print final progress state before returning.
                let progress = rebuilder.progress().await;
                print_progress_bar(&progress);
                return result.map_err(|e| anyhow::anyhow!("{}", e));
            }
            _ = interval.tick() => {
                let progress = rebuilder.progress().await;
                print_progress_bar(&progress);
            }
        }
    }
}

/// Print a single-line progress bar to stderr, overwriting the previous line.
///
/// Format: `[=====>    ] 55% (12/22 chunks, ETA: 3s)`
fn print_progress_bar(progress: &axiomvault_storage::rebuild::RebuildProgress) {
    use std::io::Write;

    let pct = progress.percentage();
    let done = progress.completed + progress.skipped + progress.failed;
    let total = progress.total;

    // Build bar: 30 characters wide.
    let bar_width = 30;
    let filled = ((pct / 100.0) * bar_width as f64) as usize;
    let filled = filled.min(bar_width);
    let empty = bar_width - filled;

    let arrow = if filled > 0 && filled < bar_width {
        format!("{}>{}", "=".repeat(filled - 1), " ".repeat(empty))
    } else if filled == bar_width {
        "=".repeat(bar_width)
    } else {
        " ".repeat(bar_width)
    };

    let eta_str = match progress.eta() {
        Some(d) => format!(", ETA: {}s", d.as_secs()),
        None => String::new(),
    };

    eprint!(
        "\r[{}] {:>3.0}% ({}/{} chunks{})",
        arrow, pct, done, total, eta_str,
    );
    let _ = std::io::stderr().flush();
}

/// Configure or change the RAID mode.
async fn cmd_raid_configure(
    vault_path: &Path,
    mode: RaidModeArg,
    data_shards: Option<usize>,
    parity_shards: Option<usize>,
) -> Result<()> {
    info!("Configuring RAID mode");

    let mut raid_cfg = load_raid_config(vault_path).await?.ok_or_else(|| {
        anyhow::anyhow!("No RAID configuration found. Run raid-add-backend first.")
    })?;

    let mode_config = match mode {
        RaidModeArg::Mirror => RaidModeConfig {
            mode_type: "mirror".to_string(),
            data_shards: None,
            parity_shards: None,
        },
        RaidModeArg::Erasure => {
            let k = data_shards
                .ok_or_else(|| anyhow::anyhow!("Erasure mode requires --data-shards (-k)"))?;
            let m = parity_shards
                .ok_or_else(|| anyhow::anyhow!("Erasure mode requires --parity-shards (-m)"))?;

            if k == 0 {
                anyhow::bail!("data-shards must be at least 1");
            }
            if m == 0 {
                anyhow::bail!("parity-shards must be at least 1");
            }
            if k + m != raid_cfg.backends.len() {
                anyhow::bail!(
                    "data-shards ({}) + parity-shards ({}) must equal backend count ({})",
                    k,
                    m,
                    raid_cfg.backends.len()
                );
            }

            RaidModeConfig {
                mode_type: "erasure".to_string(),
                data_shards: Some(k),
                parity_shards: Some(m),
            }
        }
    };

    raid_cfg.mode = mode_config;
    save_raid_config(vault_path, &raid_cfg).await?;

    match mode {
        RaidModeArg::Mirror => println!("RAID mode set to Mirror (RAID 1)."),
        RaidModeArg::Erasure => {
            let k = data_shards.expect("validated above");
            let m = parity_shards.expect("validated above");
            println!("RAID mode set to Erasure (k={k}, m={m}).");
        }
    }

    Ok(())
}
