use std::os::raw::{c_char, c_int};

pub const ABI_VERSION: u32 = 1;
pub const INIT_SYMBOL: &[u8] = b"stats_plugin_init_v1\0";
pub const CAPABILITY_ENRICH: u64 = 1;
pub const CAPABILITY_REPORT: u64 = 1 << 1;
pub const CAPABILITY_OUTPUT: u64 = 1 << 2;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PluginDescriptorV1 {
    pub abi_version: u32,
    pub capabilities: u64,
    pub name: *const c_char,
    pub version: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HostApiV1 {
    pub abi_version: u32,
    pub log: Option<unsafe extern "C" fn(level: u32, message: *const c_char)>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EventV1 {
    pub client_ip: *const c_char,
    pub method: *const c_char,
    pub url: *const c_char,
    pub status: u16,
    pub bytes: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PluginApiV1 {
    pub descriptor: PluginDescriptorV1,
    pub enrich: Option<unsafe extern "C" fn(event: *mut EventV1) -> c_int>,
    pub shutdown: Option<unsafe extern "C" fn()>,
}

pub type PluginInitV1 = unsafe extern "C" fn(host: *const HostApiV1) -> *mut PluginApiV1;
