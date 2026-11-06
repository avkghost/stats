use libloading::Library;
use stats_plugin_api::{
    HostApi, PluginApi, PluginEvent, PluginInit, RenderedReport, ReportRequest, ABI_VERSION,
    CAPABILITY_ENRICH, CAPABILITY_REPORT, INIT_SYMBOL,
};
use std::{
    ffi::{CStr, CString, NulError},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    ptr::NonNull,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;

pub const REPORT_WORKER_ARG: &str = "__stats_plugin_render_worker";
pub const WORKER_PROTOCOL_VERSION: u64 = 1;
pub const MAX_WORKER_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum PluginWorkerError {
    #[error("plugin worker I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("plugin worker protocol failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("plugin worker plugin operation failed: {0}")]
    Plugin(#[from] PluginLoadError),
    #[error("plugin worker request is missing {0}")]
    MissingField(&'static str),
    #[error("plugin worker request has unsupported operation {0:?}")]
    UnsupportedOperation(String),
    #[error("plugin worker request exceeds {MAX_WORKER_MESSAGE_BYTES} bytes")]
    MessageTooLarge,
}

#[derive(Debug, Error)]
pub enum WorkerRequestError {
    #[error("native report worker I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("native report worker protocol failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("native report worker message exceeds {MAX_WORKER_MESSAGE_BYTES} bytes")]
    MessageTooLarge,
    #[error("native report worker returned an invalid response: {0}")]
    InvalidResponse(String),
    #[error("native report worker timed out after {0:?}")]
    Timeout(Duration),
    #[error("native report worker exited unsuccessfully: {0}")]
    Exit(std::process::ExitStatus),
}

pub fn supervise_report_worker(
    executable: impl AsRef<Path>,
    plugin: impl AsRef<Path>,
    request: &serde_json::Value,
    timeout: Duration,
) -> Result<serde_json::Value, WorkerRequestError> {
    let request = serde_json::to_vec(request)?;
    if request.len() > MAX_WORKER_MESSAGE_BYTES {
        return Err(WorkerRequestError::MessageTooLarge);
    }
    let mut child = Command::new(executable.as_ref())
        .arg(REPORT_WORKER_ARG)
        .arg(plugin.as_ref())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("native report worker stdin is unavailable"))?;
    stdin.write_all(&request)?;
    stdin.write_all(b"\n")?;
    drop(stdin);

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("native report worker stdout is unavailable"))?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0_u8; 8192];
        let result = loop {
            match stdout.read(&mut buffer) {
                Ok(0) => break Ok(output),
                Ok(read) => {
                    if output.len().saturating_add(read) > MAX_WORKER_MESSAGE_BYTES {
                        break Err(WorkerRequestError::MessageTooLarge);
                    }
                    output.extend_from_slice(&buffer[..read]);
                }
                Err(error) => break Err(WorkerRequestError::Io(error)),
            }
        };
        let _ = sender.send(result);
    });

    let deadline = Instant::now() + timeout;
    let mut status = None;
    let mut output = None;
    while Instant::now() < deadline {
        if output.is_none() {
            if let Ok(result) = receiver.try_recv() {
                output = Some(result);
            }
        }
        if status.is_none() {
            status = child.try_wait()?;
        }
        if status.is_some() && output.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let status = match status {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(WorkerRequestError::Timeout(timeout));
        }
    };
    let output = match output {
        Some(output) => output,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(WorkerRequestError::Timeout(timeout));
        }
    }?;
    if !status.success() {
        return Err(WorkerRequestError::Exit(status));
    }
    let response: serde_json::Value = serde_json::from_slice(&output)?;
    let version = response
        .get("protocol_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| WorkerRequestError::InvalidResponse("missing protocol_version".into()))?;
    if version != WORKER_PROTOCOL_VERSION {
        return Err(WorkerRequestError::InvalidResponse(format!(
            "unsupported protocol_version {version}"
        )));
    }
    if !response.is_object() {
        return Err(WorkerRequestError::InvalidResponse(
            "response must be a JSON object".into(),
        ));
    }
    Ok(response)
}

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
        section: Option<&str>,
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
        let section = section.map(CString::new).transpose()?;
        let request = ReportRequest {
            format: format.as_ptr(),
            report_json: report_json.as_ptr(),
            section: section
                .as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr()),
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

/// Runs one isolated native report-plugin request over stdin/stdout.
///
/// Frontends enter this function only when launched with [`REPORT_WORKER_ARG`].
/// A parent process owns the supervision boundary; a plugin crash or abort ends
/// this worker without taking down the frontend.
pub fn run_report_worker(path: impl AsRef<Path>) -> Result<(), PluginWorkerError> {
    let host = HostApi {
        abi_version: ABI_VERSION,
        log: None,
    };
    let plugin = unsafe { LoadedPlugin::load(path.as_ref(), &host) }?;
    let mut request_text = String::new();
    io::stdin()
        .take((MAX_WORKER_MESSAGE_BYTES + 1) as u64)
        .read_to_string(&mut request_text)?;
    if request_text.len() > MAX_WORKER_MESSAGE_BYTES {
        return Err(PluginWorkerError::MessageTooLarge);
    }
    let request: serde_json::Value = serde_json::from_str(&request_text)?;
    let operation = request
        .get("operation")
        .and_then(serde_json::Value::as_str)
        .ok_or(PluginWorkerError::MissingField("operation"))?;
    let response = match operation {
        "metadata" => serde_json::json!({
            "protocol_version": WORKER_PROTOCOL_VERSION,
            "ok": true,
            "name": plugin.name(),
            "version": plugin.version(),
            "capabilities": plugin.capabilities(),
        }),
        "render" => {
            let format = request
                .get("format")
                .and_then(serde_json::Value::as_str)
                .ok_or(PluginWorkerError::MissingField("format"))?;
            let report_json = request
                .get("report_json")
                .and_then(serde_json::Value::as_str)
                .ok_or(PluginWorkerError::MissingField("report_json"))?;
            let section = request.get("section").and_then(serde_json::Value::as_str);
            match plugin.render_report(format, report_json, section) {
                Ok((media_type, extension, body)) => serde_json::json!({
                    "protocol_version": WORKER_PROTOCOL_VERSION,
                    "ok": true,
                    "media_type": media_type,
                    "extension": extension,
                    "body": body,
                }),
                Err(error) => serde_json::json!({
                    "protocol_version": WORKER_PROTOCOL_VERSION,
                    "ok": false,
                    "error": error.to_string(),
                }),
            }
        }
        other => return Err(PluginWorkerError::UnsupportedOperation(other.to_owned())),
    };
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    serde_json::to_writer(&mut stdout, &response)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

unsafe fn read_report_field(
    value: *const std::os::raw::c_char,
    field: &'static str,
) -> Result<String, PluginLoadError> {
    let value = NonNull::new(value as *mut _).ok_or(PluginLoadError::NullReportField { field })?;
    Ok(CStr::from_ptr(value.as_ptr()).to_str()?.to_owned())
}
