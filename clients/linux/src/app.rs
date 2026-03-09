//! Application bootstrap and GTK/tokio integration.
//!
//! The GTK main loop runs on the main thread. Async vault operations are
//! dispatched to a background tokio runtime. Results are forwarded back
//! to the main thread via `glib::MainContext::default().spawn_local()`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::gio;
use tokio::runtime::Runtime;

use axiomvault_app::AppService;

use crate::ui;

/// Shared application state accessible from GTK callbacks.
pub struct AppState {
    pub service: AppService,
    pub runtime: Arc<Runtime>,
}

impl AppState {
    fn new() -> Self {
        let runtime = Runtime::new().expect("failed to create tokio runtime");
        Self {
            service: AppService::new(),
            runtime: Arc::new(runtime),
        }
    }
}

/// Run the GTK application. Returns the exit code.
pub fn run() -> i32 {
    let app = adw::Application::builder()
        .application_id("com.axiomvault.linux")
        .flags(gio::ApplicationFlags::FLAGS_NONE)
        .build();

    let state = Rc::new(RefCell::new(AppState::new()));

    app.connect_activate(move |app| {
        let state = Rc::clone(&state);
        ui::build_window(app, state);
    });

    app.run().into()
}

/// Spawn an async task on the tokio runtime and forward the result to the GTK
/// main thread via a callback.
///
/// # Usage
/// ```ignore
/// spawn_async(&state, |service| async move {
///     service.list_directory("/").await
/// }, |result| {
///     // runs on GTK main thread
/// });
/// ```
pub fn spawn_async<F, Fut, T, C>(state: &AppState, task: F, on_done: C)
where
    F: FnOnce(AppService) -> Fut + 'static,
    Fut: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
    C: FnOnce(T) + 'static,
{
    let service = state.service.clone();
    let ctx = glib::MainContext::default();

    state.runtime.spawn(async move {
        let result = task(service).await;
        ctx.spawn_local(async move {
            on_done(result);
        });
    });
}
