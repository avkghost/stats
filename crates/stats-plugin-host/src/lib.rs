use libloading::Library;
use stats_plugin_api::{
    HostApi, PluginApi, PluginEvent, PluginInit, RenderedReport, ReportRequest, ABI_VERSION,
    CAPABILITY_ENRICH, CAPABILITY_REPORT, INIT_SYMBOL,
};
use std::{
    ffi::{CStr, CString, NulError},
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
    #[error("plugin {name} does not provide a report renderer")]
    NoReportRenderer { name: String },
    #[error("plugin {name} report renderer failed with status {code}")]
    ReportCallback { name: String, code: i32 },
    #[error("plugin report renderer returned a null {field}")]
    NullReportField { field: &'static str },
    #[error("plugin name or version is invalid UTF-8")]
    MetadataUtf8(#[from] std::str::Utf8Error),
    #[error("plugin metadata contains an interior NUL")]
    MetadataNul(#[from] NulError),
}

pub struct LoadedPlugin {
    _library: Library,
    api: LoadedApi,
    name: String,
    version: String,
}

enum LoadedApi {
    Plugin(NonNull<PluginApi>),
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
    pub unsafe fn load(path: impl AsRef<Path>, host: &HostApi) -> Result<Self, PluginLoadError> {
        let path = path.as_ref().to_path_buf();
        let library = Library::new(&path).map_err(|source| PluginLoadError::Open {
            path: path.clone(),
            source,
        })?;
        let init: PluginInit =
            *library
                .get(INIT_SYMBOL)
                .map_err(|source| PluginLoadError::Symbol {
                    path: path.clone(),
                    source,
                })?;
        let api = NonNull::new(init(host as *const HostApi)).ok_or(PluginLoadError::NullApi)?;
        let descriptor = (*api.as_ptr()).descriptor;
        if descriptor.abi_version != ABI_VERSION {
            return Err(PluginLoadError::Abi {
                actual: descriptor.abi_version,
                expected: ABI_VERSION,
            });
        };
        let api = LoadedApi::Plugin(api);
        let descriptor = match api {
            LoadedApi::Plugin(api) => (*api.as_ptr()).descriptor,
        };
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
        unsafe {
            match self.api {
                LoadedApi::Plugin(api) => api.as_ref().descriptor.capabilities,
            }
        }
    }

    pub fn enrich(&self, event: &mut PluginEvent) -> Result<(), PluginLoadError> {
        unsafe {
            let (capabilities, enrich) = match self.api {
                LoadedApi::Plugin(api) => {
                    (api.as_ref().descriptor.capabilities, api.as_ref().enrich)
                }
            };
            if capabilities & CAPABILITY_ENRICH == 0 {
                return Ok(());
            }
            if let Some(enrich) = enrich {
                let code = enrich(event as *mut PluginEvent);
                if code != 0 {
                    return Err(PluginLoadError::Callback {
                        name: self.name.clone(),
                        code,
                    });
                }
            }
        }
        Ok(())
    }

    pub fn render_report(
        &self,
        format: &str,
        report_json: &str,
    ) -> Result<(String, String, String), PluginLoadError> {
        let LoadedApi::Plugin(api) = self.api;
        let api = unsafe { api.as_ref() };
        if api.descriptor.capabilities & CAPABILITY_REPORT == 0 {
            return Err(PluginLoadError::NoReportRenderer {
                name: self.name.clone(),
            });
        }
        let render = api
            .render_report
            .ok_or_else(|| PluginLoadError::NoReportRenderer {
                name: self.name.clone(),
            })?;
        let release = api
            .release_report
            .ok_or_else(|| PluginLoadError::NoReportRenderer {
                name: self.name.clone(),
            })?;
        let format = CString::new(format)?;
        let report_json = CString::new(report_json)?;
        let request = ReportRequest {
            format: format.as_ptr(),
            report_json: report_json.as_ptr(),
        };
        let mut output = RenderedReport {
            media_type: std::ptr::null(),
            extension: std::ptr::null(),
            body: std::ptr::null(),
        };
        let code = unsafe { render(&request, &mut output) };
        if code != 0 {
            unsafe { release(&mut output) };
            return Err(PluginLoadError::ReportCallback {
                name: self.name.clone(),
                code,
            });
        }
        let result = unsafe {
            let media_type = read_report_field(output.media_type, "media_type")?;
            let extension = read_report_field(output.extension, "extension")?;
            let body = read_report_field(output.body, "body")?;
            Ok((media_type, extension, body))
        };
        unsafe { release(&mut output) };
        result
    }
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        unsafe {
            let shutdown = match self.api {
                LoadedApi::Plugin(api) => api.as_ref().shutdown,
            };
            if let Some(shutdown) = shutdown {
                shutdown();
            }
        }
    }
}

unsafe fn read_report_field(
    value: *const std::os::raw::c_char,
    field: &'static str,
) -> Result<String, PluginLoadError> {
    let value = NonNull::new(value as *mut _).ok_or(PluginLoadError::NullReportField { field })?;
    Ok(CStr::from_ptr(value.as_ptr()).to_str()?.to_owned())
}
