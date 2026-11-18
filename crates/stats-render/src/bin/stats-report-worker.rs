//! Isolated native report-plugin worker. Frontends run it as a child process per request.
//!
//! Usage: `stats-report-worker <plugin-path>`; the request arrives as one JSON line on stdin.

use std::{env, path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    let plugin = match (args.next(), args.next()) {
        (Some(plugin), None) => PathBuf::from(plugin),
        _ => {
            eprintln!("usage: stats-report-worker <plugin-path>");
            return ExitCode::from(2);
        }
    };
    match stats_render::run_report_worker(plugin) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("native report worker failed: {error}");
            ExitCode::from(1)
        }
    }
}
