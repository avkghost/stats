use crate::PluginConfig;

/// Compatibility adapter for AWStats `hashfiles`.
///
/// AWStats uses Perl `Storable` hash files for internal persistence. Stats
/// stores the same authoritative state in SQLite, so the compatible backend
/// is explicit SQLite persistence rather than an incompatible `.hash` format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashFilesBackend {
    Sqlite,
}

pub fn hashfiles_backend(plugins: &[PluginConfig]) -> Option<HashFilesBackend> {
    plugins
        .iter()
        .any(|plugin| plugin.name == "hashfiles")
        .then_some(HashFilesBackend::Sqlite)
}

#[cfg(test)]
mod tests {
    use super::{hashfiles_backend, HashFilesBackend};
    use crate::PluginConfig;

    #[test]
    fn selects_sqlite_as_the_hashfiles_compatibility_backend() {
        let plugins = vec![PluginConfig {
            name: "hashfiles".into(),
            args: Vec::new(),
        }];
        assert_eq!(hashfiles_backend(&plugins), Some(HashFilesBackend::Sqlite));
    }
}
