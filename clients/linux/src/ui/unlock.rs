//! Unlock / open vault view.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use axiomvault_app::OpenVaultParams;

use crate::app::{self, AppState};

/// The initial view: enter a vault path and password to unlock.
pub struct UnlockView {
    page: adw::NavigationPage,
}

impl UnlockView {
    pub fn new(state: Rc<RefCell<AppState>>, nav: adw::NavigationView) -> Self {
        let path_row = adw::EntryRow::builder().title("Vault path").build();

        let password_row = adw::PasswordEntryRow::builder().title("Password").build();

        let open_button = gtk::Button::builder()
            .label("Open Vault")
            .css_classes(["suggested-action", "pill"])
            .halign(gtk::Align::Center)
            .margin_top(24)
            .build();

        let create_button = gtk::Button::builder()
            .label("Create New Vault")
            .css_classes(["pill"])
            .halign(gtk::Align::Center)
            .margin_top(12)
            .build();

        let status_label = gtk::Label::builder()
            .css_classes(["dim-label"])
            .halign(gtk::Align::Center)
            .margin_top(12)
            .build();

        let group = adw::PreferencesGroup::builder()
            .title("Open Vault")
            .margin_start(24)
            .margin_end(24)
            .margin_top(24)
            .build();
        group.add(&path_row);
        group.add(&password_row);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&group);
        content.append(&open_button);
        content.append(&create_button);
        content.append(&status_label);

        let clamp = adw::Clamp::builder()
            .maximum_size(500)
            .child(&content)
            .build();

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&adw::HeaderBar::new());
        toolbar_view.set_content(Some(&clamp));

        let page = adw::NavigationPage::builder()
            .title("AxiomVault")
            .tag("unlock")
            .child(&toolbar_view)
            .build();

        // Open vault action
        {
            let path_row = path_row.clone();
            let password_row = password_row.clone();
            let status_label = status_label.clone();
            let state = Rc::clone(&state);
            let _nav = nav.clone();

            open_button.connect_clicked(move |_| {
                let path = path_row.text().to_string();
                let password = password_row.text().to_string();

                if path.is_empty() || password.is_empty() {
                    status_label.set_text("Path and password are required.");
                    return;
                }

                status_label.set_text("Opening vault...");
                let status = status_label.clone();
                let st = state.borrow();

                app::spawn_async(
                    &st,
                    move |service| async move {
                        service
                            .open_vault(OpenVaultParams {
                                password,
                                provider_type: "local".to_string(),
                                provider_config: serde_json::json!({ "root": path }),
                            })
                            .await
                    },
                    move |result| match result {
                        Ok(_) => status.set_text("Vault opened."),
                        Err(e) => status.set_text(&format!("Error: {}", e)),
                    },
                );
            });
        }

        // Create vault action
        {
            let path_row = path_row.clone();
            let password_row = password_row.clone();
            let status_label = status_label.clone();
            let state = Rc::clone(&state);

            create_button.connect_clicked(move |_| {
                let path = path_row.text().to_string();
                let password = password_row.text().to_string();

                if path.is_empty() || password.is_empty() {
                    status_label.set_text("Path and password are required.");
                    return;
                }

                let vault_name = std::path::Path::new(&path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("vault")
                    .to_string();

                status_label.set_text("Creating vault...");
                let status = status_label.clone();
                let st = state.borrow();

                app::spawn_async(
                    &st,
                    move |service| async move {
                        service
                            .create_vault(axiomvault_app::CreateVaultParams {
                                vault_id: vault_name,
                                password,
                                provider_type: "local".to_string(),
                                provider_config: serde_json::json!({ "root": path }),
                            })
                            .await
                    },
                    move |result| match result {
                        Ok(created) => {
                            status.set_text(&format!(
                                "Vault created. Recovery words:\n{}",
                                created.recovery_words
                            ));
                        }
                        Err(e) => status.set_text(&format!("Error: {}", e)),
                    },
                );
            });
        }

        Self { page }
    }

    pub fn page(&self) -> &adw::NavigationPage {
        &self.page
    }
}
