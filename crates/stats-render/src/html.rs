use std::{fs, path::Path};

use anyhow::Context;
use stats_core::{BandwidthRow, DailyRow, Report, ReportRow};

pub(crate) fn table(rows: &[ReportRow]) -> String {
    let mut out = String::from(
        "<table class=\"report-table\" data-filterable><thead><tr><th>Key</th><th>Hits</th></tr></thead><tbody>",
    );
    for row in rows {
        out.push_str(&format!(
            "<tr title=\"{}: {} hits\"><td>{}</td><td>{}</td></tr>",
            html_escape(&row.key),
            row.hits,
            html_escape(&row.key),
            row.hits
        ));
    }
    out.push_str("</tbody></table>");
    out
}

pub(crate) fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(crate) fn bandwidth_table(rows: &[BandwidthRow]) -> String {
    let mut out = String::from(
        "<table class=\"report-table\" data-filterable><thead><tr><th>Key</th><th>Hits</th><th>Bytes</th></tr></thead><tbody>",
    );
    for row in rows {
        out.push_str(&format!(
            "<tr title=\"{}: {} hits, {} bytes\"><td>{}</td><td>{}</td><td>{}</td></tr>",
            html_escape(&row.key),
            row.hits,
            row.bytes,
            html_escape(&row.key),
            row.hits,
            row.bytes
        ));
    }
    out.push_str("</tbody></table>");
    out
}

pub(crate) fn daily_table(rows: &[DailyRow]) -> String {
    let mut out = String::from(
        "<table class=\"report-table\" data-filterable><thead><tr><th>Date</th><th>Hits</th><th>Bytes</th><th>Pages</th><th>Files</th><th>Visits</th></tr></thead><tbody>",
    );
    for row in rows {
        out.push_str(&format!(
            "<tr title=\"{}: {} hits, {} bytes\"><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            html_escape(&row.date),
            row.hits,
            row.bytes,
            html_escape(&row.date),
            row.hits,
            row.bytes,
            row.pages,
            row.files,
            row.visits
        ));
    }
    out.push_str("</tbody></table>");
    out
}

pub(crate) fn daily_chart(rows: &[DailyRow]) -> String {
    let maximum = rows.iter().map(|row| row.hits).max().unwrap_or(0);
    let mut out = String::from(
        "<div class=\"aw-chart\" aria-label=\"Daily traffic chart\"><div class=\"aw-chart-caption\"><span>Daily traffic</span><span>Hits</span></div>",
    );
    for row in rows {
        let width = row
            .hits
            .saturating_mul(100)
            .checked_div(maximum)
            .unwrap_or(0);
        out.push_str(&format!(
            "<div class=\"aw-chart-row\" title=\"{}: {} hits\"><span class=\"aw-chart-label\">{}</span><span class=\"aw-chart-track\"><span class=\"aw-chart-bar\" style=\"width:{}%\"></span></span><span class=\"aw-chart-value\">{}</span></div>",
            html_escape(&row.date), row.hits, html_escape(&row.date), width, row.hits
        ));
    }
    out.push_str("</div>");
    out
}

pub(crate) fn row_chart(rows: &[ReportRow], label: &str) -> String {
    let maximum = rows.iter().map(|row| row.hits).max().unwrap_or(0);
    let mut out = format!(
        "<div class=\"aw-chart\" aria-label=\"{} chart\"><div class=\"aw-chart-caption\"><span>{}</span><span>Hits</span></div>",
        html_escape(label),
        html_escape(label)
    );
    for row in rows {
        let width = row
            .hits
            .saturating_mul(100)
            .checked_div(maximum)
            .unwrap_or(0);
        out.push_str(&format!(
            "<div class=\"aw-chart-row\" title=\"{}: {} hits\"><span class=\"aw-chart-label\">{}</span><span class=\"aw-chart-track\"><span class=\"aw-chart-bar\" style=\"width:{}%\"></span></span><span class=\"aw-chart-value\">{}</span></div>",
            html_escape(&row.key), row.hits, html_escape(&row.key), width, row.hits
        ));
    }
    out.push_str("</div>");
    out
}

