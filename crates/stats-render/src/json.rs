use stats_core::{PluginDescriptor, RenderedReport, Report, ReportRenderer};

pub(crate) struct JsonRenderer;

impl ReportRenderer for JsonRenderer {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            name: "json-report".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            capabilities: vec!["report".into(), "json".into()],
        }
    }

    fn format_name(&self) -> &str {
        "json"
    }

    fn render(&self, report: &Report) -> Result<RenderedReport, String> {
        serde_json::to_string_pretty(report)
            .map(|body| RenderedReport {
                media_type: "application/json; charset=utf-8".into(),
                extension: "json".into(),
                body,
            })
            .map_err(|error| error.to_string())
    }
}
