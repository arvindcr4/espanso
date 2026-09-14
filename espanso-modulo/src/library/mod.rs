/*
 * This file is part of espanso.
 *
 * Copyright (C) 2019-2021 Federico Terzi
 *
 * espanso is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * espanso is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with espanso.  If not, see <https://www.gnu.org/licenses/>.
 */

pub use crate::sys::library::show;

// A snippet shown by the native library window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryEntry {
    // Opaque identifier handed back to the handlers on save and delete.
    pub id: String,
    // Human readable name of the file the snippet is stored in.
    pub collection: String,
    pub trigger: String,
    pub replacement: String,
    pub label: String,
    // Read-only entries can be browsed and searched but not modified.
    pub editable: bool,
}

// Invoked when the user saves a snippet. The id is `None` for new snippets.
// Returns the id of the saved snippet, or a user-facing error message.
pub type SaveHandler = Box<dyn Fn(Option<&str>, &str, &str, &str) -> Result<String, String>>;
// Invoked when the user deletes the snippet with the given id.
pub type DeleteHandler = Box<dyn Fn(&str) -> Result<(), String>>;
// Invoked when the user toggles the "start at login" control.
pub type StartupHandler = Box<dyn Fn(bool) -> Result<(), String>>;

pub struct LibraryHandlers {
    pub save: SaveHandler,
    pub delete: DeleteHandler,
    // When `None`, the startup control is hidden.
    pub set_startup: Option<StartupHandler>,
}

pub struct LibraryOptions {
    pub window_icon_path: Option<String>,
    // Displayed to the user and opened by the "Open config folder" button.
    pub config_dir: String,
    // Collection name shown for snippets that have not been saved yet.
    pub new_entry_collection: String,
    pub entries: Vec<LibraryEntry>,
    pub startup_enabled: bool,
    pub handlers: LibraryHandlers,
}
