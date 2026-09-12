//! `tinybrowser`: the smallest headless browser for AI agents.
//!
//! One executable. No separately shipped helper. Engine stops at DOM + JS.
//! The same executable may self-spawn a profile daemon
//! ([ADR 0009](../docs/adrs/0009-named-profile-daemon.md)) and renderer
//! processes ([ADR 0011](../docs/adrs/0011-renderer-processes-per-site.md)).
//! The embeddable surface lives here; CDP is a peer crate.

mod cli;
mod daemon;

use std::path::PathBuf;
use std::process::ExitCode;

use browser::{AgentBuilder, Browser, NetworkSession, Profile, ProfileStore};
use clap::{Parser, Subcommand};
use logging::{Config, Level, Logger};

#[derive(Parser)]
#[command(
    name = "tinybrowser",
    version,
    disable_version_flag = true,
    about = "The smallest headless browser for AI agents"
)]
struct Cli {
    /// Named profile (implicit name: `default`)
    #[arg(
        long,
        global = true,
        value_name = "NAME",
        value_parser = parse_profile,
        default_value = "default"
    )]
    profile: Profile,

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
        action = clap::ArgAction::Version
    )]
    version: (),

    /// Serve classic `WebDriver` on this loopback port
    #[arg(long, value_name = "PORT")]
    webdriver: Option<u16>,

    /// Rewrite a host to an address; repeatable
    #[arg(long = "resolve", global = true, value_name = "PATTERN=ADDR")]
    resolve: Vec<String>,

    /// Run the profile daemon until the process exits (internal)
    #[arg(long, hide = true)]
    daemon: bool,

    /// Run a renderer worker on stdin/stdout (internal)
    #[arg(long, hide = true)]
    renderer: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Create a tab and select it
    Create {
        /// Document URL (defaults to about:blank)
        url: Option<String>,
    },
    /// List live tabs
    List,
    /// Select the tab later commands use
    Select {
        /// Target id from `list`
        id: String,
    },
    /// Evaluate a script in the selected tab
    #[command(alias = "evaluate")]
    Eval {
        /// Script source
        script: String,
    },
    /// Navigate the selected tab
    Navigate {
        /// Document URL
        url: String,
    },
    /// Close a tab (defaults to the selected tab)
    Close {
        /// Target id from `list`
        id: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    install_logger(&cli);
    let code = run(cli);
    if !logging::flush() {
        logging::error!(target: "logging", "file log did not flush before exit");
    }
    code
}

fn run(cli: Cli) -> ExitCode {
    if let Some(error) = mode_conflict(&cli) {
        return usage_error(error);
    }
    if cli.renderer {
        return match renderer::serve_stdio() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                logging::error!(target: "renderer", "{error}");
                ExitCode::from(1)
            }
        };
    }
    if cli.daemon {
        return match daemon::data_home().and_then(|home| daemon::run(&cli.profile, &home)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                logging::error!(target: "daemon", "{error}");
                ExitCode::from(1)
            }
        };
    }
    let builder = match resolve_builder(&cli.resolve) {
        Ok(builder) => builder,
        Err(error) => return usage_error(&error),
    };
    if let Some(port) = cli.webdriver {
        return serve_webdriver(port, builder, &cli.profile);
    }
    if let Some(command) = cli.command {
        cli::run(&cli.profile, command)
    } else {
        let mut command = <Cli as clap::CommandFactory>::command();
        let _result = command.print_help();
        ExitCode::from(2)
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
    let process = if cli.renderer {
        "renderer"
    } else if cli.daemon {
        "daemon"
    } else if cli.webdriver.is_some() {
        "webdriver"
    } else {
        "cli"
    };
    let mut config = Config::new(process).level(level);
    if !cli.renderer
        && (cli.daemon || cli.webdriver.is_some())
        && let Some(path) = profile_log_file(&cli.profile)
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

fn mode_conflict(cli: &Cli) -> Option<&'static str> {
    if cli.renderer {
        if cli.daemon || cli.webdriver.is_some() || !cli.resolve.is_empty() || cli.command.is_some()
        {
            return Some("--renderer does not accept other modes or commands");
        }
        return None;
    }
    if !cli.daemon {
        return None;
    }
    if cli.webdriver.is_some() {
        return Some("--daemon and --webdriver are mutually exclusive");
    }
    if !cli.resolve.is_empty() {
        return Some("--daemon and --resolve are mutually exclusive");
    }
    if cli.command.is_some() {
        return Some("--daemon does not accept a command");
    }
    None
}

fn usage_error(message: &str) -> ExitCode {
    logging::error!(target: "cli", "{message}");
    ExitCode::from(2)
}

fn serve_webdriver(port: u16, builder: AgentBuilder, profile: &Profile) -> ExitCode {
    let data_home = match daemon::data_home() {
        Ok(home) => home,
        Err(error) => {
            logging::error!(target: "webdriver", "{error}");
            return ExitCode::from(1);
        }
    };
    let listener = match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(error) => {
            logging::error!(target: "webdriver", "bind failed: {error}");
            return ExitCode::from(1);
        }
    };
    let network = match ProfileStore::open_in(&data_home, profile)
        .and_then(|store| NetworkSession::from_builder(builder, store))
    {
        Ok(network) => network,
        Err(error) => {
            logging::error!(target: "webdriver", "profile failed: {error}");
            return ExitCode::from(1);
        }
    };
    let browser = Browser::open_with_network(network);
    if let Err(error) = webdriver::serve(&listener, browser.handle()) {
        logging::error!(target: "webdriver", "{error}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
