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

#[cfg(target_os = "macos")]
use anyhow::bail;
#[cfg(target_os = "macos")]
use anyhow::Context;
use anyhow::Result;
#[cfg(target_os = "macos")]
use log::{info, warn};
#[cfg(target_os = "macos")]
use std::{
    fs::create_dir_all,
    path::PathBuf,
    process::{Command, ExitStatus, Output},
};
use thiserror::Error;

#[cfg(target_os = "macos")]
use crate::{cli::util::prevent_running_as_root_on_macos, error_eprintln};

const SERVICE_PLIST_CONTENT: &str = include_str!("../../res/macos/com.federicoterzi.espanso.plist");
#[cfg(target_os = "macos")]
const SERVICE_PLIST_FILE_NAME: &str = "com.federicoterzi.espanso.plist";

#[cfg(target_os = "macos")]
pub fn register() -> Result<()> {
    prevent_running_as_root_on_macos();

    ensure_registration_supported()?;
    let plist_file = write_service_plist()?;

    info!("reloading espanso launchctl entry");

    match Command::new("launchctl").arg("unload").arg(&plist_file).output() {
        Ok(output) if !output.status.success() => {
            warn!("launchctl unload failed: {}", launchctl_error(&output));
        }
        Ok(_) => {}
        Err(err) => warn!("launchctl unload command failed: {err}"),
    }

    let output = Command::new("launchctl")
        .arg("load")
        .arg(&plist_file)
        .output()
        .context("unable to execute launchctl load")?;

    if output.status.success() {
        Ok(())
    } else {
        Err(RegisterError::LaunchCtlLoadFailed {
            status: output.status,
            details: launchctl_error(&output),
        }
        .into())
    }
}

#[derive(Error, Debug)]
pub enum RegisterError {
    #[cfg(target_os = "macos")]
    #[error("the Espanso executable path is not valid UTF-8")]
    ExecutablePathNotUtf8,

    #[cfg(target_os = "macos")]
    #[error("the PATH environment variable is not valid UTF-8")]
    PathNotUtf8,

    #[error("{field} contains a character that cannot be stored in an XML plist")]
    InvalidXmlCharacter { field: &'static str },

    #[cfg(target_os = "macos")]
    #[error("launchctl load failed with status {status}: {details}")]
    LaunchCtlLoadFailed { status: ExitStatus, details: String },
}

#[cfg(target_os = "macos")]
pub fn unregister() -> Result<()> {
    prevent_running_as_root_on_macos();

    let plist_file = get_service_file_path()?;
    if plist_file.exists() {
        match Command::new("launchctl").arg("unload").arg(&plist_file).output() {
            Ok(output) if !output.status.success() => {
                warn!("launchctl unload failed: {}", launchctl_error(&output));
            }
            Ok(_) => {}
            Err(err) => warn!("launchctl unload command failed: {err}"),
        }

        std::fs::remove_file(&plist_file)
            .with_context(|| format!("unable to remove LaunchAgents entry {}", plist_file.display()))?;

        Ok(())
    } else {
        Err(UnregisterError::PlistNotFound.into())
    }
}

#[cfg(target_os = "macos")]
#[derive(Error, Debug)]
pub enum UnregisterError {
    #[error("plist entry not found")]
    PlistNotFound,
}

#[cfg(target_os = "macos")]
pub fn is_registered() -> bool {
    get_service_file_path().map_or(false, |path| path.is_file())
}

// The library changes the next-login setting only; it must not interrupt the
// currently loaded LaunchAgent or the daemon it owns.
#[cfg(target_os = "macos")]
pub fn set_library_startup(enabled: bool) -> Result<()> {
    prevent_running_as_root_on_macos();

    if enabled {
        ensure_registration_supported()?;
        write_service_plist()?;
    } else {
        let plist_file = get_service_file_path()?;
        if plist_file.exists() {
            std::fs::remove_file(&plist_file).with_context(|| {
                format!("unable to remove LaunchAgents entry {}", plist_file.display())
            })?;
        }
    }

    Ok(())
}

#[cfg(target_os = "macos")]
pub fn start_service() -> Result<()> {
    if !is_registered() {
        eprintln!("Unable to start espanso as a service as it's not been registered.");
        eprintln!("You can either register it first with `espanso service register` or");
        eprintln!("you can run it in unmanaged mode with `espanso service start --unmanaged`");
        eprintln!();
        eprintln!("NOTE: unmanaged mode means espanso does not rely on the system service manager");
        eprintln!("      to run, but as a result, you are in charge of starting/stopping espanso");
        eprintln!("      when needed.");
        return Err(StartError::NotRegistered.into());
    }

    let res = Command::new("launchctl")
        .args(["start", "com.federicoterzi.espanso"])
        .status();

    if let Ok(status) = res {
        if status.success() {
            Ok(())
        } else {
            Err(StartError::LaunchCtlNonZeroExit(status).into())
        }
    } else {
        Err(StartError::LaunchCtlFailure.into())
    }
}

#[cfg(target_os = "macos")]
#[derive(Error, Debug)]
pub enum StartError {
    #[error("not registered as a service")]
    NotRegistered,

