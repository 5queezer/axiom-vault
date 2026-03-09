//! Application facade for AxiomVault.
//!
//! This crate provides a unified, high-level API that wraps the vault, storage,
//! and sync subsystems into a single stateful service. It is the sole interface
//! that platform UI shells (SwiftUI, GTK4) should use.
//!
//! # Design principles
//! - **Single entry point**: All vault operations go through [`AppService`].
//! - **Event-driven**: State changes are broadcast via [`AppEvent`] over a
//!   tokio broadcast channel.
//! - **DTO boundary**: UI layers receive plain [`dto`] structs, never internal
//!   domain types.
//! - **Thread-safe**: `AppService` is `Send + Sync` and safe to share across
//!   threads via `Arc`.

pub mod dto;
pub mod error;
pub mod events;
pub mod local_index;
pub mod service;

pub use dto::*;
pub use error::{AppError, AppResult};
pub use events::{AppEvent, EventReceiver, EventSender};
pub use local_index::{IndexEntry, LocalIndex};
pub use service::AppService;
