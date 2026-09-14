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

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::SystemTime,
};

use anyhow::{anyhow, bail, Context, Result};
use log::{info, warn};
use serde_norway::{Mapping, Value};

use super::{CliModule, CliModuleArgs, LogMode};
use crate::path::Paths;

// Files whose first line is exactly this header are owned by the snippet
// library and can be edited from the UI. Every other match file is shown
// read-only and never rewritten, so hand-written YAML is preserved as-is.
pub const MANAGED_HEADER: &str = "# espanso-library: managed by the Espanso snippet library";

// Managed files live in this directory, relative to the match directory.
const MANAGED_DIR: &str = "library";
// New snippets are appended to this managed file.
const DEFAULT_MANAGED_FILE: &str = "snippets.yml";

pub fn new() -> CliModule {
    #[allow(clippy::needless_update)]
    CliModule {
        requires_paths: true,
        enable_logs: true,
        disable_logs_terminal_output: true,
        log_mode: LogMode::AppendOnly,
        subcommand: "library".to_string(),
        show_in_dock: true,
        entry: library_main,
        ..Default::default()
    }
}

// Opens the snippet library in a dedicated process, so that the caller
// (launcher or worker) is never blocked by the native UI event loop.
pub fn open_library(paths: &Paths) -> Result<()> {
    let espanso_exe_path =
        std::env::current_exe().context("unable to determine the espanso executable path")?;
    let mut command = Command::new(espanso_exe_path);
    command
        .arg("library")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Propagate the resolved directories so the child uses the same config
    // even when the parent was started with explicit path overrides.
    for (variable, dir) in [
        ("ESPANSO_CONFIG_DIR", &paths.config),
        ("ESPANSO_PACKAGE_DIR", &paths.packages),
        ("ESPANSO_RUNTIME_DIR", &paths.runtime),
    ] {
        if dir.is_dir() {
            command.env(variable, dir);
        }
    }

    crate::util::set_command_flags(&mut command);

    let mut child = command
        .spawn()
        .context("unable to spawn the espanso library process")?;
    info!("opened the snippet library in process {}", child.id());

    // Reap the child in the background so it doesn't linger as a zombie.
    std::thread::Builder::new()
        .name("espanso-library-reaper".to_string())
        .spawn(move || {
            let _ = child.wait();
        })
        .context("unable to spawn the library reaper thread")?;

    Ok(())
}

#[cfg(feature = "modulo")]
fn library_main(args: CliModuleArgs) -> i32 {
    use std::{cell::RefCell, rc::Rc};

    use espanso_modulo::library::{LibraryEntry, LibraryHandlers, LibraryOptions};

    let paths = args.paths.expect("missing paths in library main");
    let icon_paths =
        crate::icon::load_icon_paths(&paths.runtime).expect("unable to load icon paths");

    let store = match LibraryStore::load(&paths.config) {
        Ok(store) => store,
        Err(error) => {
            crate::error_eprintln!("unable to load the snippet library: {error:?}");
            return 1;
        }
    };

    let entries = store
        .snippets()
        .into_iter()
        .map(|snippet| LibraryEntry {
            id: snippet.id,
            collection: snippet.collection,
            trigger: snippet.trigger,
            replacement: snippet.replacement,
            label: snippet.label,
            editable: snippet.editable,
        })
        .collect();
    let new_entry_collection = store.default_collection();

    let store = Rc::new(RefCell::new(store));
    let save_store = Rc::clone(&store);
    let delete_store = Rc::clone(&store);

    espanso_modulo::library::show(LibraryOptions {
        window_icon_path: icon_paths
            .wizard_icon
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        config_dir: paths.config.to_string_lossy().to_string(),
        new_entry_collection,
        entries,
        startup_enabled: startup::is_enabled(),
        handlers: LibraryHandlers {
            save: Box::new(move |id, trigger, replacement, label| {
                save_store
                    .borrow_mut()
                    .save(id, trigger, replacement, label)
                    .map_err(user_message)
            }),
            delete: Box::new(move |id| delete_store.borrow_mut().delete(id).map_err(user_message)),
            set_startup: startup::handler(),
        },
    });

    0
}

#[cfg(not(feature = "modulo"))]
fn library_main(_: CliModuleArgs) -> i32 {
    crate::error_eprintln!("this version of espanso was not compiled with 'modulo' support, the snippet library is not available");
    1
}

