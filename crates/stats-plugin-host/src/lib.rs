use libloading::Library;
use stats_plugin_api::{
    EventV1, HostApiV1, PluginApiV1, PluginInitV1, ABI_VERSION, CAPABILITY_ENRICH, INIT_SYMBOL,
};
use std::{
    ffi::{CStr, NulError},
    path::{Path, PathBuf},
    ptr::NonNull,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PluginLoadError {
    #[error("cannot load plugin {path}: {source}")]
    Open {
        path: PathBuf,
        source: libloading::Error,
    },
    #[error("plugin {path} has no {INIT_SYMBOL:?} symbol: {source}")]
    Symbol {
        path: PathBuf,
        source: libloading::Error,
    },
    #[error("plugin returned a null API pointer")]
    NullApi,
    #[error("plugin ABI version {actual} is unsupported; expected {expected}")]
    Abi { actual: u32, expected: u32 },
    #[error("plugin descriptor field {field} is null")]
    NullMetadata { field: &'static str },
    #[error("plugin {name} enrichment callback failed with status {code}")]
    Callback { name: String, code: i32 },
    #[error("plugin name or version is invalid UTF-8")]
    MetadataUtf8(#[from] std::str::Utf8Error),
    #[error("plugin metadata contains an interior NUL")]
    MetadataNul(#[from] NulError),
}

pub struct LoadedPlugin {
    _library: Library,
    api: NonNull<PluginApiV1>,
    name: String,
    version: String,
}

impl LoadedPlugin {
    /// Loads a trusted native plugin and validates its ABI before exposing it.
    ///
    /// Native plugins execute arbitrary code. Callers must only load configured,
    /// trusted paths and must keep the plugin alive until all calls complete.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `path` identifies a trusted plugin compiled
    /// against the compatible ABI, that `host` remains valid for the duration
    /// of plugin initialization, and that the loaded plugin does not outlive
    /// any data referenced by the host callbacks it receives.
    pub unsafe fn load(path: impl AsRef<Path>, host: &HostApiV1) -> Result<Self, PluginLoadError> {
        let path = path.as_ref().to_path_buf();
        let library = Library::new(&path).map_err(|source| PluginLoadError::Open {
            path: path.clone(),
            source,
        })?;
        let init: PluginInitV1 =
            *library
                .get(INIT_SYMBOL)
                .map_err(|source| PluginLoadError::Symbol {
                    path: path.clone(),
                    source,
                })?;
        let api = NonNull::new(init(host as *const HostApiV1)).ok_or(PluginLoadError::NullApi)?;
        let descriptor = (*api.as_ptr()).descriptor;
        if descriptor.abi_version != ABI_VERSION {
            return Err(PluginLoadError::Abi {
                actual: descriptor.abi_version,
                expected: ABI_VERSION,
            });
        }
        let name_ptr = NonNull::new(descriptor.name as *mut _)
            .ok_or(PluginLoadError::NullMetadata { field: "name" })?;
        let version_ptr = NonNull::new(descriptor.version as *mut _)
            .ok_or(PluginLoadError::NullMetadata { field: "version" })?;
        let name = CStr::from_ptr(name_ptr.as_ptr()).to_str()?.to_owned();
        let version = CStr::from_ptr(version_ptr.as_ptr()).to_str()?.to_owned();
        Ok(Self {
            _library: library,
            api,
            name,
            version,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn capabilities(&self) -> u64 {
        unsafe { self.api.as_ref().descriptor.capabilities }
    }

    pub fn enrich(&self, event: &mut EventV1) -> Result<(), PluginLoadError> {
        let api = unsafe { self.api.as_ref() };
        if api.descriptor.capabilities & CAPABILITY_ENRICH == 0 {
            return Ok(());
        }
        if let Some(enrich) = api.enrich {
            let code = unsafe { enrich(event as *mut EventV1) };
            if code != 0 {
                return Err(PluginLoadError::Callback {
                    name: self.name.clone(),
                    code,
                });
            }
        }
        Ok(())
    }
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        unsafe {
            if let Some(shutdown) = self.api.as_ref().shutdown {
                shutdown();
            }
        }
    }
}
