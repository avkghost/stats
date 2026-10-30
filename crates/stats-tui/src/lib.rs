use anyhow::Result;
use stats_core::{Report, ReportRow};
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
}

impl View {
    fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Geography => "Geography",
            Self::Clients => "Clients",
            Self::Performance => "Performance",
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
        report.hits,
        report.pages,
        report.files,
        report.visits,
        report.bytes,
        log_file.display()
    );
    frame.render_widget(
        Paragraph::new(overview).block(Block::bordered().title("Stats")),
        header,
    );
    match view {
        View::Overview => {
            render_rows(frame, top_left, "Top URLs", &report.urls);
            render_rows(frame, top_right, "HTTP Errors", &report.errors);
            render_rows(frame, bottom_left, "Referrers", &report.referers);
            render_rows(
                frame,
                bottom_right,
                "Referring Sites",
                &report.referrer_domains,
            );
        }
        View::Geography => {
            render_rows(frame, top_left, "Countries", &report.countries);
            render_rows(frame, top_right, "Regions", &report.regions);
            render_rows(frame, bottom_left, "Cities", &report.cities);
            render_rows(frame, bottom_right, "Organizations", &report.organizations);
        }
        View::Clients => {
            render_rows(frame, top_left, "User agents", &report.user_agents);
            render_rows(frame, top_right, "Browsers", &report.browsers);
            render_rows(frame, bottom_left, "Operating systems", &report.oses);
            render_rows(frame, bottom_right, "Robots", &report.robots);
        }
        View::Performance => {
            render_rows(frame, top_left, "Request times", &report.request_times);
            render_rows(frame, top_right, "Hours", &report.hours);
            render_rows(frame, bottom_left, "Status classes", &report.status_classes);
            render_rows(frame, bottom_right, "Methods", &report.methods);
        }
    }
    frame.render_widget(
        Paragraph::new(format!(
            "1-4 views | r refresh | q/Esc/Ctrl-C quit | {} | {}s | diag {}",
            view.title(),
            refresh.as_secs(),
            report.diagnostics.iter().map(|row| row.hits).sum::<u64>()
        ))
        .style(Style::default().fg(Color::DarkGray)),
        footer,
    );
}

fn render_rows(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    title: &str,
    rows: &[ReportRow],
) {
    use ratatui::widgets::{Block, List, ListItem};
    let items = rows
        .iter()
        .map(|row| ListItem::new(format!("{:>8}  {}", row.hits, row.key)))
        .collect::<Vec<_>>();
    frame.render_widget(List::new(items).block(Block::bordered().title(title)), area);
}