#[cfg(feature = "modulo")]
fn user_message(error: anyhow::Error) -> String {
    format!("{error:#}")
}

#[cfg(feature = "modulo")]
mod startup {
    use espanso_modulo::library::StartupHandler;

    #[cfg(target_os = "macos")]
    pub fn is_enabled() -> bool {
        crate::cli::service::is_registered()
    }

    #[cfg(target_os = "macos")]
    pub fn handler() -> Option<StartupHandler> {
        Some(Box::new(|enabled| {
            crate::cli::service::set_library_startup(enabled).map_err(|error| format!("{error:#}"))
        }))
    }

    #[cfg(not(target_os = "macos"))]
    pub fn is_enabled() -> bool {
        false
    }

    #[cfg(not(target_os = "macos"))]
    pub fn handler() -> Option<StartupHandler> {
        None
    }
}

// A snippet as presented to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippet {
    pub id: String,
    pub collection: String,
    pub trigger: String,
    pub replacement: String,
    pub label: String,
    pub editable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fingerprint {
    modified: Option<SystemTime>,
    len: u64,
}

#[derive(Debug)]
struct LoadedFile {
    path: PathBuf,
    collection: String,
    managed: bool,
    // `None` when the file does not exist yet.
    fingerprint: Option<Fingerprint>,
    document: Mapping,
}

#[derive(Debug)]
struct Entry {
    key: u64,
    file: usize,
    // Position inside the `matches` sequence of the file.
    index: usize,
    trigger: String,
    replacement: String,
    label: String,
    editable: bool,
}

// In-memory view of every loaded match file. Managed files are rewritten from
// their parsed document on save, other files are only read.
pub struct LibraryStore {
    match_dir: PathBuf,
    files: Vec<LoadedFile>,
    entries: Vec<Entry>,
    next_key: u64,
}

impl LibraryStore {
    pub fn load(config_dir: &Path) -> Result<Self> {
        let (_config_store, match_store, _non_fatal_errors) =
            espanso_config::load(config_dir).context("unable to load the espanso configuration")?;

        let mut paths = match_store.loaded_paths();
        paths.sort();

        let match_dir = config_dir.join("match");
        let match_dir = std::fs::canonicalize(&match_dir).unwrap_or(match_dir);

        let mut store = Self {
            match_dir,
            files: Vec::new(),
            entries: Vec::new(),
            next_key: 1,
        };

        for path in paths {
            if let Err(error) = store.load_file(Path::new(&path)) {
                warn!("skipping match file {path} in the snippet library: {error:#}");
            }
        }

        Ok(store)
    }

    pub fn snippets(&self) -> Vec<Snippet> {
        self.entries
            .iter()
            .map(|entry| Snippet {
                id: entry.key.to_string(),
                collection: self.files[entry.file].collection.clone(),
                trigger: entry.trigger.clone(),
                replacement: entry.replacement.clone(),
                label: entry.label.clone(),
                editable: entry.editable,
            })
            .collect()
    }

    pub fn default_collection(&self) -> String {
        self.collection_name(&self.default_file_path())
    }

    pub fn save(
        &mut self,
        id: Option<&str>,
        trigger: &str,
        replacement: &str,
        label: &str,
    ) -> Result<String> {
        if trigger.trim().is_empty() {
            bail!("the trigger cannot be empty");
        }
        if replacement.is_empty() {
            bail!("the replacement cannot be empty");
        }

        match id {
            Some(id) => self.update(id, trigger, replacement, label),
            None => self.create(trigger, replacement, label),
        }
    }

    pub fn delete(&mut self, id: &str) -> Result<()> {
        let position = self.find_entry(id)?;
        let file_index = self.entries[position].file;
        let index = self.entries[position].index;
        if !self.entries[position].editable {
            bail!("{}", read_only_message());
        }

        let file = &mut self.files[file_index];
        file.ensure_unchanged()?;
        let matches = file.matches_mut();
        if index >= matches.len() {
            bail!("snippet not found: {id}");
        }
        matches.remove(index);
        file.write()?;

        self.entries.remove(position);
        for entry in &mut self.entries {
            if entry.file == file_index && entry.index > index {
                entry.index -= 1;
            }
        }

        Ok(())
    }

