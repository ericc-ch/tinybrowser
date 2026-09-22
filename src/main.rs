//! `tinybrowser`: the smallest headless browser for AI agents.
//!
//! One executable. No separately shipped helper. Engine stops at DOM + JS.
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

use browser::{AgentOptions, Browser, Profile};
use cli::{Cli, Command};
use logging::{Config, Level, Logger};

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
            install_logger(&cli);
            let code = run(&cli);
            if !logging::flush() {
                logging::error!(target: "logging", "file log did not flush before exit");
            }
            code
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: &Cli) -> ExitCode {
    match &cli.command {
        Some(Command::Renderer) => match browser::child::serve() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                logging::error!(target: "renderer", "{error}");
                ExitCode::from(1)
            }
        },
        Some(Command::Daemon { profile }) => run_browser_process("daemon", async {
            let home = daemon::data_home()?;
            daemon::run(profile, &home).await
        }),
        Some(Command::Webdriver {
            port,
            profile,
            resolve,
            tls_ca,
        }) => {
            let options = match resolve_config(resolve, tls_ca) {
                Ok(options) => options,
                Err(error) => return usage_error(&error),
            };
            run_browser_process("webdriver", serve_webdriver(*port, options, profile))
        }
        None => {
            print!("{}", cli::HELP);
            ExitCode::from(2)
        }
    }
}

/// Installs the process logger for the mode and flags this invocation selected.
fn install_logger(cli: &Cli) {
    let requested = cli.log_level.or(if cli.verbose {
        Some(Level::Debug)
    } else {
        None
    });
    let level = requested.or_else(env_level).unwrap_or(Level::Info);
    let process = match &cli.command {
        Some(Command::Renderer) => "renderer",
        Some(Command::Daemon { .. }) => "daemon",
        Some(Command::Webdriver { .. }) => "webdriver",
        None => "cli",
    };
    let mut config = Config::new(process).level(level);
    if let Some(Command::Daemon { profile } | Command::Webdriver { profile, .. }) = &cli.command
        && let Some(path) = profile_log_file(profile)
    {
        config = config.file(path);
    }
    logging::install(Logger::new(config));
}

/// Level passed to a spawned daemon or renderer, when no flag overrides it.
fn env_level() -> Option<Level> {
    std::env::var("TINYBROWSER_LOG")
        .ok()
        .and_then(|value| value.parse().ok())
}

/// Per-profile log file, or `None` when no data home can be resolved.
fn profile_log_file(profile: &Profile) -> Option<PathBuf> {
    let home = daemon::data_home().ok()?;
    Some(
        home.join("tinybrowser")
            .join("logs")
            .join(format!("{}.log", profile.name().as_str())),
    )
}

/// Raw `--resolve` specs and `--tls-ca` PEM bytes for [`browser::Agent::new`], which
/// validates them when the browser opens. Only file-read errors fail here.
fn resolve_config(specs: &[String], tls_ca: &[PathBuf]) -> Result<AgentOptions, String> {
    let mut options = AgentOptions {
        resolve: specs.to_vec(),
        ..AgentOptions::default()
    };
    for path in tls_ca {
        let pem =
            std::fs::read(path).map_err(|error| format!("--tls-ca {}: {error}", path.display()))?;
        options.tls_cas.push(pem);
    }
    Ok(options)
}

fn usage_error(message: &str) -> ExitCode {
    logging::error!(target: "cli", "{message}");
    ExitCode::from(2)
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
            // Flag *content* is validated when the profile opens (deferred to
            // `Agent::new`), so it exits like a usage error. Nothing else in
            // the workspace produces `InvalidInput`.
            if error.kind() == io::ErrorKind::InvalidInput {
                ExitCode::from(2)
            } else {
                ExitCode::from(1)
            }
        }
    }
}

async fn serve_webdriver(port: u16, options: AgentOptions, profile: &Profile) -> io::Result<()> {
    let data_home = daemon::data_home()?;
    // Open before binding: option content is validated here, so a bad value
    // surfaces as a usage error before the port is taken.
    let browser = Browser::open_in_with_network(&data_home, profile, options).map_err(|error| {
        if error.kind() == io::ErrorKind::InvalidInput {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid network option (--resolve/--tls-ca): {error}"),
            )
        } else {
            io::Error::other(format!("profile failed: {error}"))
        }
    })?;
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|error| io::Error::new(error.kind(), format!("bind failed: {error}")))?;
    let result = webdriver::serve(&listener, &browser.handle()).await;
    result.and(browser.handle().close().await)
}
