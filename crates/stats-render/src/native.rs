use stats_core::{
    Config, PluginConfig, PluginDescriptor, PluginRegistry, RenderedReport, RendererRegistryError,
    Report, ReportRenderer,
};
use stats_plugin_api::{HostApi, ABI_VERSION};
use stats_plugin_host::LoadedPlugin;
use std::path::Path;

pub struct NativeReportRenderer {
    plugin: LoadedPlugin,
    format: String,
}

impl NativeReportRenderer {
    pub fn load(path: impl AsRef<Path>, format: impl Into<String>) -> anyhow::Result<Self> {
        let host = HostApi {
            abi_version: ABI_VERSION,
            log: None,
        };
        let plugin = unsafe { LoadedPlugin::load(path.as_ref(), &host) }.map_err(|error| {
            anyhow::anyhow!("load report renderer {}: {error}", path.as_ref().display())
        })?;
        Ok(Self {
            plugin,
            format: format.into(),
        })
    }
}

pub fn register_configured_renderer(
    registry: &mut PluginRegistry,
    config: &Config,
    format: &str,
) -> anyhow::Result<()> {
    let library = configured_renderer_library(config, format)?;
    let renderer = NativeReportRenderer::load(library, format)?;
    let result = match format {
        "html" => registry.replace_renderer(renderer),
        _ => registry.add_renderer(renderer),
    };
    result.map_err(|error: RendererRegistryError| {
        anyhow::anyhow!("register external report renderer: {error}")
    })?;
    Ok(())
}

fn configured_renderer_library<'a>(config: &'a Config, format: &str) -> anyhow::Result<&'a Path> {
    let plugin = config
        .plugins
        .iter()
        .find(|plugin| report_plugin_format(plugin).as_deref() == Some(format))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no external report renderer for {format} is configured (expected LoadPlugin=report_{format} <library>)"
            )
        })?;
    let library = plugin.args.last().ok_or_else(|| {
        anyhow::anyhow!(
            "external report renderer {} has no library path",
            plugin.name
        )
    })?;
    Ok(Path::new(library))
}

pub fn report_plugin_format(plugin: &PluginConfig) -> Option<String> {
    match plugin.name.as_str() {
        "export_to_csv" => Some("csv".into()),
        name => name
            .strip_prefix("report_")
            .or_else(|| name.strip_prefix("renderer_"))
            .filter(|format| matches!(*format, "html" | "json" | "csv"))
            .map(str::to_owned),
    }
}

impl ReportRenderer for NativeReportRenderer {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            name: self.plugin.name().to_owned(),
            version: self.plugin.version().to_owned(),
            capabilities: vec!["report".into(), self.format.clone()],
        }
    }

    fn format_name(&self) -> &str {
        &self.format
    }

    fn render(&self, report: &Report) -> Result<RenderedReport, String> {
        self.render_with_section(report, None)
    }

    fn render_section(&self, report: &Report, section: &str) -> Result<RenderedReport, String> {
        self.render_with_section(report, Some(section))
    }
}

impl NativeReportRenderer {
    fn render_with_section(
        &self,
        report: &Report,
        section: Option<&str>,
    ) -> Result<RenderedReport, String> {
        let input = serde_json::to_string(report).map_err(|error| error.to_string())?;
        let (media_type, extension, body) = self
            .plugin
            .render_report(&self.format, &input, section)
            .map_err(|error| error.to_string())?;
        Ok(RenderedReport {
            media_type,
            extension,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::report_plugin_format;
    use stats_core::PluginConfig;

    #[test]
    fn recognizes_external_awstats_report_formats() {
        for (name, expected) in [
            ("report_html", "html"),
            ("report_json", "json"),
            ("report_csv", "csv"),
            ("renderer_html", "html"),
            ("renderer_json", "json"),
            ("renderer_csv", "csv"),
            ("export_to_csv", "csv"),
        ] {
            let plugin = PluginConfig {
                name: name.into(),
                args: vec!["libreport.so".into()],
            };
            assert_eq!(report_plugin_format(&plugin).as_deref(), Some(expected));
        }
    }

    #[test]
    fn ignores_non_report_plugins_and_unknown_formats() {
        for name in ["html", "report_xml", "geoip2_city", "tooltips"] {
            let plugin = PluginConfig {
                name: name.into(),
                args: Vec::new(),
            };
            assert_eq!(report_plugin_format(&plugin), None);
        }
    }
}
