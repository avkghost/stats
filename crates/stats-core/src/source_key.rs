//! Stable identity of one active log family, used to scope checkpoint matching.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Derives the source key from the configured active `LogFile`.
///
/// The key is the canonical directory of the file joined with its configured
/// file name, so relative paths, `..` segments, and symlinked directories that
/// name the same log produce the same key. Rotated names are never passed in:
/// the configured name is the family base.
///
/// Call this with the path from the configuration *before* any command-line
/// override is applied, so every archive in a backfill maps to one source.
pub fn source_key(configured_log_file: &Path) -> io::Result<String> {
    let name = configured_log_file.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "LogFile must include a file name: {}",
                configured_log_file.display()
            ),
        )
    })?;
    let parent = match configured_log_file.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let canonical_parent = canonical_directory(parent)?;
    Ok(format!(
        "{}/{}",
        canonical_parent.display(),
        name.to_string_lossy()
    ))
}

fn canonical_directory(directory: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(directory).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "LogFile directory must exist to derive source identity: {}: {error}",
                directory.display()
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "stats-core-source-key-{}-{}",
            std::process::id(),
            name
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(directory.join("logs")).unwrap();
        directory
    }

    #[test]
    fn equivalent_spellings_of_one_file_share_a_key() {
        let directory = temp_directory("spellings");
        let plain = directory.join("logs").join("access.log");
        let dotted = directory.join("logs").join(".").join("access.log");
        let via_parent = directory
            .join("logs")
            .join("..")
            .join("logs")
            .join("access.log");

        let key = source_key(&plain).unwrap();
        assert_eq!(source_key(&dotted).unwrap(), key);
        assert_eq!(source_key(&via_parent).unwrap(), key);
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_directory_resolves_to_its_target() {
        let directory = temp_directory("symlink");
        let link = directory.join("link");
        std::os::unix::fs::symlink(directory.join("logs"), &link).unwrap();

        assert_eq!(
            source_key(&directory.join("logs").join("access.log")).unwrap(),
            source_key(&link.join("access.log")).unwrap()
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn different_file_names_in_one_directory_differ() {
        let directory = temp_directory("names");
        let access = source_key(&directory.join("logs").join("access.log")).unwrap();
        let other = source_key(&directory.join("logs").join("other.log")).unwrap();
        assert_ne!(access, other);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn missing_directory_is_an_error_not_a_key() {
        let directory = temp_directory("missing");
        let error = source_key(&directory.join("absent").join("access.log")).unwrap_err();
        assert!(error.to_string().contains("must exist"), "{error}");
        let _ = fs::remove_dir_all(directory);
    }
}
