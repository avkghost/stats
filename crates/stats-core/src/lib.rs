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
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let (value, unit, decimals) = scale_for_display(bytes as f64, 1024.0, UNITS.len() - 1);
    format!("{value:.decimals$} {}", UNITS[unit])
}

/// Formats a non-byte count using compact decimal units for human-facing output.
pub fn format_count(count: u64) -> String {
    const UNITS: [&str; 5] = ["", "K", "M", "B", "T"];
    if count < 1000 {
        return count.to_string();
    }
    let (value, unit, decimals) = scale_for_display(count as f64, 1000.0, UNITS.len() - 1);
    format!("{value:.decimals$}{}", UNITS[unit])
}

/// Chooses the unit and decimal places for a value of at least one base unit.
///
/// The value is rounded at the precision it will be displayed with, and the unit is chosen
/// from that rounded value. Rounding first prevents "1024 KB" or "1000K" when the scaled value
/// is just below the next unit.
fn scale_for_display(raw: f64, base: f64, max_unit: usize) -> (f64, usize, usize) {
    let mut unit = 0;
    let mut value = raw;
    while value >= base && unit < max_unit {
        value /= base;
        unit += 1;
    }
    loop {
        let decimals = if value >= 100.0 {
            0
        } else if value >= 10.0 {
            1
        } else {
            2
        };
        let factor = 10f64.powi(decimals as i32);
        let rounded = (value * factor).round() / factor;
        if rounded >= base && unit < max_unit {
            value = rounded / base;
            unit += 1;
        } else {
            return (rounded, unit, decimals);
        }
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
    UpdateStats, DEFAULT_CARDINALITY_LIMIT, PREPARED_STATEMENT_CACHE_CAPACITY,
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
        assert_eq!(format_bytes(1_048_575), "1.00 MB");
        assert_eq!(format_bytes(1_048_051), "1023 KB");
        assert_eq!(format_bytes(1_073_741_823), "1.00 GB");
    }

    #[test]
    fn formats_human_readable_counts() {
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1.00K");
        assert_eq!(format_count(12_500), "12.5K");
        assert_eq!(format_count(1_000_000), "1.00M");
        assert_eq!(format_count(2_000_000_000), "2.00B");
        assert_eq!(format_count(999_999), "1.00M");
        assert_eq!(format_count(999_950), "1.00M");
        assert_eq!(format_count(999_400), "999K");
        assert_eq!(format_count(9_996), "10.00K");
    }
}
