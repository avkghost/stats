use crate::{AccessEvent, GeoIp};

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

pub struct PluginRegistry {
    enrichers: Vec<Box<dyn Enricher>>,
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