pub(crate) fn bandwidth_chart(rows: &[BandwidthRow]) -> String {
    let maximum = rows.iter().map(|row| row.bytes).max().unwrap_or(0);
    let mut out = String::from(
        "<div class=\"aw-chart\" aria-label=\"Bandwidth by URL chart\"><div class=\"aw-chart-caption\"><span>Bandwidth by URL</span><span>Bytes</span></div>",
    );
    for row in rows {
        let width = row
            .bytes
            .saturating_mul(100)
            .checked_div(maximum)
            .unwrap_or(0);
        out.push_str(&format!(
            "<div class=\"aw-chart-row\" title=\"{}: {} bytes\"><span class=\"aw-chart-label\">{}</span><span class=\"aw-chart-track\"><span class=\"aw-chart-bar\" style=\"width:{}%\"></span></span><span class=\"aw-chart-value\">{}</span></div>",
            html_escape(&row.key), row.bytes, html_escape(&row.key), width, row.bytes
        ));
    }
    out.push_str("</div>");
    out
}

fn section(title: &str, content: String) -> String {
    format!(
        "<section class=\"report-section\"><h2>{}</h2>{}</section>",
        html_escape(title),
        content
    )
}

fn metric(label: &str, value: u64) -> String {
    format!(
        "<div class=\"metric\"><strong>{value}</strong><span>{}</span></div>",
        html_escape(label)
    )
}

fn table_limited(rows: &[ReportRow], limit: usize) -> String {
    table(&rows[..rows.len().min(limit)])
}

fn overview(report: &Report, periods: &[(String, String)]) -> String {
    let period_section = if periods.is_empty() {
        String::new()
    } else {
        let links = periods
            .iter()
            .map(|(label, href)| {
                format!(
                    "<li><a href=\"{}\">{}</a></li>",
                    html_escape(href),
                    html_escape(label)
                )
            })
            .collect::<String>();
        section(
            "Periods",
            format!("<ul class=\"period-links\">{links}</ul>"),
        )
    };
    format!(
        "<div class=\"eyebrow\">Traffic overview</div><h1>Stats Web Server Statistics</h1><p class=\"lede\">Aggregated activity from the configured access log.</p><div class=\"metric-grid\">{}{}{}{}{}{}</div><div class=\"overview-grid\">{}{}{}{}{}</div>",
        metric("Hits", report.hits),
        metric("Bytes", report.bytes),
        metric("Pages", report.pages),
        metric("Files", report.files),
        metric("Visits", report.visits),
        metric("Unique visits", report.unique_visits),
        section("Status classes", row_chart(&report.status_classes, "Status classes")),
        section("Daily traffic", daily_chart(&report.daily)),
        section("Top URLs", table_limited(&report.urls, 5)),
        section("Top referrers", table_limited(&report.referers, 5)),
        period_section,
    )
}

