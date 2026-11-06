use std::{
    io::{self, Write},
    thread,
    time::Duration,
};

fn main() {
    let mode = std::env::args().nth(2).unwrap_or_else(|| "valid".into());
    let mode = mode.strip_prefix("fixture-mode://").unwrap_or(&mode);
    match mode {
        "valid" => print_response(
            r#"{"protocol_version":1,"ok":true,"name":"fixture","version":"1","media_type":"application/json","extension":"json","body":"{}"}"#,
        ),
        "empty" => {}
        "truncated" => print_raw("{\"protocol_version\":1,"),
        "extra" => print_raw(r#"{"protocol_version":1,"ok":true} trailing"#),
        "diagnostic" => print_raw("diagnostic on stdout\n"),
        "invalid-version" => print_raw(r#"{"protocol_version":99,"ok":true}"#),
        "invalid-type" => print_raw(r#"[1,2,3]"#),
        "oversized" => {
            let mut stdout = io::BufWriter::new(io::stdout().lock());
            let _ = stdout.write_all(&vec![b'x'; 8 * 1024 * 1024 + 1]);
            let _ = stdout.flush();
        }
        "partial-hang" => {
            print_raw("{\"protocol_version\":1,");
            thread::sleep(Duration::from_secs(30));
        }
        "hang" => thread::sleep(Duration::from_secs(30)),
        "exit" => std::process::exit(17),
        "abort" => std::process::abort(),
        "segv" => segv(),
        other => panic!("unknown worker fixture mode {other}"),
    }
}

fn print_response(response: &str) {
    print_raw(response);
}

fn print_raw(response: &str) {
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let _ = stdout.write_all(response.as_bytes());
    let _ = stdout.flush();
}

#[cfg(unix)]
fn segv() {
    unsafe {
        let core_limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        libc::setrlimit(libc::RLIMIT_CORE, &core_limit);
        libc::signal(libc::SIGSEGV, libc::SIG_DFL);
        libc::raise(libc::SIGSEGV);
    }
    std::process::abort();
}

#[cfg(not(unix))]
fn segv() {
    std::process::abort();
}
