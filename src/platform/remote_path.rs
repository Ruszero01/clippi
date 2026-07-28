//! Remote file-system path detection that does not read file contents.

/// Return a stable host label for a remote path.
///
/// UNC paths are parsed as strings. Windows mapped drives and macOS network
/// mounts are resolved through the operating system's mount tables. The result
/// is intended to be captured once and persisted; UI rendering must not call
/// this function.
pub fn remote_host_label(path: &str) -> Option<String> {
    unc_host_label(path)
        .or_else(|| mapped_drive_host_label(path))
        .or_else(|| macos_mount_host_label(path))
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

#[cfg(target_os = "macos")]
fn macos_mount_host_label(path: &str) -> Option<String> {
    use std::ffi::CStr;
    use std::path::Path;

    let path = Path::new(path);
    if !path.is_absolute() {
        return None;
    }

    let mut mounts_ptr: *mut libc::statfs = std::ptr::null_mut();
    // SAFETY: getmntinfo writes a borrowed pointer to the system mount table.
    // MNT_NOWAIT returns cached mount information and does not refresh remote
    // volume statistics, which avoids network access on the clipboard thread.
    let mount_count = unsafe { libc::getmntinfo(&mut mounts_ptr, libc::MNT_NOWAIT) };
    if mount_count <= 0 || mounts_ptr.is_null() {
        return None;
    }

    // SAFETY: getmntinfo returned mount_count consecutive statfs entries. The
    // buffer remains valid until the next getmntinfo call in this process; all
    // strings needed below are copied before this function returns.
    let mounts = unsafe { std::slice::from_raw_parts(mounts_ptr, mount_count as usize) };
    let mut best_match: Option<(usize, u32, String, String)> = None;

    for mount in mounts {
        // SAFETY: statfs mount strings are fixed-size NUL-terminated C arrays.
        let mount_point = unsafe { CStr::from_ptr(mount.f_mntonname.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        let mount_path = Path::new(&mount_point);
        if !path.starts_with(mount_path) {
            continue;
        }

        let match_len = mount_path.components().count();
        if best_match
            .as_ref()
            .is_some_and(|(best_len, _, _, _)| *best_len >= match_len)
        {
            continue;
        }

        // SAFETY: f_mntfromname has the same NUL-terminated representation.
        let source = unsafe { CStr::from_ptr(mount.f_mntfromname.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        best_match = Some((match_len, mount.f_flags, source, mount_point));
    }

    let (_, flags, source, mount_point) = best_match?;
    if flags & libc::MNT_LOCAL as u32 != 0 {
        return None;
    }

    remote_mount_source_host_label(&source).or_else(|| {
        Path::new(&mount_point)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
    })
}

#[cfg(not(target_os = "macos"))]
fn macos_mount_host_label(_path: &str) -> Option<String> {
    None
}

fn remote_mount_source_host_label(source: &str) -> Option<String> {
    let source = source.trim();
    if source.is_empty() {
        return None;
    }

    if let Ok(parsed) = url::Url::parse(source) {
        if let Some(host) = parsed.host_str().filter(|host| !host.is_empty()) {
            return Some(host.to_string());
        }
    }

    if let Some(authority) = source.strip_prefix("//") {
        let authority = authority.split('/').next()?.trim();
        let host = authority.rsplit('@').next()?.trim_matches(['[', ']']);
        return (!host.is_empty()).then(|| host.to_string());
    }

    if let Some(rest) = source.strip_prefix('[') {
        let end = rest.find(']')?;
        let host = &rest[..end];
        return (!host.is_empty()).then(|| host.to_string());
    }

    source
        .split_once(":/")
        .map(|(host, _)| host.trim())
        .filter(|host| !host.is_empty() && !host.starts_with('/'))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::{remote_mount_source_host_label, unc_host_label};

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

    #[test]
    fn extracts_hosts_from_macos_network_mount_sources() {
        assert_eq!(
            remote_mount_source_host_label("//alice@nas.local/media"),
            Some("nas.local".to_string())
        );
        assert_eq!(
            remote_mount_source_host_label("fileserver:/exports/media"),
            Some("fileserver".to_string())
        );
        assert_eq!(
            remote_mount_source_host_label("smb://alice@nas.local/media"),
            Some("nas.local".to_string())
        );
        assert_eq!(
            remote_mount_source_host_label("[fe80::1]:/exports/media"),
            Some("fe80::1".to_string())
        );
    }

    #[test]
    fn rejects_local_mount_sources() {
        assert_eq!(remote_mount_source_host_label("/dev/disk3s1"), None);
        assert_eq!(remote_mount_source_host_label(""), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn local_macos_paths_are_not_remote_mounts() {
        assert_eq!(super::remote_host_label("/tmp/clippi-local-file"), None);
    }
}
