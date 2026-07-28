//! Windows ``Win+V`` takeover — registry read/write and ownership tracking.
//!
//! This module implements the ``DisabledHotkeys`` approach: it adds/removes
//! the letter `V` from the Explorer ``DisabledHotkeys`` registry value so that
//! ``Win+V`` is released for ``RegisterHotKey``.  A separate ownership marker
//! (``ManagedDisabledHotkeyV`` under ``HKCU\Software\Clippi``) tracks whether
//! Clippi is responsible for the `V` so we never remove another application's
//! entry.
//!
//! All public functions are only compiled on Windows.

use std::fmt;

// ---------------------------------------------------------------------------
// Pure string helpers (no I/O) — these can be tested without touching the
// registry.
// ---------------------------------------------------------------------------

/// Add *letter* (a single uppercase ASCII `char`) to `current`.
/// Only appends the letter if not already present; does not reorder or
/// otherwise modify existing characters.
pub fn add_disabled_hotkey_letter(current: &str, letter: char) -> String {
    debug_assert!(
        letter.is_ascii_uppercase(),
        "letter must be ASCII uppercase"
    );

    if current.contains(letter) {
        return current.to_string();
    }

    let mut result = current.to_string();
    result.push(letter);
    result
}

/// Remove *letter* (a single uppercase ASCII `char`) from `current`.
/// Returns the resulting string (may be empty).  Preserves all remaining
/// characters in their original order.
pub fn remove_disabled_hotkey_letter(current: &str, letter: char) -> String {
    debug_assert!(
        letter.is_ascii_uppercase(),
        "letter must be ASCII uppercase"
    );

    if !current.contains(letter) {
        return current.to_string();
    }

    current.chars().filter(|&c| c != letter).collect()
}

// ---------------------------------------------------------------------------
// Registry paths and value names (Windows-only).
// ---------------------------------------------------------------------------

/// Registry sub-key under `HKEY_CURRENT_USER` that holds `DisabledHotkeys`.
#[cfg(target_os = "windows")]
const EXPLORER_ADVANCED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
#[cfg(target_os = "windows")]
const DISABLED_HOTKEYS_VALUE: &str = "DisabledHotkeys";

/// Clippi's private ownership marker.
#[cfg(target_os = "windows")]
const CLIPPI_REG_KEY: &str = r"Software\Clippi";
#[cfg(target_os = "windows")]
const MANAGED_DISABLED_V: &str = "ManagedDisabledHotkeyV";

/// The letter we manage.
const TAKEOVER_LETTER: char = 'V';

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Snapshot of the current `DisabledHotkeys` state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinVRegistrySnapshot {
    /// True when `V` is present in `DisabledHotkeys`.
    pub win_v_disabled: bool,
    /// True when Clippi's ownership marker (`ManagedDisabledHotkeyV=1`) exists.
    pub managed_by_clippi: bool,
}

/// What the mutating registry operation actually did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinVRegistryMutation {
    /// `V` was already present in `DisabledHotkeys` — Clippi did not touch it.
    VAlreadyPresent,
    /// Clippi added `V` and created the ownership marker.
    AddedByClippi,
    /// Clippi removed its `V` and cleared the ownership marker.
    RemovedByClippi,
    /// Clippi does not own `V`, so it left the value unchanged.
    SkippedNoOwnership,
}

