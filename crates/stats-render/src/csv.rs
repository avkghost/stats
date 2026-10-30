use stats_core::{PluginDescriptor, RenderedReport, Report, ReportRenderer};

pub(crate) struct CsvRenderer;

impl ReportRenderer for CsvRenderer {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            name: "csv-report".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            capabilities: vec!["report".into(), "csv".into()],
        }
    }

    fn format_name(&self) -> &str {
        "csv"
    }

    fn render(&self, report: &Report) -> Result<RenderedReport, String> {
        Ok(RenderedReport {
            media_type: "text/csv; charset=utf-8".into(),
            extension: "csv".into(),
            body: render_csv_report(report),
        })
    }
}

pub(crate) fn csv_field(value: &str) -> String {
    if value
        .chars()
        .any(|character| matches!(character, ',' | '"' | '\n' | '\r'))
    {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

pub(crate) fn render_csv_report(report: &Report) -> String {
    let mut out = String::from("section,key,hits,bytes,pages,files,visits\n");
    out.push_str(&format!("summary,hits,{},,,,\n", report.hits));
    out.push_str(&format!("summary,bytes,,{},,,\n", report.bytes));
    out.push_str(&format!("summary,pages,{},,,,\n", report.pages));
    out.push_str(&format!("summary,files,{},,,,\n", report.files));
    out.push_str(&format!("summary,visits,{},,,,\n", report.visits));
    out.push_str(&format!(
        "summary,unique_visits,{},,,,\n",
        report.unique_visits
    ));
    for (section, rows) in [
        ("countries", &report.countries),
        ("regions", &report.regions),
        ("cities", &report.cities),
        ("asns", &report.asns),
        ("organizations", &report.organizations),
        ("statuses", &report.statuses),
        ("status_classes", &report.status_classes),
        ("errors", &report.errors),
        ("methods", &report.methods),
        ("urls", &report.urls),
        ("users", &report.users),
        ("user_agents", &report.user_agents),
        ("referers", &report.referers),
        ("referrer_domains", &report.referrer_domains),
        ("file_types", &report.file_types),
        ("browsers", &report.browsers),
        ("oses", &report.oses),
        ("robots", &report.robots),
        ("request_times", &report.request_times),
        ("virtual_hosts", &report.virtual_hosts),
        ("entry_pages", &report.entry_pages),
        ("exit_pages", &report.exit_pages),
        ("search_engines", &report.search_engines),
        ("keyphrases", &report.keyphrases),
        ("hours", &report.hours),
        ("dates", &report.dates),
        ("months", &report.months),
        ("weekdays", &report.weekdays),
        ("diagnostics", &report.diagnostics),
    ] {
        for item in rows {
            csv_row(&mut out, section, &item.key, item.hits);
        }
    }
    for item in &report.bandwidth_urls {
        csv_bandwidth_row(&mut out, "bandwidth_urls", &item.key, item.hits, item.bytes);
    }
    for item in &report.daily {
        out.push_str(&format!(
            "daily,{},{},{},{},{},{}\n",
            csv_field(&item.date),
            item.hits,
            item.bytes,
            item.pages,
            item.files,
            item.visits
        ));
    }
    out
}

fn csv_row(out: &mut String, section: &str, key: &str, hits: u64) {
    out.push_str(&format!(
        "{},{},{},,,,\n",
        csv_field(section),
        csv_field(key),
        hits
    ));
}

fn csv_bandwidth_row(out: &mut String, section: &str, key: &str, hits: u64, bytes: u64) {
    out.push_str(&format!(
        "{},{},{},{},,,\n",
        csv_field(section),
        csv_field(key),
        hits,
        bytes
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_fields_are_escaped() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("/a,b\"c"), "\"/a,b\"\"c\"");
    }
}
