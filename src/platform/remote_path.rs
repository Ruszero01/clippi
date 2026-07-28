//! Remote file-system path detection that does not read file contents.

/// Return a stable host label for a remote path.
///
/// UNC paths are parsed as strings. On Windows, mapped network drives are
/// resolved through the local WNet mapping table. The result is intended to be
/// captured once and persisted; UI rendering must not call this function.
pub fn remote_host_label(path: &str) -> Option<String> {
    unc_host_label(path).or_else(|| mapped_drive_host_label(path))
}

fn unc_host_label(path: &str) -> Option<String> {
    let trimmed = path.trim();
    let rest = trimmed
        .strip_prefix("\\\\")
        .or_else(|| trimmed.strip_prefix("//"))?;
    let host = rest
        .split(['\\', '/'])
        .next()
        .map(str::trim)
        .filter(|host| !host.is_empty())?;
    Some(host.to_string())
}

#[cfg(target_os = "windows")]
fn mapped_drive_host_label(path: &str) -> Option<String> {
    use windows_sys::Win32::NetworkManagement::WNet::WNetGetConnectionW;
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;

    const DRIVE_REMOTE: u32 = 4;

    let bytes = path.as_bytes();
    if bytes.len() < 2 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
        return None;
    }

    let drive = format!("{}:", (bytes[0] as char).to_ascii_uppercase());
    let local_name: Vec<u16> = drive.encode_utf16().chain(std::iter::once(0)).collect();
    let mut remote_name = vec![0_u16; 1024];
    let mut remote_name_len = remote_name.len() as u32;

    // SAFETY: Both UTF-16 buffers are NUL-terminated or explicitly sized.
    // WNetGetConnectionW only reads the local drive name and writes at most
    // `remote_name_len` WCHARs into the owned output buffer.
    let result = unsafe {
        WNetGetConnectionW(
            local_name.as_ptr(),
            remote_name.as_mut_ptr(),
            &mut remote_name_len,
        )
    };
    if result == 0 {
        let end = remote_name
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(remote_name_len as usize)
            .min(remote_name.len());
        let remote = String::from_utf16_lossy(&remote_name[..end]);
        return unc_host_label(&remote).or(Some(drive));
    }

    let root: Vec<u16> = format!("{drive}\\")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `root` is a valid NUL-terminated UTF-16 drive-root string.
    (unsafe { GetDriveTypeW(root.as_ptr()) } == DRIVE_REMOTE).then_some(drive)
}

#[cfg(not(target_os = "windows"))]
fn mapped_drive_host_label(_path: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::unc_host_label;

    #[test]
    fn extracts_unc_server_name_without_file_system_access() {
        assert_eq!(
            unc_host_label(r"\\NAS01\photos\image.png"),
            Some("NAS01".to_string())
        );
        assert_eq!(
            unc_host_label(r"\\192.168.1.20\share\file.bin"),
            Some("192.168.1.20".to_string())
        );
    }

    #[test]
    fn rejects_local_paths() {
        assert_eq!(unc_host_label(r"C:\photos\image.png"), None);
        assert_eq!(unc_host_label("/Users/example/image.png"), None);
    }
}
