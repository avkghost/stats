use crate::{AccessEvent, GeoIp, Report};

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

pub trait ReportRenderer: Send + Sync {
    fn descriptor(&self) -> PluginDescriptor;
    fn format_name(&self) -> &str;
    fn render(&self, report: &Report) -> Result<RenderedReport, String>;
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

    pub fn add_renderer<R: ReportRenderer + 'static>(&mut self, renderer: R) {
        self.report_renderers.push(Box::new(renderer));
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
