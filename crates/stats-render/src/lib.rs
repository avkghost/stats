mod csv;
mod html;
mod json;

use anyhow::anyhow;
use stats_core::{PluginRegistry, Report};

pub fn register_builtin_renderers(registry: &mut PluginRegistry) {
    registry
        .add_renderer(html::HtmlOverviewRenderer)
        .expect("built-in HTML renderer format must be unique");
    registry
        .add_renderer(json::JsonRenderer)
        .expect("built-in JSON renderer format must be unique");
    registry
        .add_renderer(csv::CsvRenderer)
        .expect("built-in CSV renderer format must be unique");
}

pub fn builtin_renderer_registry() -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    register_builtin_renderers(&mut registry);
    registry
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

pub use html::{write_html_site, write_html_site_with_periods};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_report_renderers_are_registered() {
        let registry = builtin_renderer_registry();
        let formats: Vec<_> = registry
            .renderers()
            .map(|renderer| renderer.format_name())
            .collect();
        assert_eq!(formats, ["html", "json", "csv"]);
        assert_eq!(
            registry.renderer("csv").unwrap().descriptor().name,
            "csv-report"
        );
        assert!(registry.renderer("unknown").is_none());
    }
}
