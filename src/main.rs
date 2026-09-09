use std::net::TcpListener;
use std::process::ExitCode;

use browser::AgentBuilder;

const USAGE: &str = "usage: tinybrowser --webdriver=PORT [--resolve=PATTERN=ADDR]...";

fn main() -> ExitCode {
    let mut port = None;
    let mut builder = AgentBuilder::new();
    for arg in std::env::args().skip(1) {
        if let Some(value) = arg.strip_prefix("--webdriver=") {
            let Ok(parsed) = value.parse::<u16>() else {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            };
            port = Some(parsed);
        } else if let Some(spec) = arg.strip_prefix("--resolve=") {
            match builder.resolve(spec) {
                Ok(next) => builder = next,
                Err(error) => {
                    eprintln!("{error}");
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            }
        } else {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    }
    let Some(port) = port else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("webdriver bind failed: {error}");
            return ExitCode::from(1);
        }
    };
    if let Err(error) = webdriver::serve(&listener, builder) {
        eprintln!("webdriver: {error}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