fn detail_body(report: &Report, page: &str) -> String {
    match page {
        "visitors" => format!(
            "{}{}{}{}{}{}",
            section("Users", table(&report.users)),
            section("Hostnames", table(&report.hostnames)),
            section("User agents", table(&report.user_agents)),
            section("Browsers", table(&report.browsers)),
            section("Operating systems", table(&report.oses)),
            section("Robots", table(&report.robots))
        ),
        "pages" => format!(
            "{}{}{}{}{}{}",
            section("Methods", table(&report.methods)),
            section("URLs", table(&report.urls)),
            section("File types", table(&report.file_types)),
            section("Entry pages", table(&report.entry_pages)),
            section("Exit pages", table(&report.exit_pages)),
            section("Security probes", table(&report.scanners))
        ),
        "referrers" => format!(
            "{}{}{}{}{}",
            section("Referrers", table(&report.referers)),
            section("Referring sites", table(&report.referrer_domains)),
            section("Referer spam", table(&report.referer_spam)),
            section("Search engines", table(&report.search_engines)),
            section("Keyphrases", table(&report.keyphrases))
        ),
        "geography" => format!(
            "{}{}{}{}{}{}",
            section("Virtual hosts", table(&report.virtual_hosts)),
            section("Countries", table(&report.countries)),
            section("Regions", table(&report.regions)),
            section("Cities", table(&report.cities)),
            section("ASN", table(&report.asns)),
            section("Organizations", table(&report.organizations))
        ),
        "calendar" => format!(
            "{}{}{}{}{}",
            section("Months", table(&report.months)),
            section("Days of month", table(&report.dates)),
            section("Days of week", table(&report.weekdays)),
            section("Hours", table(&report.hours)),
            section("Daily traffic", daily_table(&report.daily))
        ),
        "performance" => format!(
            "{}{}{}{}",
            section("Request times", table(&report.request_times)),
            section("Hours", table(&report.hours)),
            section(
                "Bandwidth by URL",
                format!(
                    "{}{}",
                    bandwidth_chart(&report.bandwidth_urls),
                    bandwidth_table(&report.bandwidth_urls)
                )
            ),
            section(
                "Daily traffic",
                format!(
                    "{}{}",
                    daily_chart(&report.daily),
                    daily_table(&report.daily)
                )
            )
        ),
        _ => overview(report, &[]),
    }
}

fn sidebar(active: &str, overview_link: &str, standalone: bool) -> String {
    let link = |page: &str, label: &str| {
        let class = if active == page {
            " class=\"active\""
        } else {
            ""
        };
        format!(
            "<a{class} href=\"{}\">{label}</a>",
            match page {
                "overview" => overview_link.to_owned(),
                _ => format!("{page}.html"),
            }
        )
    };
    let body = format!("<a class=\"brand\" href=\"{}\">Stats</a><p class=\"side-note\">AWStats-style traffic analysis</p><nav>{}{}{}{}{}{}{}</nav>", overview_link, link("overview", "Overview"), link("visitors", "Visitors"), link("pages", "Pages"), link("referrers", "Referrers"), link("geography", "Geography"), link("performance", "Performance"), link("calendar", "Calendar"));
    if standalone {
        format!("<nav class=\"inline-nav\">{body}</nav>")
    } else {
        format!("<aside class=\"sidebar\">{body}</aside>")
    }
}

fn site_css() -> &'static str {
    r#":root { color-scheme: light; --ink:#203247; --muted:#66798d; --blue:#1f5b91; --navy:#143a61; --pale:#eaf2f9; --line:#c8d7e5; --paper:#ffffff; --canvas:#eef3f7; }
