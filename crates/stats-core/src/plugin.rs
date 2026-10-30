use crate::{AccessEvent, GeoIp, Report};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct PluginDescriptor {
    pub name: String,
    pub version: String,
    pub capabilities: Vec<String>,
}

pub trait Enricher: Send + Sync {
    fn descriptor(&self) -> PluginDescriptor;
    fn enrich(&self, event: &mut AccessEvent);
}

#[derive(Debug, Clone)]
pub struct RenderedReport {
    pub media_type: String,
    pub extension: String,
    pub body: String,
}

pub trait ReportRenderer {
    fn descriptor(&self) -> PluginDescriptor;
    fn format_name(&self) -> &str;
    fn render(&self, report: &Report) -> Result<RenderedReport, String>;
    fn render_section(&self, report: &Report, section: &str) -> Result<RenderedReport, String> {
        if section == "overview" {
            self.render(report)
        } else {
            Err(format!(
                "renderer {} does not support report sections",
                self.format_name()
            ))
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RendererRegistryError {
    #[error("report renderer format {format:?} is already registered")]
    DuplicateFormat { format: String },
}

pub struct PluginRegistry {
    enrichers: Vec<Box<dyn Enricher>>,
    report_renderers: Vec<Box<dyn ReportRenderer>>,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            enrichers: Vec::new(),
            report_renderers: Vec::new(),
        }
    }
    pub fn add<E: Enricher + 'static>(&mut self, enricher: E) {
        self.enrichers.push(Box::new(enricher));
    }
    pub fn enrich(&self, event: &mut AccessEvent) {
        for enricher in &self.enrichers {
            enricher.enrich(event);
        }
    }

    pub fn add_renderer<R: ReportRenderer + 'static>(
        &mut self,
        renderer: R,
    ) -> Result<(), RendererRegistryError> {
        let format = renderer.format_name().to_owned();
        if self.renderer(&format).is_some() {
            return Err(RendererRegistryError::DuplicateFormat { format });
        }
        self.report_renderers.push(Box::new(renderer));
        Ok(())
    }

    pub fn replace_renderer<R: ReportRenderer + 'static>(
        &mut self,
        renderer: R,
    ) -> Result<(), RendererRegistryError> {
        let format = renderer.format_name().to_owned();
        if let Some(index) = self
            .report_renderers
            .iter()
            .position(|existing| existing.format_name() == format)
        {
            self.report_renderers.remove(index);
        }
        self.report_renderers.push(Box::new(renderer));
        Ok(())
    }

    pub fn renderer(&self, format: &str) -> Option<&dyn ReportRenderer> {
        self.report_renderers
            .iter()
            .find(|renderer| renderer.format_name() == format)
            .map(Box::as_ref)
    }

    pub fn renderers(&self) -> impl Iterator<Item = &dyn ReportRenderer> {
        self.report_renderers.iter().map(Box::as_ref)
    }
}

pub struct GeoIpEnricher {
    pub provider: Box<dyn GeoIp>,
}
impl Enricher for GeoIpEnricher {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            name: "geoip".into(),
            version: "0.1".into(),
            capabilities: vec!["country".into(), "city".into(), "asn".into()],
        }
    }
    fn enrich(&self, event: &mut AccessEvent) {
        event.geo = self.provider.lookup(&event.client_ip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestRenderer(&'static str);

    impl ReportRenderer for TestRenderer {
        fn descriptor(&self) -> PluginDescriptor {
            PluginDescriptor {
                name: self.0.into(),
                version: "test".into(),
                capabilities: vec!["report".into()],
            }
        }

        fn format_name(&self) -> &str {
            self.0
        }

        fn render(&self, _report: &Report) -> Result<RenderedReport, String> {
            Ok(RenderedReport {
                media_type: "text/plain".into(),
                extension: "txt".into(),
                body: String::new(),
            })
        }
    }

    #[test]
    fn rejects_duplicate_renderer_formats() {
        let mut registry = PluginRegistry::new();
        registry.add_renderer(TestRenderer("text")).unwrap();
        assert_eq!(
            registry.add_renderer(TestRenderer("text")),
            Err(RendererRegistryError::DuplicateFormat {
                format: "text".into()
            })
        );
    }

    #[test]
    fn replaces_renderer_for_configured_overrides() {
        let mut registry = PluginRegistry::new();
        registry.add_renderer(TestRenderer("html")).unwrap();
        registry.replace_renderer(TestRenderer("html")).unwrap();
        assert_eq!(registry.renderers().count(), 1);
        assert_eq!(registry.renderer("html").unwrap().format_name(), "html");
    }
}