    fn update(
        &mut self,
        id: &str,
        trigger: &str,
        replacement: &str,
        label: &str,
    ) -> Result<String> {
        let position = self.find_entry(id)?;
        let file_index = self.entries[position].file;
        let index = self.entries[position].index;
        if !self.entries[position].editable {
            bail!("{}", read_only_message());
        }

        let file = &mut self.files[file_index];
        file.ensure_unchanged()?;
        let item = file.item_mut(index)?;
        apply_fields(item, trigger, replacement, label);
        file.write()?;

        let entry = &mut self.entries[position];
        entry.trigger = trigger.to_string();
        entry.replacement = replacement.to_string();
        entry.label = label.trim().to_string();

        Ok(id.to_string())
    }

    fn create(&mut self, trigger: &str, replacement: &str, label: &str) -> Result<String> {
        let file_index = self.default_file_index()?;
        let file = &mut self.files[file_index];
        file.ensure_unchanged()?;

        let mut item = Mapping::new();
        apply_fields(&mut item, trigger, replacement, label);
        let matches = file.matches_mut();
        matches.push(Value::Mapping(item));
        let index = matches.len() - 1;
        file.write()?;

        let key = self.next_key;
        self.next_key += 1;
        self.entries.push(Entry {
            key,
            file: file_index,
            index,
            trigger: trigger.to_string(),
            replacement: replacement.to_string(),
            label: label.trim().to_string(),
            editable: true,
        });

        Ok(key.to_string())
    }

    fn load_file(&mut self, path: &Path) -> Result<()> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("unable to read {}", path.display()))?;
        let managed = is_managed(&content);
        let document: Value = serde_norway::from_str(&content)
            .with_context(|| format!("unable to parse {}", path.display()))?;
        let document = match document {
            Value::Mapping(mapping) => mapping,
            Value::Null => Mapping::new(),
            _ => bail!("{} does not contain a YAML mapping", path.display()),
        };

        let file_index = self.files.len();
        let matches = document
            .get("matches")
            .and_then(Value::as_sequence)
            .cloned()
            .unwrap_or_default();

        for (index, item) in matches.iter().enumerate() {
            let Some(item) = item.as_mapping() else {
                continue;
            };
            let Some(view) = snippet_view(item) else {
                continue;
            };
            self.entries.push(Entry {
                key: self.next_key,
                file: file_index,
                index,
                trigger: view.trigger,
                replacement: view.replacement,
                label: view.label,
                editable: managed && view.editable,
            });
            self.next_key += 1;
        }

        self.files.push(LoadedFile {
            path: path.to_path_buf(),
            collection: self.collection_name(path),
            managed,
            fingerprint: fingerprint(path)?,
            document,
        });

        Ok(())
    }

    fn find_entry(&self, id: &str) -> Result<usize> {
        let key: u64 = id
            .parse()
            .map_err(|_| anyhow!("invalid snippet id: {id}"))?;
        self.entries
            .iter()
            .position(|entry| entry.key == key)
            .ok_or_else(|| anyhow!("snippet not found: {id}"))
    }

    fn default_file_path(&self) -> PathBuf {
        self.match_dir.join(MANAGED_DIR).join(DEFAULT_MANAGED_FILE)
    }

    fn default_file_index(&mut self) -> Result<usize> {
        let path = self.default_file_path();
        if let Some(index) = self.files.iter().position(|file| file.path == path) {
            if !self.files[index].managed {
                bail!(
                    "{} exists but is not managed by the library. Add the header line \"{MANAGED_HEADER}\" or move the file elsewhere",
                    path.display()
                );
            }
            return Ok(index);
        }

        if path.exists() {
            bail!(
                "{} exists but could not be loaded by espanso. Fix the file (see 'espanso log') and reopen the library",
                path.display()
            );
        }

        let collection = self.collection_name(&path);
        self.files.push(LoadedFile {
            path,
            collection,
            managed: true,
            fingerprint: None,
            document: Mapping::new(),
        });

        Ok(self.files.len() - 1)
    }

    fn collection_name(&self, path: &Path) -> String {
        let canonical = std::fs::canonicalize(path).ok();
        let relative = path.strip_prefix(&self.match_dir).ok().or_else(|| {
            canonical
                .as_deref()
                .and_then(|canonical| canonical.strip_prefix(&self.match_dir).ok())
        });

        relative.map_or_else(
            || path.display().to_string(),
            |relative| relative.to_string_lossy().replace('\\', "/"),
        )
    }
}

