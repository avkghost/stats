use stats_plugin_api::{HostApi, PluginApi, PluginDescriptor, ABI_VERSION};

static NAME: &[u8] = b"example\0";
static EMPTY: &[u8] = b"\0";

/// Minimal native extension template corresponding to AWStats `example.pm`.
///
/// Add a Rust callback to `PluginApi` when creating a real extension. This
/// example intentionally has no enrichment or renderer side effects.
///
/// # Safety
///
/// The host must keep the returned plugin API alive until the plugin is
/// unloaded.
#[no_mangle]
pub unsafe extern "C" fn stats_plugin_init(_host: *const HostApi) -> *mut PluginApi {
    Box::into_raw(Box::new(PluginApi {
        descriptor: PluginDescriptor {
            abi_version: ABI_VERSION,
            capabilities: 0,
            name: NAME.as_ptr().cast(),
            version: EMPTY.as_ptr().cast(),
        },
        enrich: None,
        render_report: None,
        release_report: None,
        shutdown: Some(shutdown),
    }))
}

unsafe extern "C" fn shutdown() {}
