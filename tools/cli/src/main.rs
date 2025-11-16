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
