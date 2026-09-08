use std::net::TcpListener;
use std::process::ExitCode;

const USAGE: &str = "usage: tinybrowser --webdriver=PORT";

fn main() -> ExitCode {
    let Some(port) = std::env::args().find_map(|arg| {
        arg.strip_prefix("--webdriver=")
            .and_then(|port| port.parse::<u16>().ok())
    }) else {
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
    if let Err(error) = webdriver::serve(&listener) {
        eprintln!("webdriver: {error}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
