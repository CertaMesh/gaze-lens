//! One reader for every trusted operator file: configuration, certificate,
//! private key, authority and identity history. Type, ownership, privacy, the
//! containing directory and the opened inode are all verified before a byte is
//! admitted, and storage is sized from a constant rather than from metadata.
use gaze_lens_protocol::{Error, Result};
use std::{
    fs::{File, Metadata, OpenOptions},
    io::Read,
    path::Path,
};

/// Bytes admitted from a local operator file before parsing.
pub(crate) const FILE_BYTES: usize = 65_536;

/// Reads a trusted operator file. Anything that is not a regular file owned by
/// this process and readable by nobody else denies, as does a file whose opened
/// descriptor is not the inspected inode.
pub(crate) fn read(path: &Path) -> Result<Vec<u8>> {
    let mut file = open(path, OpenOptions::new().read(true))?;
    let mut bytes = vec![0u8; FILE_BYTES + 1];
    let mut read = 0;
    while read < bytes.len() {
        let got = file
            .read(&mut bytes[read..])
            .map_err(|_| Error::Unavailable)?;
        if got == 0 {
            break;
        }
        read += got;
    }
    if read > FILE_BYTES {
        return Err(Error::CapExceeded);
    }
    bytes.truncate(read);
    Ok(bytes)
}

/// Opens an existing trusted file with the reader's checks. The type is settled
/// on the link before the open, so a symlink or FIFO never reaches `open(2)`.
pub(crate) fn open(path: &Path, options: &OpenOptions) -> Result<File> {
    directory(path)?;
    let link = std::fs::symlink_metadata(path).map_err(|_| Error::Unavailable)?;
    private(&link, false)?;
    let file = options.open(path).map_err(|_| Error::Unavailable)?;
    let opened = file.metadata().map_err(|_| Error::Unavailable)?;
    private(&opened, false)?;
    same(&link, &opened)?;
    Ok(file)
}

/// Validates the containing directory and returns it for adjacent temporary
/// files. A path without a parent component is resolved against the working
/// directory, which must itself be private.
pub(crate) fn directory(path: &Path) -> Result<&Path> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    private(
        &std::fs::symlink_metadata(parent).map_err(|_| Error::Unavailable)?,
        true,
    )?;
    Ok(parent)
}

/// A regular file or directory that only the running user may read or write.
pub(crate) fn private(meta: &Metadata, directory: bool) -> Result<()> {
    let expected = if directory {
        meta.is_dir()
    } else {
        meta.is_file()
    };
    if !expected {
        return Err(Error::Unavailable);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o077 != 0 || meta.uid() != rustix::process::geteuid().as_raw() {
            return Err(Error::Unavailable);
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        // Non-Unix permission validation is deliberately unavailable pending an
        // ACL proof; no trusted file is admitted without it.
        Err(Error::Unavailable)
    }
}

/// The opened descriptor must be the inode that passed the checks above.
pub(crate) fn same(link: &Metadata, opened: &Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if link.ino() != opened.ino() || link.dev() != opened.dev() {
            return Err(Error::Unavailable);
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (link, opened);
        Err(Error::Unavailable)
    }
}
