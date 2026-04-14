//! Security helpers for MCP handlers.
//!
//! Every MCP tool that reads files or shells out to `git` needs a single,
//! consistent way to translate a user-supplied path into an absolute path
//! that is guaranteed to live under the server's configured project root.
//! `safe_join` is that gate: it rejects absolute paths, `..` traversal, and
//! any resolved location that escapes the root (including symlink escapes).
//!
//! Handlers should funnel *every* file read or subprocess CWD through this
//! module. Bypassing it — even "just to read one known file" — is how
//! MCP-over-HTTP deployments turn into arbitrary-file-read primitives.

use std::path::{Component, Path, PathBuf};

/// Error returned when a user-supplied path fails validation.
#[derive(Debug)]
pub struct PathSecurityError {
    pub message: String,
}

impl std::fmt::Display for PathSecurityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "path security: {}", self.message)
    }
}

impl std::error::Error for PathSecurityError {}

impl From<PathSecurityError> for String {
    fn from(e: PathSecurityError) -> Self {
        e.to_string()
    }
}

/// Safely join a user-supplied relative path onto `project_root`.
///
/// Returns the resolved absolute path on success. Rejects:
/// - absolute user paths (`/etc/passwd`, `C:\...`)
/// - parent traversal components (`..`)
/// - resolved paths that do not have `project_root` as a prefix (covers
///   symlink-based escapes once the target exists)
///
/// The `project_root` itself is canonicalized up front; non-existent
/// descendants of the root are permitted so this can be used for paths that
/// are about to be created.
pub fn safe_join(project_root: &str, user_path: &str) -> Result<PathBuf, PathSecurityError> {
    let root = Path::new(project_root)
        .canonicalize()
        .map_err(|e| PathSecurityError {
            message: format!("project root {:?} not accessible: {}", project_root, e),
        })?;

    let trimmed = user_path.trim_start_matches("./");
    let candidate = Path::new(trimmed);

    if candidate.is_absolute() {
        return Err(PathSecurityError {
            message: format!("absolute paths are not allowed: {}", user_path),
        });
    }

    for comp in candidate.components() {
        match comp {
            Component::ParentDir => {
                return Err(PathSecurityError {
                    message: format!("'..' traversal is not allowed: {}", user_path),
                });
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(PathSecurityError {
                    message: format!("absolute/drive paths are not allowed: {}", user_path),
                });
            }
            _ => {}
        }
    }

    let joined = root.join(candidate);

    // If the target exists, canonicalize and verify the resolved path is
    // still under root (defends against symlink escapes).
    if let Ok(canonical) = joined.canonicalize() {
        if !canonical.starts_with(&root) {
            return Err(PathSecurityError {
                message: format!("resolved path escapes project root: {}", user_path),
            });
        }
        return Ok(canonical);
    }

    // Non-existent path: we've already verified there are no '..' components
    // and no absolute prefix, so `joined` cannot escape root lexically.
    Ok(joined)
}

/// Verify a user-supplied path is safe, returning the original (relative)
/// path on success for use as a database key. Use when you don't actually
/// need the absolute path — just the guarantee that it's well-formed.
pub fn validate_relative(user_path: &str) -> Result<&str, PathSecurityError> {
    let trimmed = user_path.trim_start_matches("./");
    let candidate = Path::new(trimmed);

    if candidate.is_absolute() {
        return Err(PathSecurityError {
            message: format!("absolute paths are not allowed: {}", user_path),
        });
    }
    for comp in candidate.components() {
        if matches!(
            comp,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            return Err(PathSecurityError {
                message: format!("unsafe path component in: {}", user_path),
            });
        }
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn rejects_absolute_unix() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let err = safe_join(root, "/etc/passwd").unwrap_err();
        assert!(err.message.contains("absolute"));
    }

    #[test]
    fn rejects_parent_traversal() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let err = safe_join(root, "../../../etc/passwd").unwrap_err();
        assert!(err.message.contains("..") || err.message.contains("traversal"));
    }

    #[test]
    fn rejects_sneaky_parent_midpath() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let err = safe_join(root, "src/../../etc/passwd").unwrap_err();
        assert!(err.message.contains("traversal"));
    }

    #[test]
    fn accepts_normal_relative() {
        let tmp = tempdir().unwrap();
        let sub = tmp.path().join("src");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("lib.rs"), "fn main(){}").unwrap();

        let root = tmp.path().to_str().unwrap();
        let out = safe_join(root, "src/lib.rs").unwrap();
        assert!(out.ends_with("src/lib.rs"));
    }

    #[test]
    fn accepts_dot_slash_prefix() {
        let tmp = tempdir().unwrap();
        let sub = tmp.path().join("src");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("lib.rs"), "").unwrap();

        let root = tmp.path().to_str().unwrap();
        let out = safe_join(root, "./src/lib.rs").unwrap();
        assert!(out.ends_with("src/lib.rs"));
    }

    #[test]
    fn allows_nonexistent_child() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let out = safe_join(root, "new/file/path.rs").unwrap();
        assert!(out.starts_with(tmp.path().canonicalize().unwrap()));
    }

    #[test]
    fn rejects_symlink_escape() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let tmp = tempdir().unwrap();
            let outside = tempdir().unwrap();
            fs::write(outside.path().join("secret"), "pw").unwrap();
            symlink(outside.path().join("secret"), tmp.path().join("link")).unwrap();

            let root = tmp.path().to_str().unwrap();
            let err = safe_join(root, "link").unwrap_err();
            assert!(err.message.contains("escapes project root"));
        }
    }

    #[test]
    fn validate_relative_ok() {
        assert_eq!(validate_relative("src/foo.rs").unwrap(), "src/foo.rs");
        assert_eq!(validate_relative("./src/foo.rs").unwrap(), "src/foo.rs");
    }

    #[test]
    fn validate_relative_rejects_traversal() {
        assert!(validate_relative("../etc/passwd").is_err());
        assert!(validate_relative("/etc/passwd").is_err());
    }
}
