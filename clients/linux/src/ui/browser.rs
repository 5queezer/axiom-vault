//! Vault browser view — displays directory contents and supports file operations.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use std::sync::Arc;

use axiomvault_app::{AppService, DirectoryEntryDto};

use crate::app::AppState;

/// Column indices for the file list model.
mod col {
    pub const NAME: u32 = 0;
    pub const IS_DIR: u32 = 1;
    pub const SIZE: u32 = 2;
    pub const PATH: u32 = 3;
}

/// The vault browser view. Shows directory listings and supports navigation.
pub struct BrowserView {
    page: adw::NavigationPage,
}

impl BrowserView {
    pub fn new(state: Rc<RefCell<AppState>>, nav: adw::NavigationView) -> Self {
        let list_box = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .margin_start(12)
            .margin_end(12)
            .margin_top(12)
            .margin_bottom(12)
            .build();

        let scrolled = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .child(&list_box)
            .build();

        let path_label = gtk::Label::builder()
            .label("/")
            .css_classes(["heading"])
            .halign(gtk::Align::Start)
            .margin_start(16)
            .margin_top(8)
            .margin_bottom(4)
            .build();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&path_label);
        content.append(&scrolled);

        let toolbar_view = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();

        // Add file button
        let add_button = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add file")
            .build();
        header.pack_end(&add_button);

        // New folder button
        let mkdir_button = gtk::Button::builder()
            .icon_name("folder-new-symbolic")
            .tooltip_text("New folder")
            .build();
        header.pack_end(&mkdir_button);

        toolbar_view.add_top_bar(&header);
        toolbar_view.set_content(Some(&content));

        let page = adw::NavigationPage::builder()
            .title("Vault Browser")
            .child(&toolbar_view)
            .build();

        // Load initial directory listing
        {
            let list_box = list_box.clone();
            let state = Rc::clone(&state);
            let path_label = path_label.clone();

            glib::MainContext::default().spawn_local(async move {
                load_directory(&state, &list_box, &path_label, "/").await;
            });
        }

        Self { page }
    }

    pub fn page(&self) -> &adw::NavigationPage {
        &self.page
    }
}

/// Load a directory listing and populate the list box.
async fn load_directory(
    state: &Rc<RefCell<AppState>>,
    list_box: &gtk::ListBox,
    path_label: &gtk::Label,
    path: &str,
) {
    path_label.set_text(path);

    // Remove existing rows.
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    let path_owned = path.to_string();
    let st = state.borrow();
    let service = Arc::clone(&st.service);
    let rt = Arc::clone(&st.runtime);

    let result = rt
        .spawn(async move { service.list_directory(&path_owned).await })
        .await;

    match result {
        Ok(Ok(entries)) => {
            for entry in entries {
                let row = entry_row(&entry);
                list_box.append(&row);
            }
            if list_box.first_child().is_none() {
                let empty = adw::ActionRow::builder()
                    .title("Empty directory")
                    .css_classes(["dim-label"])
                    .build();
                list_box.append(&empty);
            }
        }
        Ok(Err(e)) => {
            let error_row = adw::ActionRow::builder()
                .title(&format!("Error: {}", e))
                .css_classes(["error"])
                .build();
            list_box.append(&error_row);
        }
        Err(e) => {
            tracing::error!("Task join error: {}", e);
        }
    }
}

/// Create a list row for a directory entry.
fn entry_row(entry: &DirectoryEntryDto) -> adw::ActionRow {
    let icon = if entry.is_directory {
        "folder-symbolic"
    } else {
        "document-symbolic"
    };

    let subtitle = if entry.is_directory {
        "Folder".to_string()
    } else {
        entry
            .size
            .map(|s| format_size(s))
            .unwrap_or_default()
    };

    adw::ActionRow::builder()
        .title(&entry.name)
        .subtitle(&subtitle)
        .activatable(entry.is_directory)
        .build()
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