    #[error("launchctl failed to run")]
    LaunchCtlFailure,

    #[error("launchctl exited with non-zero code `{0}`")]
    LaunchCtlNonZeroExit(ExitStatus),
}

#[cfg(target_os = "macos")]
fn get_service_file_path() -> Result<PathBuf> {
    let home_dir = dirs::home_dir().context("could not get the user home directory")?;
    Ok(home_dir
        .join("Library")
        .join("LaunchAgents")
        .join(SERVICE_PLIST_FILE_NAME))
}

#[cfg(target_os = "macos")]
fn ensure_registration_supported() -> Result<()> {
    if crate::cli::util::is_subject_to_app_translocation_on_macos() {
        error_eprintln!("Unable to register Espanso as service, please move the Espanso.app bundle inside the /Applications directory to proceed.");
        error_eprintln!(
            "For more information, please see: https://github.com/espanso/espanso/issues/844"
        );
        bail!("macOS activated app-translocation on Espanso");
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn write_service_plist() -> Result<PathBuf> {
    let plist_file = get_service_file_path()?;
    let agents_dir = plist_file
        .parent()
        .expect("LaunchAgent plist path must have a parent directory");
    create_dir_all(agents_dir)
        .with_context(|| format!("unable to create LaunchAgents directory {}", agents_dir.display()))?;

    let espanso_path = std::env::current_exe()
        .context("unable to determine the Espanso executable path for launchd")?;
    let espanso_path = espanso_path
        .to_str()
        .ok_or(RegisterError::ExecutablePathNotUtf8)?;
    let user_path = std::env::var_os("PATH").unwrap_or_default();
    let user_path = user_path.to_str().ok_or(RegisterError::PathNotUtf8)?;
    let plist_content = render_service_plist(espanso_path, user_path)?;

    info!("updating LaunchAgents entry: {}", plist_file.display());
    info!("entry will point to: {espanso_path}");
    std::fs::write(&plist_file, plist_content)
        .with_context(|| format!("unable to write LaunchAgents entry {}", plist_file.display()))?;

    Ok(plist_file)
}

fn render_service_plist(espanso_path: &str, user_path: &str) -> Result<String, RegisterError> {
    let espanso_path = escape_plist_value("the Espanso executable path", espanso_path)?;
    let user_path = escape_plist_value("PATH", user_path)?;

    Ok(SERVICE_PLIST_CONTENT
        .replace("{{{espanso_path}}}", &espanso_path)
        .replace("{{{PATH}}}", &user_path))
}

fn escape_plist_value(field: &'static str, value: &str) -> Result<String, RegisterError> {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if (character < ' ' && !matches!(character, '\t' | '\n' | '\r'))
            || matches!(character, '\u{FFFE}' | '\u{FFFF}')
        {
            return Err(RegisterError::InvalidXmlCharacter { field });
        }

        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\'' => escaped.push_str("&apos;"),
            '"' => escaped.push_str("&quot;"),
            _ => escaped.push(character),
        }
    }
    Ok(escaped)
}

#[cfg(target_os = "macos")]
fn launchctl_error(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    } else {
        stderr
    }
}

#[cfg(test)]
mod tests {
    use super::render_service_plist;

    #[test]
    fn service_plist_escapes_launchd_values_and_marks_login_launches() {
        let plist = render_service_plist(
            "/Applications/Espanso & Friends/<espanso>",
            "/usr/local/bin:/tmp/a&b<'\"",
        )
        .unwrap();

        assert!(plist.contains("/Applications/Espanso &amp; Friends/&lt;espanso&gt;"));
        assert!(plist.contains("/tmp/a&amp;b&lt;&apos;&quot;"));
        assert!(plist.contains("<string>--launch-at-login</string>"));
        assert!(!plist.contains("<key>KeepAlive</key>"));
    }

    #[test]
    fn service_plist_rejects_xml_invalid_values() {
        assert!(render_service_plist("/Applications/Espanso", "/usr/bin:\u{1}").is_err());
    }
}
