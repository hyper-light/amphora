//! The `amphora` binary.

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<Option<String>> = std::env::args_os()
        .skip(1)
        .map(|a| a.into_string().ok())
        .collect();
    // postgres (docs/design/postgres.md), serve, health, ask and drill (CP-T1.13); anything else is
    // `reply`'s.
    let plain: Option<Vec<String>> = args.iter().cloned().collect();
    let served = plain.and_then(|plain| {
        amphora::postgres::listener::command(&plain, |addr| {
            let _ = writeln!(
                std::io::stderr().lock(),
                "amphora: PostgreSQL listening on {addr}"
            );
        })
        .or_else(|| {
            amphora::serve::command(&plain, |addr| {
                let _ = writeln!(std::io::stderr().lock(), "amphora: serving on {addr}");
            })
        })
    });
    let reply = served.unwrap_or_else(|| amphora::reply(&args));
    let wrote = if reply.to_stderr {
        std::io::stderr().lock().write_all(reply.text.as_bytes())
    } else {
        std::io::stdout().lock().write_all(reply.text.as_bytes())
    };
    // A closed output is a failure the status reports, never a panic.
    match wrote {
        Ok(()) => ExitCode::from(reply.status),
        Err(_) => ExitCode::from(1),
    }
}
