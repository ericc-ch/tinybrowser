//! `tinybrowser`: the smallest headless browser for AI agents.
//!
//! One executable. No separately shipped helper. Engine stops at DOM + JS.
//! The same executable may self-spawn a profile daemon
//! ([ADR 0009](../docs/adrs/0009-named-profile-daemon.md)).
//! The embeddable surface lives here; CDP is a peer crate.

mod cli;
mod daemon;

use std::process::ExitCode;

use browser::{AgentBuilder, Browser, NetworkSession, Profile, ProfileStore};

pub(crate) const USAGE: &str = "usage: tinybrowser [--profile=NAME] [--webdriver=PORT] [--resolve=PATTERN=ADDR]... [--daemon] [create|list|select|eval|navigate|close] ...";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_mode(&args) {
        Ok(Mode::Usage) => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        Ok(Mode::WebDriver {
            port,
            builder,
            profile,
        }) => serve_webdriver(port, builder, &profile),
        Ok(Mode::Daemon { profile }) => {
            match daemon::data_home().and_then(|home| daemon::run(&profile, &home)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("daemon: {error}");
                    ExitCode::from(1)
                }
            }
        }
        Ok(Mode::Cli { profile, rest }) => cli::run(&profile, &rest),
        Err(error) => {
            eprintln!("{error}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

enum Mode {
    Usage,
    WebDriver {
        port: u16,
        builder: AgentBuilder,
        profile: Profile,
    },
    Daemon {
        profile: Profile,
    },
    Cli {
        profile: Profile,
        rest: Vec<String>,
    },
}

fn parse_mode(args: &[String]) -> Result<Mode, String> {
    let mut port = None;
    let mut daemon = false;
    let mut profile = Profile::default();
    let mut builder = AgentBuilder::new();
    let mut rest = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if let Some(value) = arg.strip_prefix("--webdriver=") {
            port = Some(parse_port(value)?);
        } else if let Some(spec) = arg.strip_prefix("--resolve=") {
            builder = builder.resolve(spec).map_err(|err| err.to_string())?;
        } else if arg == "--daemon" {
            daemon = true;
        } else if let Some(value) = arg.strip_prefix("--profile=") {
            profile = Profile::parse(value).map_err(|err| err.to_string())?;
        } else if arg == "--profile" {
            index += 1;
            let value = args.get(index).ok_or_else(|| USAGE.to_owned())?;
            profile = Profile::parse(value).map_err(|err| err.to_string())?;
        } else if arg.starts_with('-') {
            return Err(USAGE.to_owned());
        } else {
            rest.extend(args[index..].iter().cloned());
            break;
        }
        index += 1;
    }
    if daemon {
        return Ok(Mode::Daemon { profile });
    }
    if let Some(port) = port {
        return Ok(Mode::WebDriver {
            port,
            builder,
            profile,
        });
    }
    if rest.is_empty() {
        return Ok(Mode::Usage);
    }
    Ok(Mode::Cli { profile, rest })
}

fn parse_port(value: &str) -> Result<u16, String> {
    value.parse::<u16>().map_err(|_| USAGE.to_owned())
}

fn serve_webdriver(port: u16, builder: AgentBuilder, profile: &Profile) -> ExitCode {
    let data_home = match daemon::data_home() {
        Ok(home) => home,
        Err(error) => {
            eprintln!("webdriver: {error}");
            return ExitCode::from(1);
        }
    };
    let listener = match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("webdriver bind failed: {error}");
            return ExitCode::from(1);
        }
    };
    let browser = Browser::open_with_network(NetworkSession::from_builder(
        builder,
        ProfileStore::open_in(&data_home, profile),
    ));
    if let Err(error) = webdriver::serve(&listener, browser.handle()) {
        eprintln!("webdriver: {error}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