impl fmt::Display for WinVRegistryMutation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VAlreadyPresent => {
                f.write_str("V already present in DisabledHotkeys; left unchanged")
            }
            Self::AddedByClippi => f.write_str("Added V to DisabledHotkeys"),
            Self::RemovedByClippi => f.write_str("Removed V from DisabledHotkeys"),
            Self::SkippedNoOwnership => {
                f.write_str("Skipped: Clippi does not own DisabledHotkeys V")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Windows registry I/O
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use winreg::enums::{RegType, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
    use winreg::RegKey;

    /// The Win32 error code for "access denied".
    const ERROR_ACCESS_DENIED: i32 = 5;

    /// Read `DisabledHotkeys` and Clippi's ownership marker.
    /// Only treats a genuinely missing value as empty; other errors are
    /// propagated as `RegistryError`.
    pub fn inspect_win_v_registry() -> Result<WinVRegistrySnapshot, String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let explorer = hkcu
            .open_subkey_with_flags(EXPLORER_ADVANCED_KEY, KEY_READ)
            .map_err(|e| format!("Failed to open Explorer\\Advanced key: {e}"))?;

        // Only treat genuine NotFound as empty; propagate other errors.
        let disabled_raw: String = match explorer.get_raw_value(DISABLED_HOTKEYS_VALUE) {
            Ok(raw_val) => {
                if raw_val.vtype != RegType::REG_SZ && raw_val.vtype != RegType::REG_EXPAND_SZ {
                    return Err(format!(
                        "DisabledHotkeys is an unsupported registry type ({:?})",
                        raw_val.vtype
                    ));
                }
                // Type is valid — now read the actual string.  Propagate
                // decode errors; only NotFound means "value is absent".
                match explorer.get_value::<String, _>(DISABLED_HOTKEYS_VALUE) {
                    Ok(s) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                    Err(e) => {
                        return Err(format!("Failed to decode DisabledHotkeys as string: {e}"));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                return Err(format!("Failed to read DisabledHotkeys: {e}"));
            }
        };

        let v_present = disabled_raw.contains(TAKEOVER_LETTER);

        // Only NotFound means "Clippi has never written ownership";
        // access-denied or corrupt data must propagate as an error.
        let owned = match hkcu.open_subkey_with_flags(CLIPPI_REG_KEY, KEY_READ) {
            Ok(cli) => match cli.get_value::<u32, _>(MANAGED_DISABLED_V) {
                Ok(v) => v == 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                Err(e) => {
                    return Err(format!("Failed to read ManagedDisabledHotkeyV: {e}"));
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => {
                return Err(format!("Failed to open Clippi registry key: {e}"));
            }
        };

        Ok(WinVRegistrySnapshot {
            win_v_disabled: v_present,
            managed_by_clippi: owned,
        })
    }

    /// Set Clippi's ownership marker to `1`.
    fn set_ownership_marker() -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (cli, _) = hkcu
            .create_subkey(CLIPPI_REG_KEY)
            .map_err(|e| format!("Failed to create/open Clippi key: {e}"))?;
        cli.set_value(MANAGED_DISABLED_V, &1u32)
            .map_err(|e| format!("Failed to set ManagedDisabledHotkeyV: {e}"))
    }

    /// Remove Clippi's ownership marker.  Returns Ok even when the value or
    /// key doesn't exist (already cleaned up).  Propagates permission errors.
    fn clear_ownership_marker() -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        match hkcu.open_subkey_with_flags(CLIPPI_REG_KEY, KEY_SET_VALUE) {
            Ok(cli) => match cli.delete_value(MANAGED_DISABLED_V) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("Failed to delete ManagedDisabledHotkeyV: {e}")),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("Failed to open Clippi key for cleanup: {e}")),
        }
    }

    /// Write the new `DisabledHotkeys` string, preserving the existing value
    /// type (`REG_SZ` vs `REG_EXPAND_SZ`).  Refuses to overwrite non-string
    /// types.
    fn write_disabled_hotkeys(value: &str) -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (explorer, _) = hkcu
            .create_subkey(EXPLORER_ADVANCED_KEY)
            .map_err(|e| format!("Failed to create/open Explorer\\Advanced key: {e}"))?;

        // Read the existing value type first.  Only treat NotFound as
        // "value absent" (default to REG_SZ); propagate other errors.
        let raw_type = match explorer.get_raw_value(DISABLED_HOTKEYS_VALUE) {
            Ok(raw) => raw.vtype,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => RegType::REG_SZ,
            Err(e) => {
                return Err(format!("Failed to read DisabledHotkeys type: {e}"));
            }
        };

        if raw_type != RegType::REG_SZ && raw_type != RegType::REG_EXPAND_SZ {
            let raw = explorer.get_raw_value(DISABLED_HOTKEYS_VALUE);
            return Err(format!(
                "DisabledHotkeys is an unsupported registry type ({raw_type:?}); current raw value: {raw:?}"
            ));
        }

        let result = if raw_type == RegType::REG_EXPAND_SZ {
            // Preserve REG_EXPAND_SZ by writing raw bytes (UTF-16LE + NUL).
            let mut bytes: Vec<u8> = Vec::new();
            for c in value.encode_utf16() {
                bytes.extend_from_slice(&c.to_le_bytes());
            }
            bytes.extend_from_slice(&[0u8, 0]); // NUL terminator
            let raw_value = winreg::RegValue {
                bytes: std::borrow::Cow::Owned(bytes),
                vtype: RegType::REG_EXPAND_SZ,
            };
            explorer.set_raw_value(DISABLED_HOTKEYS_VALUE, &raw_value)
        } else {
            explorer.set_value(DISABLED_HOTKEYS_VALUE, &value)
        };

        result.map_err(|e| {
            if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) {
                "Access denied writing to DisabledHotkeys.".to_string()
            } else {
                format!("Failed to write DisabledHotkeys: {e}")
            }
        })
    }

    /// Enable takeover: add `V` to `DisabledHotkeys` if not already present,
    /// and create the ownership marker.
    ///
    /// On failure after writing DisabledHotkeys but before setting the
    /// ownership marker, the partial `V` write is rolled back so no orphaned
    /// registry change is left behind.
    pub fn configure_win_v_takeover() -> Result<WinVRegistryMutation, String> {
        let snapshot = inspect_win_v_registry()?;

        if snapshot.win_v_disabled {
            return Ok(WinVRegistryMutation::VAlreadyPresent);
        }

        // Re-read the current value to avoid races.
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let explorer = hkcu
            .open_subkey_with_flags(EXPLORER_ADVANCED_KEY, KEY_READ)
            .map_err(|e| format!("Failed to open Explorer\\Advanced key: {e}"))?;
        let current: String = explorer
            .get_value(DISABLED_HOTKEYS_VALUE)
            .unwrap_or_default();

        let new_value = add_disabled_hotkey_letter(&current, TAKEOVER_LETTER);

        // Step 1: Write the new DisabledHotkeys value.
        write_disabled_hotkeys(&new_value)?;

        // Step 2: Set the ownership marker.  If this fails, roll back step 1.
        if let Err(own_err) = set_ownership_marker() {
            match write_disabled_hotkeys(&current) {
                Ok(()) => Err(format!(
                    "Ownership marker write failed; V was rolled back: {own_err}"
                )),
                Err(rollback_err) => Err(format!(
                    "CRITICAL: Ownership marker write failed AND rollback also failed!\n\
                     V may still be in DisabledHotkeys without Clippi ownership.\n\
                     Marker error: {own_err}\n\
                     Rollback error: {rollback_err}\n\
                     Manual recovery: remove 'V' from DisabledHotkeys in registry."
                )),
            }
        } else {
            Ok(WinVRegistryMutation::AddedByClippi)
        }
    }

    /// Disable takeover: remove `V` from `DisabledHotkeys` **only** if
    /// Clippi owns it, then clear the ownership marker.
    ///
    /// If clearing the ownership marker fails after `V` was already removed,
    /// the removal is rolled back so the registry remains consistent.
    pub fn restore_win_v_if_managed() -> Result<WinVRegistryMutation, String> {
        let snapshot = inspect_win_v_registry()?;

        if !snapshot.managed_by_clippi {
            return Ok(WinVRegistryMutation::SkippedNoOwnership);
        }

        // Re-read the current value.
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let explorer = hkcu
            .open_subkey_with_flags(EXPLORER_ADVANCED_KEY, KEY_READ)
            .map_err(|e| format!("Failed to open Explorer\\Advanced key: {e}"))?;
        let current: String = explorer
            .get_value(DISABLED_HOTKEYS_VALUE)
            .unwrap_or_default();

        let new_value = remove_disabled_hotkey_letter(&current, TAKEOVER_LETTER);

        // Write the new value (even if empty), keeping the key present.
        write_disabled_hotkeys(&new_value)?;

        // Clear the ownership marker.  If this fails, restore the original
        // DisabledHotkeys value so the registry stays consistent.
        if let Err(marker_err) = clear_ownership_marker() {
            match write_disabled_hotkeys(&current) {
                Ok(()) => Err(format!(
                    "Ownership marker removal failed; V was restored: {marker_err}"
                )),
                Err(rollback_err) => Err(format!(
                    "CRITICAL: Ownership marker removal failed AND rollback also failed!\n\
                     V may have been removed without clearing ownership.\n\
                     Marker error: {marker_err}\n\
                     Rollback error: {rollback_err}"
                )),
            }
        } else {
            Ok(WinVRegistryMutation::RemovedByClippi)
        }
    }
}

