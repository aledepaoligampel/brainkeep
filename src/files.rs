//! Unix-only local prototype. Other operating systems require their own permission adapter.
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::Path,
};
pub type Result<T> = std::result::Result<T, String>;
pub fn private_dir(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) => {
            if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
                return Err(
                    "data directory must be owned by this user, mode 0700, and not a symlink"
                        .into(),
                );
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)
                .map_err(|_| "cannot create private directory")?;
            return private_dir(path);
        }
        Err(_) => return Err("cannot inspect data directory".into()),
    }
    Ok(())
}
pub fn check_file(path: &Path) -> Result<()> {
    let m = fs::symlink_metadata(path).map_err(|_| "cannot inspect file")?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o077 != 0
        || m.nlink() != 1
    {
        return Err("file must be private, owned by this user, regular and not linked".into());
    }
    Ok(())
}
pub fn new_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "cannot create file; destination may already exist".into())
}
pub fn private_file(path: &Path) -> Result<File> {
    if !path.try_exists().map_err(|_| "cannot inspect file")? {
        return new_file(path);
    }
    check_file(path)?;
    OpenOptions::new()
        .write(true)
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "cannot open private file".into())
}
