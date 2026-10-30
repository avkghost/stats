use std::os::raw::{c_char, c_int};

pub const ABI_VERSION: u32 = 1;
pub const INIT_SYMBOL: &[u8] = b"stats_plugin_init\0";
pub const CAPABILITY_ENRICH: u64 = 1;
pub const CAPABILITY_REPORT: u64 = 1 << 1;
pub const CAPABILITY_OUTPUT: u64 = 1 << 2;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PluginDescriptor {
    pub abi_version: u32,
    pub capabilities: u64,
    pub name: *const c_char,
    pub version: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HostApi {
    pub abi_version: u32,
    pub log: Option<unsafe extern "C" fn(level: u32, message: *const c_char)>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PluginEvent {
    pub client_ip: *const c_char,
    pub method: *const c_char,
    pub url: *const c_char,
    pub status: u16,
    pub bytes: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PluginApi {
    pub descriptor: PluginDescriptor,
    pub enrich: Option<unsafe extern "C" fn(event: *mut PluginEvent) -> c_int>,
    pub render_report: Option<RenderReport>,
    pub release_report: Option<ReleaseReport>,
    pub shutdown: Option<unsafe extern "C" fn()>,
}

pub type PluginInit = unsafe extern "C" fn(host: *const HostApi) -> *mut PluginApi;

/// UTF-8 JSON report input supplied to a native report renderer.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ReportRequest {
    pub format: *const c_char,
    pub report_json: *const c_char,
}

/// UTF-8 strings returned by a native report renderer.
///
/// The plugin owns these pointers and must release them from `release_report`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RenderedReport {
    pub media_type: *const c_char,
    pub extension: *const c_char,
    pub body: *const c_char,
}

pub type RenderReport =
    unsafe extern "C" fn(request: *const ReportRequest, output: *mut RenderedReport) -> c_int;
pub type ReleaseReport = unsafe extern "C" fn(output: *mut RenderedReport);
