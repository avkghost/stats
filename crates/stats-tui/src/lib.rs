use anyhow::Result;
use stats_core::{NotFoundRow, Report, ReportRow};
use std::{
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
enum View {
    Overview,
    Geography,
    Clients,
    Performance,
    Security,
}

impl View {
    fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Geography => "Geography",
            Self::Clients => "Clients",
            Self::Performance => "Performance",
            Self::Security => "Security",
        }
    }
}

/// Run the terminal dashboard until the user quits.
///
/// The callback owns refresh/update policy in the CLI. This crate only handles
/// terminal input, presentation, and replacing the displayed report.
pub fn run<F>(
    mut report: Report,
    log_file: &Path,
    refresh: Duration,
    mut refresh_report: F,
) -> Result<()>
where
    F: FnMut() -> Result<Report>,
{
    let mut terminal = ratatui::init();
    let result = run_loop(
        &mut terminal,
        &mut report,
        log_file,
        refresh,
        &mut refresh_report,
    );
    ratatui::restore();
    result
}

fn run_loop<F>(
    terminal: &mut ratatui::DefaultTerminal,
    report: &mut Report,
    log_file: &Path,
    refresh: Duration,
    refresh_report: &mut F,
) -> Result<()>
where
    F: FnMut() -> Result<Report>,
{
    let mut last_refresh = Instant::now();
    let mut view = View::Overview;
    loop {
        terminal.draw(|frame| draw(frame, report, log_file, refresh, view))?;
        if ratatui::crossterm::event::poll(Duration::from_millis(100))? {
            if let ratatui::crossterm::event::Event::Key(key) = ratatui::crossterm::event::read()? {
                use ratatui::crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
                if key.kind == KeyEventKind::Press
                    && (matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                        || (key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL)))
                {
                    break;
                }
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('1') => view = View::Overview,
                        KeyCode::Char('2') => view = View::Geography,
                        KeyCode::Char('3') => view = View::Clients,
                        KeyCode::Char('4') => view = View::Performance,
                        KeyCode::Char('5') => view = View::Security,
                        KeyCode::Char('r') => last_refresh = Instant::now() - refresh,
                        _ => {}
                    }
                }
            }
        }
        if last_refresh.elapsed() >= refresh {
            *report = refresh_report()?;
            last_refresh = Instant::now();
        }
    }
    Ok(())
}

