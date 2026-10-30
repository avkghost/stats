use std::{
    fs,
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FileSandboxError {
    #[error("allowed filesystem root does not exist: {path}: {source}")]
    Root {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("allowed filesystem root is not a directory: {path}")]
    RootNotDirectory { path: PathBuf },
    #[error("cannot resolve filesystem path {path}: {source}")]
    Resolve {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("filesystem path {path} is outside allowed roots")]
    Outside { path: PathBuf },
}

/// Canonical, read-only filesystem boundary for the CGI and HTTP frontends.
#[derive(Debug, Clone)]
pub struct FileSandbox {
    roots: Vec<PathBuf>,
}

impl FileSandbox {
    pub fn for_data_dir(data_dir: &Path) -> Result<Self, FileSandboxError> {
        Self::new([data_dir.to_path_buf()])
    }

    pub fn new<I>(roots: I) -> Result<Self, FileSandboxError>
    where
        I: IntoIterator<Item = PathBuf>,
    {
        let mut canonical_roots = Vec::new();
        for root in roots {
            let canonical = fs::canonicalize(&root).map_err(|source| FileSandboxError::Root {
                path: root.clone(),
                source,
            })?;
            if !canonical.is_dir() {
                return Err(FileSandboxError::RootNotDirectory { path: root });
            }
            if !canonical_roots.contains(&canonical) {
                canonical_roots.push(canonical);
            }
        }
        Ok(Self {
            roots: canonical_roots,
        })
    }

    pub fn check_existing(&self, path: &Path) -> Result<PathBuf, FileSandboxError> {
        let canonical = fs::canonicalize(path).map_err(|source| FileSandboxError::Resolve {
            path: path.to_path_buf(),
            source,
        })?;
        if self.roots.iter().any(|root| canonical.starts_with(root)) {
            Ok(canonical)
        } else {
            Err(FileSandboxError::Outside {
                path: path.to_path_buf(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "stats-file-sandbox-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn accepts_files_under_any_allowed_root() {
        let first = test_dir("first");
        let second = test_dir("second");
        fs::write(first.join("one"), b"one").unwrap();
        fs::write(second.join("two"), b"two").unwrap();

        let sandbox = FileSandbox::new(vec![first.clone(), second.clone()]).unwrap();
        assert!(sandbox.check_existing(&first.join("one")).is_ok());
        assert!(sandbox.check_existing(&second.join("two")).is_ok());

        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
    }

    #[test]
    fn rejects_prefix_siblings() {
        let root = test_dir("root");
        let sibling = test_dir("root-sibling");
        fs::write(sibling.join("secret"), b"secret").unwrap();

        let sandbox = FileSandbox::new(vec![root.clone()]).unwrap();
        assert!(matches!(
            sandbox.check_existing(&sibling.join("secret")),
            Err(FileSandboxError::Outside { .. })
        ));

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(sibling).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn resolves_symlinks_before_checking_containment() {
        use std::os::unix::fs::symlink;

        let root = test_dir("symlink-root");
        let outside = test_dir("symlink-outside");
        fs::write(outside.join("secret"), b"secret").unwrap();
        symlink(outside.join("secret"), root.join("link")).unwrap();

        let sandbox = FileSandbox::new(vec![root.clone()]).unwrap();
        assert!(matches!(
            sandbox.check_existing(&root.join("link")),
            Err(FileSandboxError::Outside { .. })
        ));

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