impl LoadedFile {
    fn ensure_unchanged(&self) -> Result<()> {
        let current = fingerprint(&self.path)?;
        if current != self.fingerprint {
            bail!(
                "{} was modified outside the library. Close and reopen the library to continue",
                self.path.display()
            );
        }
        Ok(())
    }

    fn matches_mut(&mut self) -> &mut Vec<Value> {
        let has_sequence = self.document.get("matches").is_some_and(Value::is_sequence);
        if !has_sequence {
            self.document.insert(
                Value::String("matches".to_string()),
                Value::Sequence(Vec::new()),
            );
        }

        self.document
            .get_mut("matches")
            .and_then(Value::as_sequence_mut)
            .expect("matches was just ensured to be a sequence")
    }

    fn item_mut(&mut self, index: usize) -> Result<&mut Mapping> {
        let path = self.path.clone();
        self.matches_mut()
            .get_mut(index)
            .and_then(Value::as_mapping_mut)
            .ok_or_else(|| anyhow!("snippet not found in {}", path.display()))
    }

    // Writes the managed document atomically: the content goes to a hidden
    // sibling file first and is then renamed over the destination.
    fn write(&mut self) -> Result<()> {
        let directory = self
            .path
            .parent()
            .ok_or_else(|| anyhow!("invalid match file path {}", self.path.display()))?;
        std::fs::create_dir_all(directory)
            .with_context(|| format!("unable to create {}", directory.display()))?;

        let body = serde_norway::to_string(&self.document)
            .context("unable to serialize the snippet file")?;
        let content = format!("{MANAGED_HEADER}\n\n{body}");

        let file_name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let pending_file = directory.join(format!(".{file_name}.espanso-library.tmp"));
        {
            let mut file = std::fs::File::create(&pending_file)
                .with_context(|| format!("unable to create {}", pending_file.display()))?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
        }
        std::fs::rename(&pending_file, &self.path)
            .with_context(|| format!("unable to replace {}", self.path.display()))?;

        self.fingerprint = fingerprint(&self.path)?;
        Ok(())
    }
}

struct SnippetView {
    trigger: String,
    replacement: String,
    label: String,
    // Only matches with a single plain trigger and a plain replacement can be
    // edited without losing information.
    editable: bool,
}

fn snippet_view(item: &Mapping) -> Option<SnippetView> {
    let label = string_field(item, "label").unwrap_or_default();

    let (trigger, simple_trigger) = if let Some(trigger) = string_field(item, "trigger") {
        (trigger, true)
    } else if let Some(triggers) = item.get("triggers").and_then(Value::as_sequence) {
        let triggers: Vec<&str> = triggers.iter().filter_map(Value::as_str).collect();
        if triggers.is_empty() {
            return None;
        }
        (triggers.join(", "), triggers.len() == 1)
    } else {
        (format!("regex: {}", string_field(item, "regex")?), false)
    };

    let (replacement, simple_replacement) = if let Some(replace) = string_field(item, "replace") {
        (replace, true)
    } else if let Some(markdown) = string_field(item, "markdown") {
        (markdown, false)
    } else if let Some(html) = string_field(item, "html") {
        (html, false)
    } else if let Some(form) = string_field(item, "form") {
        (form, false)
    } else if let Some(image) = string_field(item, "image_path") {
        (format!("[image] {image}"), false)
    } else {
        (String::new(), false)
    };

    Some(SnippetView {
        trigger,
        replacement,
        label,
        editable: simple_trigger && simple_replacement,
    })
}

fn string_field(item: &Mapping, key: &str) -> Option<String> {
    item.get(key).and_then(Value::as_str).map(str::to_string)
}

fn apply_fields(item: &mut Mapping, trigger: &str, replacement: &str, label: &str) {
    item.remove("triggers");
    item.insert(
        Value::String("trigger".to_string()),
        Value::String(trigger.to_string()),
    );
    item.insert(
        Value::String("replace".to_string()),
        Value::String(replacement.to_string()),
    );

    let label = label.trim();
    if label.is_empty() {
        item.remove("label");
    } else {
        item.insert(
            Value::String("label".to_string()),
            Value::String(label.to_string()),
        );
    }
}

