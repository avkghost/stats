mod checkpoint_match;
mod config;
mod filesystem;
mod filter;
mod geoip;
mod hashfiles;
mod method_anomaly;
mod parser;
mod plugin;
mod prefix_hash;
mod raw_log;
mod referer_spam;
mod report;
mod scanner;
mod source_key;
mod store;

/// Formats a byte count using binary units for human-facing output.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// Formats a non-byte count using compact decimal units for human-facing output.
pub fn format_count(count: u64) -> String {
    const UNITS: [&str; 5] = ["", "K", "M", "B", "T"];
    let mut value = count as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        count.to_string()
    } else if value >= 100.0 {
        format!("{value:.0}{}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1}{}", UNITS[unit])
    } else {
        format!("{value:.2}{}", UNITS[unit])
    }
}

pub use checkpoint_match::{decide, AbortReason, Decision, StateKind, StateRef};
pub use config::{
    Config, ConfigError, DatabasePeriod, FilterConfig, PluginConfig, AWSTATS_PLUGIN_INVENTORY,
};
pub use filesystem::{FileSandbox, FileSandboxError};
pub use filter::{accepts, is_page};
pub use geoip::{GeoInfo, GeoIp, MaxMindGeoIp};
pub use hashfiles::{hashfiles_backend, HashFilesBackend};
pub use method_anomaly::MethodAnomalyMatcher;
pub use parser::{AccessEvent, AccessTimestamp, LogParser, ParseError, ParseOutcome};
pub use plugin::{
    Enricher, GeoIpEnricher, PluginDescriptor, PluginRegistry, RenderedReport,
    RendererRegistryError, ReportRenderer,
};
pub use prefix_hash::{prefix_sha256, PrefixDigest};
pub use raw_log::{
    bearer_token_matches, render_redacted as render_redacted_raw_log, MAX_RAW_LOG_LINES,
};
pub use referer_spam::{RefererSpamMatcher, RefererSpamRuleError};
pub use report::{
    BandwidthRow, DailyRow, NotFoundRow, Report, ReportMetadata, ReportRow, ReportService,
};
pub use scanner::{ScannerMatcher, ScannerRuleError};
pub use source_key::source_key;
pub use store::{
    Batch, DailyAggregate, InputCheckpoint, NotFoundAggregate, PrefixState, StatsStore, StoreError,
    UpdateStats,
};

#[cfg(test)]
mod tests {
    use super::{format_bytes, format_count};

    #[test]
    fn formats_human_readable_byte_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.00 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(format_bytes(1_048_575), "1024 KB");
    }

    #[test]
    fn formats_human_readable_counts() {
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1.00K");
        assert_eq!(format_count(12_500), "12.5K");
        assert_eq!(format_count(1_000_000), "1.00M");
        assert_eq!(format_count(2_000_000_000), "2.00B");
        assert_eq!(format_count(999_999), "1000K");
    }
}
