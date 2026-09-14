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

use std::ffi::{c_void, CStr, CString};
use std::os::raw::{c_char, c_int};
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::library::{LibraryEntry, LibraryHandlers, LibraryOptions};
use crate::sys::interop::{interop_show_library, LibraryEntryMetadata, LibraryMetadata};
use crate::sys::util::convert_to_cstring_or_null;

// Must be kept in sync with the constants declared in interop.h
const LIBRARY_ACTION_SAVE: c_int = 1;
const LIBRARY_ACTION_DELETE: c_int = 2;
const LIBRARY_ACTION_SET_STARTUP: c_int = 3;

struct OwnedEntry {
    id: CString,
    collection: CString,
    trigger: CString,
    replacement: CString,
    label: CString,
    editable: c_int,
}

impl OwnedEntry {
    fn as_metadata(&self) -> LibraryEntryMetadata {
        LibraryEntryMetadata {
            id: self.id.as_ptr(),
            collection: self.collection.as_ptr(),
            trigger: self.trigger.as_ptr(),
            replacement: self.replacement.as_ptr(),
            label: self.label.as_ptr(),
            editable: self.editable,
        }
    }
}

impl From<&LibraryEntry> for OwnedEntry {
    fn from(entry: &LibraryEntry) -> Self {
        Self {
            id: to_cstring(&entry.id),
            collection: to_cstring(&entry.collection),
            trigger: to_cstring(&entry.trigger),
            replacement: to_cstring(&entry.replacement),
            label: to_cstring(&entry.label),
            editable: c_int::from(entry.editable),
        }
    }
}

// Interior NUL bytes cannot cross the FFI boundary, so they are dropped
// instead of aborting the whole UI.
fn to_cstring(value: &str) -> CString {
    CString::new(value.replace('\0', "")).expect("string without NUL bytes is a valid CString")
}

fn read_cstr(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        None
    } else {
        Some(
            unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}

// Copies `message` into the buffer provided by the native window, truncating
// on a char boundary and always NUL-terminating.
fn write_result(result: *mut c_char, result_size: c_int, message: &str) {
    let Ok(size) = usize::try_from(result_size) else {
        return;
    };
    if result.is_null() || size == 0 {
        return;
    }

    let mut end = message.len().min(size - 1);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    let bytes = &message.as_bytes()[..end];

    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), result.cast::<u8>(), bytes.len());
        *result.add(bytes.len()) = 0;
    }
}

#[allow(clippy::too_many_arguments)]
extern "C" fn action_callback(
    action: c_int,
    id: *const c_char,
    trigger: *const c_char,
    replacement: *const c_char,
    label: *const c_char,
    startup_enabled: c_int,
    data: *mut c_void,
    result: *mut c_char,
    result_size: c_int,
) -> c_int {
    let handlers = unsafe { &*data.cast::<LibraryHandlers>() };
    let id = read_cstr(id);
    let trigger = read_cstr(trigger).unwrap_or_default();
    let replacement = read_cstr(replacement).unwrap_or_default();
    let label = read_cstr(label).unwrap_or_default();

    let outcome = catch_unwind(AssertUnwindSafe(|| match action {
        LIBRARY_ACTION_SAVE => (handlers.save)(id.as_deref(), &trigger, &replacement, &label),
        LIBRARY_ACTION_DELETE => id
            .as_deref()
            .ok_or_else(|| "missing snippet id".to_string())
            .and_then(|id| (handlers.delete)(id))
            .map(|()| String::new()),
        LIBRARY_ACTION_SET_STARTUP => handlers.set_startup.as_ref().map_or_else(
            || Err("startup control is not supported on this platform".to_string()),
            |handler| handler(startup_enabled != 0).map(|()| String::new()),
        ),
        other => Err(format!("unknown library action: {other}")),
    }));

    let (code, message) = match outcome {
        Ok(Ok(message)) => (0, message),
        Ok(Err(message)) => (1, message),
        Err(_) => (
            1,
            "an internal error occurred while processing the request".to_string(),
        ),
    };

    write_result(result, result_size, &message);
    code
}

pub fn show(options: LibraryOptions) {
    let owned_entries: Vec<OwnedEntry> = options.entries.iter().map(Into::into).collect();
    let entries: Vec<LibraryEntryMetadata> =
        owned_entries.iter().map(OwnedEntry::as_metadata).collect();

    let (_c_window_icon_path, c_window_icon_path_ptr) =
        convert_to_cstring_or_null(options.window_icon_path);
    let c_config_dir = to_cstring(&options.config_dir);
    let c_new_entry_collection = to_cstring(&options.new_entry_collection);

    let mut handlers = options.handlers;

    let metadata = LibraryMetadata {
        window_icon_path: c_window_icon_path_ptr,
        entries: entries.as_ptr(),
        entries_count: entries.len() as c_int,
        config_dir: c_config_dir.as_ptr(),
        new_entry_collection: c_new_entry_collection.as_ptr(),
        startup_enabled: c_int::from(options.startup_enabled),
        startup_supported: c_int::from(handlers.set_startup.is_some()),
    };

    unsafe {
        interop_show_library(
            &metadata,
            action_callback,
            std::ptr::from_mut::<LibraryHandlers>(&mut handlers).cast::<c_void>(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::write_result;
    use std::ffi::CStr;
    use std::os::raw::c_char;

    fn read(buffer: &[c_char]) -> String {
        unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn write_result_copies_short_messages() {
        let mut buffer = [1 as c_char; 16];
        write_result(buffer.as_mut_ptr(), buffer.len() as i32, "saved");
        assert_eq!(read(&buffer), "saved");
    }

    #[test]
    fn write_result_truncates_on_char_boundaries() {
        let mut buffer = [1 as c_char; 6];
        write_result(buffer.as_mut_ptr(), buffer.len() as i32, "ab\u{e9}cd");
        assert_eq!(read(&buffer), "ab\u{e9}c");
    }

    #[test]
    fn write_result_ignores_invalid_buffers() {
        write_result(std::ptr::null_mut(), 16, "ignored");
        let mut buffer = [1 as c_char; 1];
        write_result(buffer.as_mut_ptr(), 0, "ignored");
        write_result(buffer.as_mut_ptr(), -1, "ignored");
        assert_eq!(buffer[0], 1);
    }
}
