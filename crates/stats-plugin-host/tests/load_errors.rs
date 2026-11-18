use stats_plugin_api::HostApi;
use stats_plugin_host::{LoadedPlugin, PluginLoadError};

#[test]
fn missing_plugin_path_is_reported_as_open_error() {
    let host = HostApi {
        abi_version: stats_plugin_api::ABI_VERSION,
        log: None,
    };
    let result = unsafe { LoadedPlugin::load("/definitely/missing/stats-test-plugin.so", &host) };
    assert!(matches!(result, Err(PluginLoadError::Open { .. })));
}