#[cfg(target_os = "windows")]
#[allow(unused_imports)]
pub use imp::*;

// ---------------------------------------------------------------------------
// Stub implementations for non-Windows platforms (never called at runtime).
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "windows"))]
pub fn inspect_win_v_registry() -> Result<WinVRegistrySnapshot, String> {
    Err("Not available on this platform".into())
}

#[cfg(not(target_os = "windows"))]
pub fn configure_win_v_takeover() -> Result<WinVRegistryMutation, String> {
    Err("Not available on this platform".into())
}

#[cfg(not(target_os = "windows"))]
pub fn restore_win_v_if_managed() -> Result<WinVRegistryMutation, String> {
    Err("Not available on this platform".into())
}

// ---------------------------------------------------------------------------
// Unit tests — pure string logic, no registry I/O.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- add_disabled_hotkey_letter --

    #[test]
    fn add_to_empty() {
        assert_eq!(add_disabled_hotkey_letter("", 'V'), "V");
    }

    #[test]
    fn add_to_single() {
        assert_eq!(add_disabled_hotkey_letter("A", 'V'), "AV");
    }

    #[test]
    fn add_when_already_present() {
        assert_eq!(add_disabled_hotkey_letter("AV", 'V'), "AV");
    }

    #[test]
    fn add_does_not_reorder() {
        // V is appended; existing order is preserved.
        assert_eq!(add_disabled_hotkey_letter("BA", 'V'), "BAV");
    }

    #[test]
    fn add_preserves_non_ascii_upper() {
        assert_eq!(add_disabled_hotkey_letter("A-B", 'V'), "A-BV");
    }

    // -- remove_disabled_hotkey_letter --

    #[test]
    fn remove_from_absent() {
        assert_eq!(remove_disabled_hotkey_letter("AC", 'V'), "AC");
    }

    #[test]
    fn remove_from_present() {
        assert_eq!(remove_disabled_hotkey_letter("ACV", 'V'), "AC");
    }

    #[test]
    fn remove_last() {
        assert_eq!(remove_disabled_hotkey_letter("V", 'V'), "");
    }

    #[test]
    fn remove_preserves_order_non_ascii() {
        // V is removed; remaining chars keep original order.
        assert_eq!(remove_disabled_hotkey_letter("AVB-", 'V'), "AB-");
    }

    // -- WinVRegistryMutation display --

    #[test]
    fn mutation_display_is_human_readable() {
        let texts: Vec<String> = [
            WinVRegistryMutation::VAlreadyPresent,
            WinVRegistryMutation::AddedByClippi,
            WinVRegistryMutation::RemovedByClippi,
            WinVRegistryMutation::SkippedNoOwnership,
        ]
        .iter()
        .map(|m| m.to_string())
        .collect();
        for t in &texts {
            assert!(!t.is_empty(), "mutation display should not be empty");
        }
    }
}
