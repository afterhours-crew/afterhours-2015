// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The name our server copy must carry for the installed SDK to accept it.
//!
//! The client SDK checks that a process with the registered launcher's
//! basename, or that basename plus `Debug`, is running. We use the `Debug`
//! variant so the compatibility copy of our own server is distinguishable
//! from the actual EA application, which stays closed.

use crate::exit::{Failure, Result};

/// Supported registered launcher basenames and the compatibility name used
/// for each. `Origin.exe` is implemented but has not been tested live.
const SUPPORTED: &[(&str, &str)] = &[
    ("eadesktop.exe", "EADesktopDebug.exe"),
    ("origin.exe", "OriginDebug.exe"),
];

/// Whether a path is a fully qualified Windows path (`C:\...` or `\\...`).
///
/// Registry values are always Windows paths, and tests run on every
/// platform, so this does not use the host's own path rules.
pub fn is_fully_qualified_windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/');
    drive || path.starts_with("\\\\")
}

/// Compatibility executable name for a registered launcher path.
pub fn compatibility_name(registered_path: &str) -> Result<&'static str> {
    if !is_fully_qualified_windows_path(registered_path) {
        return Err(Failure::refused(
            "Launcher registration is not an absolute path.",
        ));
    }
    let basename = registered_path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    SUPPORTED
        .iter()
        .find(|(name, _)| *name == basename)
        .map(|(_, compat)| *compat)
        .ok_or_else(|| {
            Failure::refused("Unrecognized launcher registration; no machine setting was changed.")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_registrations_map_to_debug_names() {
        assert_eq!(
            compatibility_name("C:\\Program Files\\Electronic Arts\\EADesktop.exe").unwrap(),
            "EADesktopDebug.exe"
        );
        assert_eq!(
            compatibility_name("D:\\Origin\\Origin.exe").unwrap(),
            "OriginDebug.exe"
        );
        assert_eq!(
            compatibility_name("\\\\server\\share\\EADESKTOP.EXE").unwrap(),
            "EADesktopDebug.exe"
        );
    }

    #[test]
    fn relative_and_unknown_registrations_are_refused() {
        assert!(compatibility_name("EADesktop.exe").is_err());
        assert!(compatibility_name("C:\\Windows\\System32\\cmd.exe").is_err());
        assert!(compatibility_name("").is_err());
        assert!(compatibility_name("C:EADesktop.exe").is_err());
    }
}
