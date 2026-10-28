//! `tinybrowser`: the smallest headless browser for AI agents.
//!
//! One executable. No separately shipped helper. Engine is Blitz
//! parse/style/layout/paint plus `QuickJS` behind a value-only browser
//! interface.
//! The same executable may self-spawn a profile daemon and renderer
//! processes.
//! The embeddable surface lives here; CDP is a peer crate.

mod cli;
mod daemon;

use std::future::Future;
use std::io;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;

use browser::{Browser, BrowserOptions, Profile, default_data_home};
use cli::{Cli, Command};
use logging::Level;

fn main() -> ExitCode {
    match cli::parse(std::env::args_os().skip(1)) {
        Ok(cli::Outcome::Version) => {
            println!("tinybrowser {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(cli::Outcome::Help) => {
            print!("{}", cli::HELP);
            ExitCode::SUCCESS
        }
        Ok(cli::Outcome::Run(cli)) => {
            if let Err(error) = install_logger(&cli) {
                eprintln!("error: {error}");
                if let Some(source) = std::error::Error::source(&error) {
                    eprintln!("  caused by: {source}");
                }
                return ExitCode::from(2);
            }
            let code = run(*cli);
            if !logging::flush() {
                logging::error!(target: "logging", "file log did not flush before exit");
            }
            code
        }
        // Every `CliError` renders its own cause in its message, so a chain
        // printed here would repeat it.
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> ExitCode {
    match cli.command {
        Some(Command::Renderer) => match browser::child::serve() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                logging::error!(target: "renderer", "{error}");
                ExitCode::from(1)
            }
        },
        Some(Command::Daemon { profile }) => run_browser_process("daemon", async {
            let home = default_data_home()?;
            daemon::run(&profile, &home).await
        }),
        Some(Command::Webdriver {
            port,
            profile,
            network,
        }) => run_browser_process("webdriver", serve_webdriver(port, *network, &profile)),
        None => {
            print!("{}", cli::HELP);
            ExitCode::from(2)
        }
    }
}

/// Installs the process logger for the mode and flags this invocation selected.
fn install_logger(cli: &Cli) -> Result<(), logging::InstallError> {
    let requested = cli.log_level.or(if cli.verbose {
        Some(Level::Debug)
    } else {
        None
    });
    let level = requested.or_else(env_level).unwrap_or(Level::Info);
    let file = cli.log_file.clone().or_else(|| match &cli.command {
        Some(Command::Daemon { profile } | Command::Webdriver { profile, .. }) => {
            profile_log_file(profile)
        }
        Some(Command::Renderer) | None => None,
    });
    logging::install(level, file, std::time::SystemTime::now)
}

/// Level passed to a spawned daemon or renderer, when no flag overrides it.
fn env_level() -> Option<Level> {
    std::env::var("TINYBROWSER_LOG")
        .ok()
        .and_then(|value| value.parse().ok())
}

/// Per-profile log file, or `None` when no data home can be resolved.
fn profile_log_file(profile: &Profile) -> Option<PathBuf> {
    let home = default_data_home().ok()?;
    Some(
        home.join("tinybrowser")
            .join("logs")
            .join(format!("{}.log", profile.name().as_str())),
    )
}

fn run_browser_process(
    target: &'static str,
    future: impl Future<Output = io::Result<()>>,
) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            logging::error!(target: target, "runtime failed: {error}");
            return ExitCode::from(1);
        }
    };
    match runtime.block_on(future) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            logging::error!(target: target, "{error}");
            ExitCode::from(1)
        }
    }
}

async fn serve_webdriver(
    port: u16,
    options: browser::AgentOptions,
    profile: &Profile,
) -> io::Result<()> {
    let data_home = default_data_home()?;
    let browser = Browser::new(BrowserOptions {
        data_home: Some(data_home),
        profile: profile.clone(),
        network: options,
    })?;
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|error| io::Error::new(error.kind(), format!("bind failed: {error}")))?;
    let result = webdriver::serve(&listener, browser.handle()).await;
    let close = browser.close().await;
    result.and(close)
}
