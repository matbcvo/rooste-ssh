//! SFTP over an existing SSH connection: listing, transfers with progress,
//! and file management. Remote paths are POSIX-style strings.

use std::path::Path;
use std::time::SystemTime;

use rooste_proto::ssh::{SshClient, SshError};
use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as ProtocolError;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CHUNK: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum SftpError {
    #[error("could not start SFTP")]
    Open(#[source] SshError),
    #[error("{0}")]
    Protocol(#[from] ProtocolError),
    #[error("{path}: {source}")]
    Local {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("transfer cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    Directory,
    File,
    Symlink,
    Other,
}

/// One item of a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// Full remote path.
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Unix permission bits, if the server reports them.
    pub permissions: Option<u32>,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Directory
    }
}

/// Join a remote directory and a name.
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// The parent of a remote path (`/` stays `/`).
pub fn parent(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => trimmed[..i].to_owned(),
    }
}

/// Human-readable size, e.g. `1.5 MiB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// An SFTP session on its own channel of the connection.
pub struct Sftp {
    session: SftpSession,
}

impl std::fmt::Debug for Sftp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sftp").finish_non_exhaustive()
    }
}

impl Sftp {
    pub async fn open(client: &SshClient) -> Result<Self, SftpError> {
        let stream = client
            .open_subsystem("sftp")
            .await
            .map_err(SftpError::Open)?;
        let session = SftpSession::new(stream).await?;
        Ok(Self { session })
    }

    /// The directory a new session starts in (the user's home).
    pub async fn home(&self) -> Result<String, SftpError> {
        Ok(self.session.canonicalize(".").await?)
    }

    pub async fn canonicalize(&self, path: &str) -> Result<String, SftpError> {
        Ok(self.session.canonicalize(path).await?)
    }

    /// Entries of `dir`, directories first, then by name; `.` and `..` omitted.
    pub async fn list(&self, dir: &str) -> Result<Vec<Entry>, SftpError> {
        let mut entries: Vec<Entry> = self
            .session
            .read_dir(dir)
            .await?
            .filter(|e| e.file_name() != "." && e.file_name() != "..")
            .map(|e| {
                let metadata = e.metadata();
                let file_type = metadata.file_type();
                let kind = if file_type.is_dir() {
                    EntryKind::Directory
                } else if file_type.is_symlink() {
                    EntryKind::Symlink
                } else if file_type.is_file() {
                    EntryKind::File
                } else {
                    EntryKind::Other
                };
                let name = e.file_name();
                Entry {
                    path: join(dir, &name),
                    name,
                    kind,
                    size: metadata.size.unwrap_or(0),
                    modified: metadata.modified().ok(),
                    permissions: metadata.permissions,
                }
            })
            .collect();
        entries.sort_by(|a, b| {
            (!a.is_dir(), a.name.to_lowercase()).cmp(&(!b.is_dir(), b.name.to_lowercase()))
        });
        Ok(entries)
    }

    pub async fn mkdir(&self, path: &str) -> Result<(), SftpError> {
        Ok(self.session.create_dir(path).await?)
    }

    pub async fn remove_file(&self, path: &str) -> Result<(), SftpError> {
        Ok(self.session.remove_file(path).await?)
    }

    pub async fn remove_dir(&self, path: &str) -> Result<(), SftpError> {
        Ok(self.session.remove_dir(path).await?)
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<(), SftpError> {
        Ok(self.session.rename(from, to).await?)
    }

    /// Copy a remote file to `local`. `progress(done, total)` is called as
    /// data arrives; returning `false` cancels (the partial file is removed).
    pub async fn download(
        &self,
        remote: &str,
        local: &Path,
        mut progress: impl FnMut(u64, u64) -> bool,
    ) -> Result<u64, SftpError> {
        let local_error = |source| SftpError::Local {
            path: local.display().to_string(),
            source,
        };
        let total = self.session.metadata(remote).await?.size.unwrap_or(0);
        let mut source = self.session.open(remote).await?;
        let mut target = tokio::fs::File::create(local).await.map_err(local_error)?;
        let mut buf = vec![0u8; CHUNK];
        let mut done = 0u64;
        loop {
            let n = source.read(&mut buf).await.map_err(|e| SftpError::Local {
                path: remote.to_owned(),
                source: e,
            })?;
            if n == 0 {
                break;
            }
            target.write_all(&buf[..n]).await.map_err(local_error)?;
            done += n as u64;
            if !progress(done, total.max(done)) {
                drop(target);
                let _ = tokio::fs::remove_file(local).await;
                return Err(SftpError::Cancelled);
            }
        }
        target.flush().await.map_err(local_error)?;
        Ok(done)
    }

    /// Copy `local` to a remote file, replacing it. Cancelling via
    /// `progress` leaves the partial remote file removed.
    pub async fn upload(
        &self,
        local: &Path,
        remote: &str,
        mut progress: impl FnMut(u64, u64) -> bool,
    ) -> Result<u64, SftpError> {
        let local_error = |source| SftpError::Local {
            path: local.display().to_string(),
            source,
        };
        let mut source = tokio::fs::File::open(local).await.map_err(local_error)?;
        let total = source.metadata().await.map_err(local_error)?.len();
        let mut target = self.session.create(remote).await?;
        let mut buf = vec![0u8; CHUNK];
        let mut done = 0u64;
        loop {
            let n = source.read(&mut buf).await.map_err(local_error)?;
            if n == 0 {
                break;
            }
            target
                .write_all(&buf[..n])
                .await
                .map_err(|e| SftpError::Local {
                    path: remote.to_owned(),
                    source: e,
                })?;
            done += n as u64;
            if !progress(done, total.max(done)) {
                drop(target);
                let _ = self.session.remove_file(remote).await;
                return Err(SftpError::Cancelled);
            }
        }
        target.shutdown().await.map_err(|e| SftpError::Local {
            path: remote.to_owned(),
            source: e,
        })?;
        Ok(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_paths() {
        assert_eq!(join("/home/me", "a.txt"), "/home/me/a.txt");
        assert_eq!(join("/", "etc"), "/etc");
        assert_eq!(parent("/home/me"), "/home");
        assert_eq!(parent("/home"), "/");
        assert_eq!(parent("/"), "/");
        assert_eq!(parent("/home/me/"), "/home");
    }

    #[test]
    fn sizes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1536), "1.5 KiB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MiB");
    }
}