* { box-sizing:border-box; }
html, body { margin:0; min-width:320px; width:100%; }
body { background:var(--canvas); color:var(--ink); font:14px/1.45 Arial, Helvetica, sans-serif; }
a { color:var(--blue); }
.site { display:grid; grid-template-columns:220px minmax(0,1fr); min-height:100vh; }
.sidebar { background:linear-gradient(180deg,var(--navy),#0f2b48); color:white; padding:24px 18px; }
.brand { color:white; display:block; font-size:24px; font-weight:bold; text-decoration:none; }
.side-note { color:#c8d9e8; font-size:12px; margin:6px 0 24px; }
.sidebar nav { display:grid; gap:5px; }
.sidebar nav a { border-left:3px solid transparent; color:#d9e7f3; padding:9px 10px; text-decoration:none; }
.sidebar nav a:hover, .sidebar nav a.active { background:#255d8f; border-left-color:#8fc3e8; color:white; }
.content { min-width:0; padding:28px 34px 42px; }
.eyebrow { color:var(--blue); font-size:12px; font-weight:bold; letter-spacing:.08em; text-transform:uppercase; }
h1 { font-size:30px; line-height:1.1; margin:5px 0 8px; }
.lede { color:var(--muted); margin:0 0 24px; }
.metric-grid { display:grid; gap:12px; grid-template-columns:repeat(6,minmax(110px,1fr)); margin:12px 0; }
.metric { background:var(--paper); border-top:4px solid var(--blue); box-shadow:0 1px 3px #18344a18; padding:14px 15px; }
.metric strong { display:block; font-size:24px; line-height:1.1; }
.metric span { color:var(--muted); display:block; font-size:12px; margin-top:5px; }
.overview-grid { display:grid; gap:20px; grid-template-columns:repeat(2,minmax(0,1fr)); margin-top:22px; }
.period-links { display:flex; flex-wrap:wrap; gap:10px 18px; margin:0; padding-left:20px; }
.report-section { background:var(--paper); box-shadow:0 1px 3px #18344a18; margin:0 0 22px; min-width:0; overflow:auto; }
.report-section h2 { background:var(--pale); border-left:5px solid var(--blue); color:var(--navy); font-size:17px; margin:0; padding:10px 13px; }
.report-section > table, .report-section > .aw-chart { margin:0; }
.report-table { border-collapse:collapse; min-width:100%; width:100%; }
.report-table th { background:var(--blue); color:white; font-weight:bold; text-align:left; }
.report-table td, .report-table th { border-bottom:1px solid var(--line); padding:7px 10px; white-space:nowrap; }
.report-table tr:nth-child(even) td { background:#f6f9fc; }
.aw-chart { background:#fbfdff; border-bottom:1px solid var(--line); padding:12px 14px; }
.aw-chart-caption { border-bottom:1px solid var(--line); display:flex; font-weight:bold; justify-content:space-between; margin:-2px 0 8px; padding-bottom:7px; }
.aw-chart-row { align-items:center; display:flex; gap:10px; margin:6px 0; }
.aw-chart-label { flex:0 0 180px; font-size:12px; overflow:hidden; text-align:right; text-overflow:ellipsis; white-space:nowrap; }
.aw-chart-track { background:#dce8f5; flex:1; height:17px; }
.aw-chart-bar { background:var(--blue); display:block; height:100%; min-width:2px; }
.aw-chart-value { flex:0 0 80px; font-size:12px; text-align:right; }
.table-filter { border:1px solid var(--line); font:inherit; margin:10px; padding:7px 9px; width:calc(100% - 20px); }
.footer { color:var(--muted); font-size:12px; padding:0 34px 24px; }
.inline-nav { padding:16px; }
.standalone-brand { border-bottom:4px solid var(--navy); color:var(--navy); display:flex; gap:12px; justify-content:space-between; padding:16px 24px; }
.standalone-brand span { color:var(--muted); font-size:12px; }
@media (max-width:900px) { .metric-grid { grid-template-columns:repeat(3,minmax(110px,1fr)); } }
@media (max-width:700px) { .site { display:block; } .sidebar { padding:14px; } .sidebar nav { display:flex; flex-wrap:wrap; } .sidebar nav a { padding:7px 9px; } .content { padding:20px 14px 28px; } .metric-grid, .overview-grid { grid-template-columns:1fr 1fr; } .aw-chart-label { flex-basis:120px; } .footer { padding:0 14px 20px; } }
@media (max-width:440px) { .metric-grid, .overview-grid { grid-template-columns:1fr; } }
"#
}

fn site_js() -> &'static str {
    r#"(() => {
  document.querySelectorAll('table[data-filterable]').forEach(table => {
    const rows = [...table.querySelectorAll('tbody tr')];
    if (rows.length < 12) return;
    const input = document.createElement('input');
    input.className = 'table-filter'; input.type = 'search'; input.placeholder = 'Filter this table';
    table.parentNode.insertBefore(input, table);
    input.addEventListener('input', () => {
      const query = input.value.toLowerCase();
      rows.forEach(row => { row.hidden = query && !row.textContent.toLowerCase().includes(query); });
    });
  });
})();"#
}

fn page_html(
    report: &Report,
    page: &str,
    overview_link: &str,
    standalone: bool,
    periods: &[(String, String)],
) -> String {
    let title = match page {
        "overview" => "Overview",
        "visitors" => "Visitors",
        "pages" => "Pages",
        "referrers" => "Referrers",
        "geography" => "Geography",
        "performance" => "Performance",
        "calendar" => "Calendar",
        _ => "Overview",
    };
    let body = match page {
        "overview" => overview(report, periods),
        _ => {
            let title_lower = title.to_lowercase();
            format!(
                "<div class=\"eyebrow\">Detail view</div><h1>{title}</h1><p class=\"lede\">Explore the {title_lower} dimensions independently.</p>{}",
                detail_body(report, page)
            )
        }
    };
    let assets = if standalone {
        format!("<style>{}</style>", site_css())
    } else {
        "<link rel=\"stylesheet\" href=\"assets/stats.css\">".to_owned()
    };
    let script = if standalone {
        format!("<script>{}</script>", site_js())
    } else {
        "<script src=\"assets/stats.js\" defer></script>".to_owned()
    };
    let navigation = if standalone {
        "<div class=\"standalone-brand\"><strong>Stats Web Server Statistics</strong><span>Aggregated overview</span></div>".to_owned()
    } else {
        sidebar(page, overview_link, false)
    };
    format!("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Stats — {}</title>{}</head><body><div class=\"site\">{}<main class=\"content\">{}</main></div><footer class=\"footer\">Generated by Stats · AWStats-style Rust web log analyzer</footer>{}</body></html>", html_escape(title), assets, navigation, body, script)
}

pub fn render_html_report(report: &Report) -> String {
    page_html(report, "overview", "#overview", true, &[])
}

pub fn render_html_section(report: &Report, section: &str) -> Result<String, String> {
    let page = match section {
        "overview" | "visitors" | "pages" | "referrers" | "geography" | "performance"
        | "calendar" => section,
        _ => return Err(format!("unsupported HTML report section {section:?}")),
    };
    Ok(page_html(report, page, "index.html", true, &[]))
}

pub fn write_html_site(report: &Report, output: &Path) -> anyhow::Result<()> {
    write_html_site_with_periods(report, output, &[])
}

pub fn write_html_site_with_periods(
    report: &Report,
    output: &Path,
    periods: &[(String, String)],
) -> anyhow::Result<()> {
    let directory = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(directory)
        .with_context(|| format!("create report directory {}", directory.display()))?;
    fs::create_dir_all(directory.join("assets"))
        .with_context(|| format!("create report assets directory {}", directory.display()))?;
    let overview_link = output
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("index.html");
    fs::write(
        output,
        page_html(report, "overview", overview_link, false, periods),
    )
    .with_context(|| format!("write report {}", output.display()))?;
    for page in [
        "visitors",
        "pages",
        "referrers",
        "geography",
        "performance",
        "calendar",
    ] {
        let path = directory.join(format!("{page}.html"));
        fs::write(&path, page_html(report, page, overview_link, false, &[]))
            .with_context(|| format!("write report page {}", path.display()))?;
    }
    fs::write(directory.join("assets/stats.css"), site_css())
        .with_context(|| "write report stylesheet")?;
    fs::write(directory.join("assets/stats.js"), site_js())
        .with_context(|| "write report script")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_report_table_values() {
        assert_eq!(
            html_escape("<site & \"path\">"),
            "&lt;site &amp; &quot;path&quot;&gt;"
        );
        assert!(table(&[ReportRow {
            key: "<unsafe>".into(),
            hits: 2,
        }])
        .contains("&lt;unsafe&gt;"));
        assert!(table(&[ReportRow {
            key: "<unsafe>".into(),
            hits: 2,
        }])
        .contains("title=\"&lt;unsafe&gt;: 2 hits\""));
        assert!(row_chart(
            &[ReportRow {
                key: "<unsafe>".into(),
                hits: 2,
            }],
            "Requests"
        )
        .contains("title=\"&lt;unsafe&gt;: 2 hits\""));
    }

    #[test]
    fn shared_assets_support_fast_detail_pages() {
        assert!(site_css().contains(".sidebar"));
        assert!(site_css().contains(".report-table"));
        assert!(site_js().contains("data-filterable"));
        assert!(site_js().contains("Filter this table"));
    }
}
