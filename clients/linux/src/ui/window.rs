//! Main application window with navigation stack.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::app::AppState;
use crate::ui::unlock::UnlockView;

/// Build the main application window.
pub fn build_window(app: &adw::Application, state: Rc<RefCell<AppState>>) {
    let nav_view = adw::NavigationView::new();

    // Start with the unlock/open view.
    let unlock_view = UnlockView::new(Rc::clone(&state), nav_view.clone());
    nav_view.add(&unlock_view.page());

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("AxiomVault")
        .default_width(900)
        .default_height(600)
        .content(&nav_view)
        .build();

    // Subscribe to vault events and forward to UI.
    {
        let state_ref = state.borrow();
        let mut rx = state_ref.service.subscribe();
        let nav = nav_view.clone();
        let st = Rc::clone(&state);

        state_ref.runtime.spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let nav = nav.clone();
                        let st = Rc::clone(&st);
                        glib::MainContext::default().spawn_local(async move {
                            handle_event(&nav, &st, event);
                        });
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        });
    }

    window.present();
}

fn handle_event(
    _nav: &adw::NavigationView,
    _state: &Rc<RefCell<AppState>>,
    event: axiomvault_app::AppEvent,
) {
    match event {
        axiomvault_app::AppEvent::VaultOpened(_)
        | axiomvault_app::AppEvent::VaultCreated(_) => {
            tracing::info!("Vault opened — switch to browser view");
            // TODO: push browser page onto nav_view
        }
        axiomvault_app::AppEvent::VaultClosed | axiomvault_app::AppEvent::VaultLocked => {
            tracing::info!("Vault closed — return to unlock view");
            // TODO: pop to root
        }
        axiomvault_app::AppEvent::Error { message } => {
            tracing::error!("Core error: {}", message);
        }
        _ => {}
    }
}
