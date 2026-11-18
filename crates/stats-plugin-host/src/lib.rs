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
    #[error("native report worker request writer panicked")]
    WriterPanicked,
}

/// Kills the worker's whole process group on Unix, then the worker itself, and reaps it.
/// On other platforms only the direct worker is killed; descendants are not reached.
fn terminate_worker(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // SAFETY: killpg only sends a signal. The leader is not reaped at this point unless
        // `try_wait` already collected it, and failure (for example ESRCH) is ignored.
        unsafe {
            libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

enum WorkerFinish {
    Exited(
        std::process::ExitStatus,
        Result<Vec<u8>, WorkerRequestError>,
    ),
    TimedOut,
}

/// Waits until the worker has exited and its response has been read, or the deadline passes.
fn await_worker(
    child: &mut std::process::Child,
    output: &mpsc::Receiver<Result<Vec<u8>, WorkerRequestError>>,
    deadline: Instant,
) -> io::Result<WorkerFinish> {
    let mut status = None;
    let mut response = None;
    loop {
        if response.is_none() {
            if let Ok(result) = output.try_recv() {
                response = Some(result);
            }
        }
        if status.is_none() {
            status = child.try_wait()?;
        }
        if let (Some(_), Some(_)) = (&status, &response) {
            break;
        }
        if Instant::now() >= deadline {
            return Ok(WorkerFinish::TimedOut);
        }
        thread::sleep(Duration::from_millis(5));
    }
    match (status, response) {
        (Some(status), Some(response)) => Ok(WorkerFinish::Exited(status, response)),
        _ => Ok(WorkerFinish::TimedOut),
    }
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
    let deadline = Instant::now() + timeout;
    let mut command = Command::new(executable.as_ref());
    command
        .arg(plugin.as_ref())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // The worker leads its own process group, so termination reaches descendants too.
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("native report worker stdin is unavailable"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("native report worker stdout is unavailable"))?;

    // The request is written on its own thread, so a worker that never reads stdin cannot hold
    // the supervisor past its deadline. The handle is joined below on every path.
    let writer = thread::spawn(move || {
        let mut stdin = stdin;
        let result = stdin
            .write_all(&request)
            .and_then(|()| stdin.write_all(b"\n"));
        drop(stdin);
        result
    });

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

    let finish = await_worker(&mut child, &receiver, deadline);
    // A worker that exited successfully has already read its stdin to EOF, so the writer is
    // finished. Any other outcome may leave a descendant holding stdin open, so the whole group
    // is terminated before the writer is joined.
    let settled = matches!(&finish, Ok(WorkerFinish::Exited(status, _)) if status.success());
    if !settled {
        terminate_worker(&mut child);
    }
    // Every path reaches this join after the worker and its group are gone, so the pipe's read
    // ends are closed and a blocked write has returned.
    let written = writer.join();
    let (status, response) = match finish? {
        WorkerFinish::TimedOut => {
            return Err(match written {
                Err(_) => WorkerRequestError::WriterPanicked,
                Ok(_) => WorkerRequestError::Timeout(timeout),
            });
        }
        WorkerFinish::Exited(status, response) => (status, response),
    };
    match written {
        Err(_) => return Err(WorkerRequestError::WriterPanicked),
        Ok(Err(error)) if error.kind() != io::ErrorKind::BrokenPipe => {
            return Err(WorkerRequestError::Io(error));
        }
        // A broken pipe means the worker exited before reading; its exit status explains why.
        Ok(_) => {}
    }
    let output = response?;
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
/// Only the `stats-report-worker` binary calls this function.
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
