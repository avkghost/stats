use serde_json::{json, Value};
use stats_core::{
    Config, PluginConfig, PluginDescriptor, PluginRegistry, RenderedReport, RendererRegistryError,
    Report, ReportRenderer,
};
use stats_plugin_host::supervise_report_worker;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub struct NativeReportRenderer {
    library: PathBuf,
    name: String,
    version: String,
    format: String,
}

impl NativeReportRenderer {
    pub fn load(path: impl AsRef<Path>, format: impl Into<String>) -> anyhow::Result<Self> {
        let library = path.as_ref().to_path_buf();
        let metadata = run_worker(&library, json!({"operation": "metadata"})).map_err(|error| {
            anyhow::anyhow!("load report renderer {}: {error}", library.display())
        })?;
        Ok(Self {
            name: response_string(&metadata, "name")?,
            version: response_string(&metadata, "version")?,
            library,
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
            name: self.name.clone(),
            version: self.version.clone(),
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
        let mut request = json!({
            "operation": "render",
            "format": self.format,
            "report_json": input,
        });
        if let Some(section) = section {
            request["section"] = Value::String(section.to_owned());
        }
        let response = run_worker(&self.library, request).map_err(|error| error.to_string())?;
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("native report renderer failed")
                .to_owned());
        }
        Ok(RenderedReport {
            media_type: response_string(&response, "media_type")
                .map_err(|error| error.to_string())?,
            extension: response_string(&response, "extension")
                .map_err(|error| error.to_string())?,
            body: response_string(&response, "body").map_err(|error| error.to_string())?,
        })
    }
}

const WORKER_TIMEOUT: Duration = Duration::from_secs(10);

fn response_string(response: &Value, field: &str) -> anyhow::Result<String> {
    response
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("native report worker response is missing {field}"))
}

fn run_worker(path: &Path, request: Value) -> anyhow::Result<Value> {
    Ok(supervise_report_worker(
        std::env::current_exe()?,
        path,
        &request,
        WORKER_TIMEOUT,
    )?)
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
        for name in [
            "html",
            "report_xml",
            "geoip2_city",
            "graphapplet",
            "graphgooglechartapi",
            "tooltips",
        ] {
            let plugin = PluginConfig {
                name: name.into(),
                args: Vec::new(),
            };
            assert_eq!(report_plugin_format(&plugin), None);
        }
    }
}
