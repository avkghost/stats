mod html;
mod native;

use anyhow::anyhow;
use stats_core::{PluginRegistry, RendererRegistryError, Report};

pub fn register_builtin_renderers(
    _registry: &mut PluginRegistry,
) -> Result<(), RendererRegistryError> {
    Ok(())
}

pub fn builtin_renderer_registry() -> Result<PluginRegistry, RendererRegistryError> {
    let mut registry = PluginRegistry::new();
    register_builtin_renderers(&mut registry)?;
    Ok(registry)
}

pub fn render_report_stream(
    registry: &PluginRegistry,
    report: &Report,
    format: &str,
) -> anyhow::Result<String> {
    let renderer = registry
        .renderer(format)
        .ok_or_else(|| anyhow!("no report renderer registered for format {format}"))?;
    renderer
        .render(report)
        .map(|rendered| rendered.body)
        .map_err(|error| anyhow!("render {format} report: {error}"))
}

pub use html::{
    render_html_report, render_html_section, write_html_site, write_html_site_with_periods,
};
pub use native::{register_configured_renderer, report_plugin_format, NativeReportRenderer};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_report_renderers_are_registered() {
        let registry = builtin_renderer_registry().unwrap();
        let formats: Vec<_> = registry
            .renderers()
            .map(|renderer| renderer.format_name())
            .collect();
        assert!(formats.is_empty());
        assert!(registry.renderer("unknown").is_none());
    }
}