fn draw(
    frame: &mut ratatui::Frame,
    report: &Report,
    log_file: &Path,
    refresh: Duration,
    view: View,
) {
    use ratatui::{
        layout::{Constraint, Layout},
        style::{Color, Style},
        widgets::{Block, Paragraph},
    };

    let [header, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let [upper, lower] =
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(body);
    let [top_left, top_right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(upper);
    let [bottom_left, bottom_right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(lower);
    let overview = format!(
        "Hits {}  Pages {}  Files {}  Visits {}  Bytes {}  |  {}",
        stats_core::format_count(report.hits),
        stats_core::format_count(report.pages),
        stats_core::format_count(report.files),
        stats_core::format_count(report.visits),
        stats_core::format_bytes(report.bytes),
        log_file.display()
    );
    frame.render_widget(
        Paragraph::new(overview).block(Block::bordered().title("Stats")),
        header,
    );
    match view {
        View::Overview => {
            render_rows(frame, top_left, "Top URLs", &report.urls, false);
            render_rows(frame, top_right, "HTTP Errors", &report.errors, false);
            render_rows(frame, bottom_left, "Referrers", &report.referers, false);
            render_rows(
                frame,
                bottom_right,
                "Referring Sites",
                &report.referrer_domains,
                false,
            );
        }
        View::Geography => {
            render_rows(frame, top_left, "Countries", &report.countries, false);
            render_rows(frame, top_right, "Regions", &report.regions, false);
            render_rows(frame, bottom_left, "Cities", &report.cities, false);
            render_rows(
                frame,
                bottom_right,
                "Organizations",
                &report.organizations,
                false,
            );
        }
        View::Clients => {
            render_rows(frame, top_left, "User agents", &report.user_agents, false);
            render_rows(frame, top_right, "Browsers", &report.browsers, false);
            render_rows(frame, bottom_left, "Operating systems", &report.oses, false);
            render_rows(frame, bottom_right, "Robots", &report.robots, false);
        }
        View::Performance => {
            render_rows(
                frame,
                top_left,
                "Request times",
                &report.request_times,
                false,
            );
            render_rows(frame, top_right, "Hours", &report.hours, false);
            render_rows(
                frame,
                bottom_left,
                "Status classes",
                &report.status_classes,
                false,
            );
            render_rows(frame, bottom_right, "Methods", &report.methods, false);
        }
        View::Security => {
            render_rows(
                frame,
                top_left,
                "HTTP method anomalies",
                &report.method_anomalies,
                true,
            );
            render_rows(frame, top_right, "Security probes", &report.scanners, true);
            render_not_found(frame, bottom_left, &report.not_found);
            render_rows(frame, bottom_right, "HTTP errors", &report.errors, false);
        }
    }
    frame.render_widget(
        Paragraph::new(format!(
            "1-5 views | r refresh | q/Esc/Ctrl-C quit | {} | {}s | diag {}",
            view.title(),
            refresh.as_secs(),
            report.diagnostics.iter().map(|row| row.hits).sum::<u64>()
        ))
        .style(Style::default().fg(Color::DarkGray)),
        footer,
    );
}

fn render_not_found(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, rows: &[NotFoundRow]) {
    use ratatui::{
        style::{Color, Style},
        widgets::{Block, List, ListItem},
    };
    let items = rows
        .iter()
        .map(|row| {
            ListItem::new(format!("{:>8}  {}", row.hits, row.url))
                .style(Style::default().fg(Color::Red))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(Block::bordered().title("Required but not found (404)")),
        area,
    );
}

fn render_rows(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    title: &str,
    rows: &[ReportRow],
    highlight: bool,
) {
    use ratatui::{
        style::{Color, Style},
        widgets::{Block, List, ListItem},
    };
    let items = rows
        .iter()
        .map(|row| {
            let item = ListItem::new(format!("{:>8}  {}", row.hits, row.key));
            if highlight {
                item.style(Style::default().fg(Color::Red))
            } else {
                item
            }
        })
        .collect::<Vec<_>>();
    frame.render_widget(List::new(items).block(Block::bordered().title(title)), area);
}

#[cfg(test)]
mod tests {
    use super::{draw, View};
    use ratatui::{backend::TestBackend, Terminal};
    use stats_core::{Report, ReportMetadata};
    use std::{path::Path, time::Duration};

    fn empty_report() -> Report {
        Report {
            metadata: ReportMetadata {
                schema_version: Some("13".into()),
                input_path: None,
                input_size: None,
                input_lines: None,
                config_path: None,
                config_hash: None,
                approximations: Vec::new(),
            },
            hits: 1,
            bytes: 1024,
            pages: 1,
            files: 0,
            visits: 1,
            unique_visits: 1,
            countries: Vec::new(),
            regions: Vec::new(),
            cities: Vec::new(),
            asns: Vec::new(),
            organizations: Vec::new(),
            hostnames: Vec::new(),
            statuses: Vec::new(),
            status_classes: Vec::new(),
            errors: Vec::new(),
            methods: Vec::new(),
            method_anomalies: Vec::new(),
            urls: Vec::new(),
            not_found: Vec::new(),
            bandwidth_urls: Vec::new(),
            daily: Vec::new(),
            users: Vec::new(),
            user_agents: Vec::new(),
            referers: Vec::new(),
            referrer_domains: Vec::new(),
            referer_spam: Vec::new(),
            scanners: Vec::new(),
            file_types: Vec::new(),
            browsers: Vec::new(),
            oses: Vec::new(),
            robots: Vec::new(),
            request_times: Vec::new(),
            virtual_hosts: Vec::new(),
            entry_pages: Vec::new(),
            exit_pages: Vec::new(),
            search_engines: Vec::new(),
            keyphrases: Vec::new(),
            hours: Vec::new(),
            dates: Vec::new(),
            months: Vec::new(),
            weekdays: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn renders_overview_with_test_backend() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let report = empty_report();
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &report,
                    Path::new("access.log"),
                    Duration::from_secs(5),
                    View::Overview,
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let rendered = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Stats"));
        assert!(rendered.contains("Top URLs"));
    }
}
