//! Private, bounded, capability-relative filesystem operations.
use anyhow::{bail, Context, Result};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, DirBuilder, OpenOptions};
#[cfg(unix)]
use cap_std::fs::{DirBuilderExt, OpenOptionsExt};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

pub const MAX_FILE_BYTES: usize = 100 * 1024 * 1024;

pub fn safe_relative(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 1024
        && !name.contains(['\\', ':'])
        && name.split('/').all(|s| {
            !s.is_empty()
                && s != "."
                && s != ".."
                && !s.ends_with(['.', ' '])
                && !s.chars().any(char::is_control)
                && !matches!(
                    s.split('.')
                        .next()
                        .unwrap_or("")
                        .to_ascii_uppercase()
                        .as_str(),
                    "CON"
                        | "PRN"
                        | "AUX"
                        | "NUL"
                        | "COM1"
                        | "COM2"
                        | "COM3"
                        | "COM4"
                        | "COM5"
                        | "COM6"
                        | "COM7"
                        | "COM8"
                        | "COM9"
                        | "LPT1"
                        | "LPT2"
                        | "LPT3"
                        | "LPT4"
                        | "LPT5"
                        | "LPT6"
                        | "LPT7"
                        | "LPT8"
                        | "LPT9"
                )
        })
}
pub fn identifier(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}
fn builder() -> DirBuilder {
    #[allow(unused_mut)]
    let mut b = DirBuilder::new();
    #[cfg(unix)]
    b.mode(0o700);
    b
}
/// Walk each component through an open directory handle. Never follow a link.
pub fn dir(path: &Path) -> Result<Dir> {
    if crate::bypass::get().skip_path_checks {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(path)?;
        return Ok(Dir::open_ambient_dir(path, cap_std::ambient_authority())?);
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    // macOS's root-owned system aliases are not attacker-controlled output links.
    #[cfg(target_os = "macos")]
    let absolute = if let Ok(rest) = absolute.strip_prefix("/var") {
        Path::new("/private/var").join(rest)
    } else if let Ok(rest) = absolute.strip_prefix("/tmp") {
        Path::new("/private/tmp").join(rest)
    } else {
        absolute
    };
    let mut root = PathBuf::new();
    let mut components = Vec::new();
    for c in absolute.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => root.push(c.as_os_str()),
            Component::Normal(n) => components.push(n.to_owned()),
            Component::CurDir => {}
            Component::ParentDir => bail!("parent traversal is not allowed in output paths"),
        }
    }
    let mut d = Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
    for c in components {
        let created = match d.create_dir_with(&c, &builder()) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(e) => return Err(e.into()),
        };
        d = d
            .open_dir_nofollow(&c)
            .context("output directory must not be a symlink")?;
        if created {
            restrict(&d)?;
        }
    }
    Ok(d)
}
pub fn check_destination(path: &Path, force: bool) -> Result<()> {
    let force = force || crate::bypass::get().overwrite;
    if crate::bypass::get().skip_path_checks && force {
        return Ok(());
    }
    match std::fs::symlink_metadata(path) {
        Ok(m) if !m.is_file() || m.file_type().is_symlink() || !force => {
            bail!("destination exists or is unsafe: {}", display(path))
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
pub fn write(path: &Path, bytes: &[u8], force: bool) -> Result<()> {
    let force = force || crate::bypass::get().overwrite;
    check_destination(path, force)?;
    if crate::bypass::get().skip_path_checks {
        dir(path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")))?;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true);
        if force {
            opts.create(true).truncate(true);
        } else {
            opts.create_new(true);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // mode(0600) only applies at creation; tighten an existing target too.
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        restrict(&file)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        return Ok(());
    }
    let parent = dir(path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new(".")))?;
    let name = path.file_name().context("output needs a filename")?;
    if let Ok(m) = parent.symlink_metadata(name) {
        if !m.is_file() || m.file_type().is_symlink() || !force {
            bail!("destination exists or is unsafe");
        }
    }
    let temp = format!(".flightlog-{}.tmp", uuid::Uuid::new_v4());
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    opts.mode(0o600);
    let result = (|| -> Result<()> {
        let mut file = parent.open_with(&temp, &opts)?;
        restrict(&file)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if force {
            parent.rename(&temp, &parent, name)?;
        } else {
            parent.hard_link(&temp, &parent, name)?;
        }
        Ok(())
    })();
    let _ = parent.remove_file(&temp);
    result
}
pub fn read(path: &Path, max: usize) -> Result<Vec<u8>> {
    let max = if crate::bypass::get().skip_size_checks {
        usize::MAX
    } else {
        max
    };
    if crate::bypass::get().skip_path_checks {
        let file = std::fs::File::open(path)?;
        let mut bytes = Vec::new();
        file.take((max as u64).saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() > max {
            bail!("input exceeds size limit");
        }
        return Ok(bytes);
    }
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        bail!("input must be a regular file");
    }
    if meta.len() > max as u64 {
        bail!("input exceeds size limit");
    }
    let parent = dir(path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new(".")))?;
    let mut opts = OpenOptions::new();
    opts.read(true).follow(FollowSymlinks::No);
    let file = parent.open_with(path.file_name().context("input needs a filename")?, &opts)?;
    let mut bytes = Vec::new();
    file.take((max as u64).saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() > max {
        bail!("input exceeds size limit");
    }
    Ok(bytes)
}
pub fn text(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_control() || matches!(c, '\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}') {
                c.escape_unicode().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}
pub fn display(p: &Path) -> String {
    text(&p.to_string_lossy())
}
/// Quote as a POSIX shell word. Windows commands use the same constrained IDs;
/// arbitrary path commands are displayed as separate arguments instead.
#[cfg(not(windows))]
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Set a protected owner/system DACL on Windows before sensitive bytes are written.
#[cfg(windows)]
fn restrict(file: &impl std::os::windows::io::AsRawHandle) -> Result<()> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, LocalFree, INVALID_HANDLE_VALUE},
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SetSecurityInfo,
                SDDL_REVISION_1, SE_FILE_OBJECT,
            },
            GetSecurityDescriptorDacl, DACL_SECURITY_INFORMATION,
            PROTECTED_DACL_SECURITY_INFORMATION,
        },
        Storage::FileSystem::{
            ReOpenFile, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        },
    };
    let sddl: Vec<u16> = "D:P(A;;FA;;;SY)(A;;FA;;;OW)\0".encode_utf16().collect();
    let mut descriptor = null_mut();
    // SAFETY: valid NUL-terminated UTF-16; returned allocation remains live until LocalFree.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        ) == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = null_mut();
        if GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) == 0
            || present == 0
            || acl.is_null()
        {
            LocalFree(descriptor);
            bail!("cannot construct private file permissions");
        }
        let writable = ReOpenFile(
            file.as_raw_handle(),
            0x00040000, /* WRITE_DAC */
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAG_BACKUP_SEMANTICS,
        );
        if writable == INVALID_HANDLE_VALUE {
            LocalFree(descriptor);
            return Err(std::io::Error::last_os_error().into());
        }
        let status = SetSecurityInfo(
            writable,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            acl,
            null(),
        );
        CloseHandle(writable);
        LocalFree(descriptor);
        if status != 0 {
            return Err(std::io::Error::from_raw_os_error(status as i32).into());
        }
    }
    Ok(())
}
#[cfg(not(windows))]
fn restrict<T>(_: &T) -> Result<()> {
    Ok(())
}

pub fn create_dir(path: &Path) -> Result<()> {
    let parent = dir(path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new(".")))?;
    let name = path.file_name().context("directory needs a name")?;
    parent.create_dir_with(name, &builder())?;
    restrict(&parent.open_dir_nofollow(name)?)?;
    Ok(())
}

pub fn read_text(path: &Path, max: usize) -> Result<String> {
    Ok(String::from_utf8(read(path, max)?)?)
}
