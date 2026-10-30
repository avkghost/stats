use anyhow::Context;
use maxminddb::{geoip2, Reader};
use serde::Serialize;
use std::{
    net::IpAddr,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Default, Serialize)]
pub struct GeoInfo {
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub asn: Option<u32>,
    pub organization: Option<String>,
}

pub trait GeoIp: Send + Sync {
    fn lookup(&self, ip: &str) -> GeoInfo;
}

pub struct MaxMindGeoIp {
    country: Option<Reader<Vec<u8>>>,
    city: Option<Reader<Vec<u8>>>,
    asn: Option<Reader<Vec<u8>>>,
    #[allow(dead_code)]
    paths: Vec<PathBuf>,
}

impl MaxMindGeoIp {
    pub fn open(
        country: Option<&Path>,
        city: Option<&Path>,
        asn: Option<&Path>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            country: country
                .map(|path| {
                    Reader::open_readfile(path)
                        .with_context(|| format!("open GeoIP Country database {}", path.display()))
                })
                .transpose()?,
            city: city
                .map(|path| {
                    Reader::open_readfile(path)
                        .with_context(|| format!("open GeoIP City database {}", path.display()))
                })
                .transpose()?,
            asn: asn
                .map(|path| {
                    Reader::open_readfile(path)
                        .with_context(|| format!("open GeoIP ASN database {}", path.display()))
                })
                .transpose()?,
            paths: [country, city, asn]
                .into_iter()
                .flatten()
                .map(Path::to_path_buf)
                .collect(),
        })
    }
}

impl GeoIp for MaxMindGeoIp {
    fn lookup(&self, ip: &str) -> GeoInfo {
        let normalized = ip.split_once('%').map_or(ip, |(address, _)| address);
        let Ok(ip) = normalized.parse::<IpAddr>() else {
            return GeoInfo::default();
        };
        if !is_lookup_candidate(ip) {
            return GeoInfo::default();
        }
        let mut info = GeoInfo::default();
        if let Some(reader) = &self.city {
            if let Ok(Some(city)) = reader.lookup(ip).and_then(|r| r.decode::<geoip2::City>()) {
                info.country = city.country.iso_code.map(str::to_owned);
                info.region = city
                    .subdivisions
                    .first()
                    .and_then(|r| r.iso_code)
                    .map(str::to_owned);
                info.city = city.city.names.english.map(str::to_owned);
                info.latitude = city.location.latitude;
                info.longitude = city.location.longitude;
            }
        }
        if info.country.is_none() {
            if let Some(reader) = &self.country {
                if let Ok(Some(country)) = reader
                    .lookup(ip)
                    .and_then(|r| r.decode::<geoip2::Country>())
                {
                    info.country = country.country.iso_code.map(str::to_owned);
                }
            }
        }
        if let Some(reader) = &self.asn {
            if let Ok(Some(asn)) = reader.lookup(ip).and_then(|r| r.decode::<geoip2::Asn>()) {
                info.asn = asn.autonomous_system_number;
                info.organization = asn.autonomous_system_organization.map(str::to_owned);
            }
        }
        info
    }
}

fn is_lookup_candidate(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_unspecified()
                && !ip.is_link_local()
                && !ip.is_multicast()
        }
        IpAddr::V6(ip) => {
            !ip.is_loopback()
                && !ip.is_unspecified()
                && !ip.is_unicast_link_local()
                && !ip.is_multicast()
                && !ip.is_unique_local()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_non_routable_and_scope_suffixed_addresses() {
        let geoip = MaxMindGeoIp::open(None, None, None).unwrap();
        assert!(geoip.lookup("127.0.0.1").country.is_none());
        assert!(geoip.lookup("192.168.1.1").country.is_none());
        assert!(geoip.lookup("fe80::1%eth0").country.is_none());
        assert!(geoip.lookup("::1").country.is_none());
    }
}