fn is_managed(content: &str) -> bool {
    content
        .lines()
        .next()
        .is_some_and(|line| line.trim_end() == MANAGED_HEADER)
}

fn read_only_message() -> String {
    format!("this snippet is read-only: only files managed by the library (under match/{MANAGED_DIR}) can be edited here")
}

fn fingerprint(path: &Path) -> Result<Option<Fingerprint>> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(Some(Fingerprint {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("unable to read metadata of {}", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_managed, LibraryStore, Snippet, MANAGED_HEADER};
    use std::path::{Path, PathBuf};
    use tempdir::TempDir;

    const BASE_FILE: &str = "matches:\n  - trigger: \":hello\"\n    replace: \"world\"\n";

    fn managed_file() -> String {
        format!(
            "{MANAGED_HEADER}\n\nmatches:\n  - trigger: \":sig\"\n    replace: \"Best regards\"\n    label: \"Signature\"\n  - triggers: [\":addr\"]\n    replace: \"Main St\"\n"
        )
    }

    fn setup(with_managed: bool) -> (TempDir, PathBuf) {
        let dir = TempDir::new("espanso-library").unwrap();
        let base = dir.path().join("espanso");
        std::fs::create_dir_all(base.join("config")).unwrap();
        std::fs::write(base.join("config").join("default.yml"), "").unwrap();
        std::fs::create_dir_all(base.join("match")).unwrap();
        std::fs::write(base.join("match").join("base.yml"), BASE_FILE).unwrap();
        if with_managed {
            std::fs::create_dir_all(base.join("match").join("library")).unwrap();
            std::fs::write(managed_path(&base), managed_file()).unwrap();
        }
        (dir, base)
    }

    fn managed_path(base: &Path) -> PathBuf {
        base.join("match").join("library").join("snippets.yml")
    }

    fn find<'a>(snippets: &'a [Snippet], trigger: &str) -> &'a Snippet {
        snippets
            .iter()
            .find(|snippet| snippet.trigger == trigger)
            .unwrap_or_else(|| panic!("missing snippet {trigger}"))
    }

    #[test]
    fn detects_managed_header() {
        assert!(is_managed(&managed_file()));
        assert!(is_managed(&format!("{MANAGED_HEADER}   \nmatches: []")));
        assert!(!is_managed(BASE_FILE));
        assert!(!is_managed(&format!("\n{MANAGED_HEADER}\n")));
    }

    #[test]
    fn loads_entries_with_editability() {
        let (_dir, base) = setup(true);
        let store = LibraryStore::load(&base).unwrap();
        let snippets = store.snippets();
        assert_eq!(snippets.len(), 3);

        let hello = find(&snippets, ":hello");
        assert!(!hello.editable);
        assert_eq!(hello.collection, "base.yml");
        assert_eq!(hello.replacement, "world");

        let sig = find(&snippets, ":sig");
        assert!(sig.editable);
        assert_eq!(sig.collection, "library/snippets.yml");
        assert_eq!(sig.label, "Signature");

        // A single-element `triggers` list is still editable.
        assert!(find(&snippets, ":addr").editable);
        assert_eq!(store.default_collection(), "library/snippets.yml");
    }

    #[test]
    fn saves_new_snippet_into_managed_file() {
        let (_dir, base) = setup(true);
        let mut store = LibraryStore::load(&base).unwrap();
        let id = store
            .save(None, ":new", "line one\nline two", "  New  ")
            .unwrap();
        assert_eq!(find(&store.snippets(), ":new").id, id);

        let content = std::fs::read_to_string(managed_path(&base)).unwrap();
        assert!(content.starts_with(MANAGED_HEADER));

        let reloaded = LibraryStore::load(&base).unwrap();
        let snippets = reloaded.snippets();
        assert_eq!(snippets.len(), 4);
        let new = find(&snippets, ":new");
        assert!(new.editable);
        assert_eq!(new.replacement, "line one\nline two");
        assert_eq!(new.label, "New");
        // Existing entries are untouched.
        assert_eq!(find(&snippets, ":sig").replacement, "Best regards");
    }

    #[test]
    fn creates_default_file_when_missing() {
        let (_dir, base) = setup(false);
        let mut store = LibraryStore::load(&base).unwrap();
        assert_eq!(store.snippets().len(), 1);

        store.save(None, ":fresh", "text", "").unwrap();
        assert!(managed_path(&base).is_file());

        let reloaded = LibraryStore::load(&base).unwrap();
        let snippets = reloaded.snippets();
        let fresh = find(&snippets, ":fresh");
        assert!(fresh.editable);
        assert_eq!(fresh.label, "");
    }

    #[test]
    fn updates_existing_snippet() {
        let (_dir, base) = setup(true);
        let mut store = LibraryStore::load(&base).unwrap();
        let id = find(&store.snippets(), ":addr").id.clone();

        let saved = store
            .save(Some(&id), ":address", "1 Main St", "Address")
            .unwrap();
        assert_eq!(saved, id);

        let reloaded = LibraryStore::load(&base).unwrap();
        let snippets = reloaded.snippets();
        assert_eq!(snippets.len(), 3);
        let address = find(&snippets, ":address");
        assert_eq!(address.replacement, "1 Main St");
        assert_eq!(address.label, "Address");
        assert_eq!(find(&snippets, ":sig").label, "Signature");

        let content = std::fs::read_to_string(managed_path(&base)).unwrap();
        assert!(!content.contains("triggers"));
    }

    #[test]
    fn deletes_and_reindexes_following_entries() {
        let (_dir, base) = setup(true);
        let mut store = LibraryStore::load(&base).unwrap();
        let sig = find(&store.snippets(), ":sig").id.clone();
        let addr = find(&store.snippets(), ":addr").id.clone();

        store.delete(&sig).unwrap();
        assert_eq!(store.snippets().len(), 2);

        // The remaining managed entry shifted down and must still be reachable.
        store.save(Some(&addr), ":addr", "Updated", "").unwrap();

        let reloaded = LibraryStore::load(&base).unwrap();
        let snippets = reloaded.snippets();
        assert_eq!(snippets.len(), 2);
        assert_eq!(find(&snippets, ":addr").replacement, "Updated");
        assert!(snippets.iter().all(|snippet| snippet.trigger != ":sig"));
    }

    #[test]
    fn rejects_read_only_edits() {
        let (_dir, base) = setup(true);
        let mut store = LibraryStore::load(&base).unwrap();
        let hello = find(&store.snippets(), ":hello").id.clone();

        assert!(store.save(Some(&hello), ":hello", "changed", "").is_err());
        assert!(store.delete(&hello).is_err());
        assert_eq!(
            std::fs::read_to_string(base.join("match").join("base.yml")).unwrap(),
            BASE_FILE
        );
    }

    #[test]
    fn rejects_empty_fields() {
        let (_dir, base) = setup(true);
        let mut store = LibraryStore::load(&base).unwrap();
        assert!(store.save(None, "   ", "text", "").is_err());
        assert!(store.save(None, ":trigger", "", "").is_err());
        assert!(store.save(Some("not-a-number"), ":t", "x", "").is_err());
        assert!(store.delete("999").is_err());
    }

    #[test]
    fn detects_external_modifications() {
        let (_dir, base) = setup(true);
        let mut store = LibraryStore::load(&base).unwrap();
        let sig = find(&store.snippets(), ":sig").id.clone();

        let external = format!(
            "{}  - trigger: \":external\"\n    replace: \"x\"\n",
            managed_file()
        );
        std::fs::write(managed_path(&base), &external).unwrap();

        let error = store.save(Some(&sig), ":sig", "changed", "").unwrap_err();
        assert!(error.to_string().contains("modified outside"));
        assert_eq!(
            std::fs::read_to_string(managed_path(&base)).unwrap(),
            external
        );
    }

    #[test]
    fn unmanaged_file_in_library_dir_is_read_only() {
        let (_dir, base) = setup(false);
        std::fs::create_dir_all(base.join("match").join("library")).unwrap();
        std::fs::write(managed_path(&base), BASE_FILE).unwrap();

        let mut store = LibraryStore::load(&base).unwrap();
        let snippets = store.snippets();
        let hello = find(&snippets, ":hello");
        assert!(!hello.editable);
        // The default file exists without the header, so new snippets cannot go there.
        assert!(store.save(None, ":new", "text", "").is_err());
    }
}
