//! `tinybrowser`: the smallest headless browser for AI agents.
//!
//! One executable. No separately shipped helper. Engine stops at DOM + JS.
//! The same executable may self-spawn a profile daemon and renderer
//! processes.
//! The embeddable surface lives here; CDP is a peer crate.

mod daemon;

use std::future::Future;
use std::io;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;

use browser::{AgentBuilder, Browser, NetworkSession, Profile, ProfileStore};
use clap::{Parser, Subcommand};
use logging::{Config, Level, Logger};

#[derive(Parser)]
#[command(
    name = "tinybrowser",
    version,
    propagate_version = true,
    disable_version_flag = true,
    about = "The smallest headless browser for AI agents"
)]
struct Cli {
    /// Minimum log level: error, warn, info, debug, or trace [default: info]
    ///
    /// A running daemon keeps its start-up level; `--verbose` is shorthand for
    /// `--log-level=debug`.
    #[arg(long, global = true, value_name = "LEVEL", value_parser = parse_level)]
    log_level: Option<Level>,

    /// Shorthand for --log-level=debug
    #[arg(long, global = true)]
    verbose: bool,

    /// Print version
    #[arg(
        short = 'v',
        long = "version",
        short_alias = 'V',
        global = true,
        action = clap::ArgAction::Version
    )]
    version: (),

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the profile daemon until the process exits
    Daemon {
        /// Named profile (implicit name: `default`)
        #[arg(
            long,
            value_name = "NAME",
            value_parser = parse_profile,
            default_value = "default"
        )]
        profile: Profile,
    },
    /// Run a renderer worker on its private platform channel
    Renderer,
    /// Serve classic `WebDriver` on this loopback port
    Webdriver {
        /// Loopback port
        #[arg(long, value_name = "PORT")]
        port: u16,
        /// Named profile (implicit name: `default`)
        #[arg(
            long,
            value_name = "NAME",
            value_parser = parse_profile,
            default_value = "default"
        )]
        profile: Profile,
        /// Rewrite a host to an address; repeatable
        #[arg(long = "resolve", value_name = "PATTERN=ADDR")]
        resolve: Vec<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    install_logger(&cli);
    let code = run(&cli);
    if !logging::flush() {
        logging::error!(target: "logging", "file log did not flush before exit");
    }
    code
}

fn run(cli: &Cli) -> ExitCode {
    match &cli.command {
        Some(Command::Renderer) => match renderer::serve() {
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
        }) => {
            let builder = match resolve_builder(resolve) {
                Ok(builder) => builder,
                Err(error) => return usage_error(&error),
            };
            run_browser_process("webdriver", serve_webdriver(*port, builder, profile))
        }
        None => {
            let mut command = <Cli as clap::CommandFactory>::command();
            let _result = command.print_help();
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

fn parse_profile(value: &str) -> Result<Profile, String> {
    Profile::parse(value).map_err(|error| error.to_string())
}

fn parse_level(value: &str) -> Result<Level, String> {
    value
        .parse()
        .map_err(|error: logging::ParseLevelError| error.to_string())
}

fn resolve_builder(specs: &[String]) -> Result<AgentBuilder, String> {
    let mut builder = AgentBuilder::new();
    for spec in specs {
        builder = builder.resolve(spec).map_err(|error| error.to_string())?;
    }
    Ok(builder)
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
            ExitCode::from(1)
        }
    }
}

async fn serve_webdriver(port: u16, builder: AgentBuilder, profile: &Profile) -> io::Result<()> {
    let data_home = daemon::data_home()?;
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|error| io::Error::new(error.kind(), format!("bind failed: {error}")))?;
    let network = match ProfileStore::open_in(&data_home, profile)
        .and_then(|store| NetworkSession::from_builder(builder, store))
    {
        Ok(network) => network,
        Err(error) => {
            return Err(io::Error::other(format!("profile failed: {error}")));
        }
    };
    let browser = Browser::open_with_network(network)?;
    let result = webdriver::serve(&listener, browser.handle()).await;
    result.and(browser.handle().close().await)
}
