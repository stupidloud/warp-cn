use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use warp_util::content_version::ContentVersion;
use warp_util::remote_path::RemotePath;
use warp_util::standardized_path::StandardizedPath;

/// Uniquely identifies where a file lives — either on the local filesystem
/// or on a remote host. Used across both the buffer model and the
/// editor/view layers as the canonical file-identity type.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum LocalOrRemotePath {
    /// File on the local filesystem.
    Local(PathBuf),
    /// File on a remote host, identified by host + path.
    Remote(RemotePath),
}

impl LocalOrRemotePath {
    /// Returns `true` if this is a `Local` location.
    pub fn is_local(&self) -> bool {
        matches!(self, LocalOrRemotePath::Local(_))
    }

    /// Returns `true` if this is a `Remote` location.
    pub fn is_remote(&self) -> bool {
        matches!(self, LocalOrRemotePath::Remote(_))
    }

    /// Returns the standardized path component of the location, regardless of where it lives.
    pub fn path_component(&self) -> StandardizedPath {
        match self {
            LocalOrRemotePath::Local(path) => StandardizedPath::from_local_absolute_unchecked(path),
            LocalOrRemotePath::Remote(remote) => remote.path.clone(),
        }
    }

    /// Returns the file name component for display (e.g. tab titles).
    pub fn display_name(&self) -> &str {
        match self {
            LocalOrRemotePath::Local(path) => path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default(),
            LocalOrRemotePath::Remote(remote) => remote.path.file_name().unwrap_or_default(),
        }
    }

    /// Returns a displayable path string.
    pub fn display_path(&self) -> String {
        match self {
            LocalOrRemotePath::Local(path) => path.to_string_lossy().to_string(),
            LocalOrRemotePath::Remote(remote) => format!("{}", remote.path),
        }
    }

    /// Returns the local path if this is a `Local` location, `None` for `Remote`.
    /// Callers that only work with local files (LSP, save-to-disk, reveal-in-finder)
    /// should use this to gate their behavior.
    pub fn to_local_path(&self) -> Option<&Path> {
        match self {
            LocalOrRemotePath::Local(path) => Some(path.as_path()),
            LocalOrRemotePath::Remote(_) => None,
        }
    }

    /// Joins a segment onto this location, preserving the host for remote paths.
    pub fn join(&self, segment: &str) -> LocalOrRemotePath {
        match self {
            LocalOrRemotePath::Local(path) => LocalOrRemotePath::Local(path.join(segment)),
            LocalOrRemotePath::Remote(remote) => {
                let joined = remote.path.join(segment);
                LocalOrRemotePath::Remote(RemotePath::new(remote.host_id.clone(), joined))
            }
        }
    }

    /// If `file` shares this location's host and starts with this location's path,
    /// returns the relative remainder as a string.
    pub fn strip_repo_prefix(&self, file: &LocalOrRemotePath) -> Option<String> {
        match (self, file) {
            (LocalOrRemotePath::Local(repo), LocalOrRemotePath::Local(file)) => file
                .strip_prefix(repo)
                .ok()
                .map(|path| path.to_string_lossy().into_owned()),
            (LocalOrRemotePath::Remote(repo), LocalOrRemotePath::Remote(file))
                if repo.host_id == file.host_id =>
            {
                file.path.strip_prefix(&repo.path).map(str::to_owned)
            }
            _ => None,
        }
    }
}

impl From<PathBuf> for LocalOrRemotePath {
    fn from(path: PathBuf) -> Self {
        LocalOrRemotePath::Local(path)
    }
}

impl From<&Path> for LocalOrRemotePath {
    fn from(path: &Path) -> Self {
        LocalOrRemotePath::Local(path.to_path_buf())
    }
}

impl From<&PathBuf> for LocalOrRemotePath {
    fn from(path: &PathBuf) -> Self {
        LocalOrRemotePath::Local(path.clone())
    }
}

impl From<&LocalOrRemotePath> for LocalOrRemotePath {
    fn from(path: &LocalOrRemotePath) -> Self {
        path.clone()
    }
}

impl From<RemotePath> for LocalOrRemotePath {
    fn from(remote: RemotePath) -> Self {
        LocalOrRemotePath::Remote(remote)
    }
}

impl AsRef<Path> for LocalOrRemotePath {
    fn as_ref(&self) -> &Path {
        match self {
            LocalOrRemotePath::Local(path) => path.as_path(),
            LocalOrRemotePath::Remote(remote) => Path::new(remote.path.as_str()),
        }
    }
}

/// Tracks sync state between client and server for a single remote buffer.
///
/// Uses a version vector with two components:
/// - `server_version`: bumped by the server when the file changes on disk.
/// - `client_version`: bumped by the client when the user edits the buffer.
///
/// Conflict detection:
/// - Server pushes `{S_new, C_expected}`. Client checks `C_expected == local client_version`.
///   Match → accept. Mismatch → conflict.
/// - Client sends `{S_expected, C_new}`. Server checks `S_expected == local server_version`.
///   Match → accept. Mismatch → reject (server pushes its current state).
///
/// Both fields use `ContentVersion` internally. At the wire boundary (proto
/// encode/decode), convert via `ContentVersion::as_u64()` and
/// `ContentVersion::from_raw()`.
#[derive(Clone, Debug)]
pub struct SyncClock {
    /// Last version acknowledged from the server (file-watcher side).
    pub server_version: ContentVersion,
    /// Last version acknowledged from the client (user-edit side).
    pub client_version: ContentVersion,
}

impl SyncClock {
    #[cfg_attr(not(feature = "local_fs"), allow(dead_code))]
    pub fn new() -> Self {
        Self {
            server_version: ContentVersion::from_raw(0),
            client_version: ContentVersion::from_raw(0),
        }
    }

    /// Reconstruct a `SyncClock` from wire values (proto deserialization).
    pub fn from_wire(server_version: u64, client_version: u64) -> Self {
        Self {
            server_version: ContentVersion::from_raw(server_version as usize),
            client_version: ContentVersion::from_raw(client_version as usize),
        }
    }

    /// Bump the server version after a file-watcher change.
    #[cfg_attr(not(feature = "local_fs"), allow(dead_code))]
    pub fn bump_server(&mut self) -> ContentVersion {
        self.server_version = ContentVersion::new();
        self.server_version
    }

    /// Check whether a server push's expected client version matches our local state.
    pub fn server_push_matches(&self, expected_client_version: ContentVersion) -> bool {
        self.client_version == expected_client_version
    }

    /// Check whether a client edit's expected server version matches our local state.
    #[cfg_attr(not(feature = "local_fs"), allow(dead_code))]
    pub fn client_edit_matches(&self, expected_server_version: ContentVersion) -> bool {
        self.server_version == expected_server_version
    }
}

#[cfg(test)]
#[path = "buffer_location_tests.rs"]
mod tests;
