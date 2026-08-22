use std::process::ExitCode;

const USAGE: &str = "usage: tinybrowser <serve|fetch> [args]";

fn main() -> ExitCode {
    let Some(cmd) = std::env::args().nth(1) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match cmd.as_str() {
        "serve" | "fetch" => {
            eprintln!("tinybrowser {cmd}: not wired up yet");
            ExitCode::from(2)
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
