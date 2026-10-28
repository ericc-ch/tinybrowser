//! Command-line boundary: five global flags, three subcommands, hand-scanned.
//!
//! Tokens scan once, left to right, so `--version` and `--help` short-circuit
//! where they appear and value errors surface in order. Behavior parity with
//! the removed clap surface (exit codes, streams, rejected forms) is pinned by
//! `tests/modes.rs`.
//!
//! Every flag becomes the typed value its consumer uses: `--proxy` a [`Proxy`],
//! `--resolve` a [`ResolveRule`], `--tls-ca` a parsed [`TrustedCa`]. The scanner
//! is the inbound adapter, so a bad value is a [`CliError`] carrying its cause
//! instead of a string, and `main` only formats it once.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::iter::Peekable;
use std::path::PathBuf;

use browser::{AgentOptions, InvalidCa, InvalidProxy, InvalidResolveRule, Profile, ProfileError};
use browser::{Proxy, TrustedCa};
use logging::Level;

/// Parsed command line: global flags plus an optional subcommand.
pub(crate) struct Cli {
    /// Minimum log level override.
    pub(crate) log_level: Option<Level>,
    /// Log file override, in place of the profile's log under the data home.
    pub(crate) log_file: Option<PathBuf>,
    /// Shorthand for debug logging.
    pub(crate) verbose: bool,
    /// Selected process mode, if any.
    pub(crate) command: Option<Command>,
}

/// One process mode with its validated arguments.
pub(crate) enum Command {
    /// Run the profile daemon until the process exits.
    Daemon {
        /// Named profile.
        profile: Profile,
    },
    /// Run a renderer worker on its private platform channel.
    Renderer,
    /// Serve classic `WebDriver` on a loopback port.
    Webdriver {
        /// Loopback port.
        port: u16,
        /// Named profile.
        profile: Profile,
        /// Network configuration, every flag already parsed. Boxed because an
        /// `AgentOptions` dwarfs every other command's fields.
        network: Box<AgentOptions>,
    },
}

/// What argument scanning selected.
pub(crate) enum Outcome {
    /// Print help and exit successfully.
    Help,
    /// Print the version and exit successfully.
    Version,
    /// Run with this command line.
    Run(Box<Cli>),
}

/// One command-line failure.
#[derive(Debug)]
pub(crate) enum CliError {
    /// A token is not UTF-8.
    NotUtf8,
    /// A value-taking flag had no value, including `--flag=`.
    MissingValue {
        /// Flag name, without dashes.
        flag: &'static str,
        /// Its value placeholder, as the help text spells it.
        placeholder: &'static str,
    },
    /// A flag that may appear once appeared again. `spelling` is the flag and
    /// its placeholder, the way the message quotes it.
    Repeated {
        /// Flag and placeholder, e.g. `--port <PORT>`.
        spelling: &'static str,
    },
    /// A flag without a value was given one.
    UnexpectedValue {
        /// Flag name, without dashes.
        flag: &'static str,
    },
    /// No flag of the selected subcommand names this token.
    UnexpectedArgument(String),
    /// `help` was followed by a name that is not a subcommand.
    UnrecognizedSubcommand(String),
    /// `--port` is not a port number.
    InvalidPort(String),
    /// `--profile` is not a name.
    InvalidProfile(ProfileError),
    /// `--log-level` is not a level.
    InvalidLogLevel(logging::ParseLevelError),
    /// `--resolve` is not a `PATTERN=ADDR` rule.
    InvalidResolve(InvalidResolveRule),
    /// `--proxy` is not a proxy URI.
    InvalidProxy(InvalidProxy),
    /// `--tls-ca` named a file that could not be read or parsed.
    TlsCa {
        /// The named path.
        path: PathBuf,
        /// Which step failed.
        source: TlsCaError,
    },
    /// `webdriver` was selected without `--port`.
    MissingPort,
}

/// The two ways a `--tls-ca` file can fail.
#[derive(Debug)]
pub(crate) enum TlsCaError {
    /// The file could not be read.
    Read(io::Error),
    /// The file is not a PEM certificate.
    Parse(InvalidCa),
}

impl fmt::Display for TlsCaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::Parse(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for TlsCaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::Parse(error) => Some(error),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotUtf8 => {
                formatter.write_str("Invalid UTF-8 was detected in one or more arguments")
            }
            Self::MissingValue { flag, placeholder } => write!(
                formatter,
                "a value is required for '--{flag} {placeholder}' but none was supplied"
            ),
            Self::Repeated { spelling } => {
                write!(
                    formatter,
                    "the argument '{spelling}' was provided more than once"
                )
            }
            Self::UnexpectedValue { flag } => {
                write!(formatter, "unexpected value for '--{flag}'")
            }
            Self::UnexpectedArgument(token) => {
                write!(formatter, "unexpected argument '{token}' found")
            }
            Self::UnrecognizedSubcommand(token) => {
                write!(formatter, "unrecognized subcommand '{token}'")
            }
            Self::InvalidPort(value) => {
                write!(formatter, "invalid value '{value}' for '--port <PORT>'")
            }
            Self::InvalidProfile(error) => error.fmt(formatter),
            Self::InvalidLogLevel(error) => error.fmt(formatter),
            Self::InvalidResolve(error) => error.fmt(formatter),
            Self::InvalidProxy(error) => error.fmt(formatter),
            Self::TlsCa { path, source } => {
                write!(formatter, "--tls-ca {}: {source}", path.display())
            }
            Self::MissingPort => formatter
                .write_str("the following required arguments were not provided:\n  --port <PORT>"),
        }
    }
}

impl std::error::Error for CliError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidProfile(error) => Some(error),
            Self::InvalidLogLevel(error) => Some(error),
            Self::InvalidResolve(error) => Some(error),
            Self::InvalidProxy(error) => Some(error),
            Self::TlsCa { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Full help text, also printed when no subcommand is given.
pub(crate) const HELP: &str = r"The smallest headless browser for AI agents

Usage: tinybrowser [OPTIONS] [COMMAND]

Commands:
  daemon     Run the profile daemon until the process exits
  renderer   Run a renderer worker on its private platform channel
  webdriver  Serve classic `WebDriver` on this loopback port
  help       Print this message

Options:
      --log-level <LEVEL>
          Minimum log level: error, warn, info, debug, or trace [default: info]

          A running daemon keeps its start-up level; `--verbose` is shorthand for `--log-level=debug`.
      --log-file <PATH>
          Write the log file here; without it, a daemon or webdriver logs under the data home
      --verbose
          Shorthand for --log-level=debug
  -v, --version
          Print version
  -h, --help
          Print help
";

/// Scans `args` (without the executable name) into an [`Outcome`].
///
/// # Errors
///
/// Returns the first unrecognized, misplaced, repeated, or invalid argument.
pub(crate) fn parse(args: impl Iterator<Item = OsString>) -> Result<Outcome, CliError> {
    Scanner::new(args).scan()
}

struct Scanner<I: Iterator<Item = OsString>> {
    args: Peekable<I>,
    end_of_flags: bool,
    log_level: Option<Level>,
    log_file: Option<PathBuf>,
    verbose: bool,
    command: Option<Partial>,
}

enum Partial {
    Daemon {
        profile: Option<Profile>,
    },
    Renderer,
    Help,
    Webdriver {
        port: Option<u16>,
        profile: Option<Profile>,
        network: Box<AgentOptions>,
    },
}

impl<I: Iterator<Item = OsString>> Scanner<I> {
    fn new(args: I) -> Self {
        Self {
            args: args.peekable(),
            end_of_flags: false,
            log_level: None,
            log_file: None,
            verbose: false,
            command: None,
        }
    }

    fn scan(mut self) -> Result<Outcome, CliError> {
        while let Some(arg) = self.args.next() {
            let text = arg.to_str().ok_or(CliError::NotUtf8)?;
            if let Some(outcome) = self.token(text)? {
                return Ok(outcome);
            }
        }
        self.finish()
    }

    fn token(&mut self, text: &str) -> Result<Option<Outcome>, CliError> {
        if self.end_of_flags {
            // clap treats tokens after `--` as positional values; there are
            // none, so even a subcommand name is an error.
            return Err(CliError::UnexpectedArgument(text.to_owned()));
        }
        if text == "--" {
            self.end_of_flags = true;
            return Ok(None);
        }
        if let Some(long) = text.strip_prefix("--") {
            return self.long(long);
        }
        if let Some(short) = text.strip_prefix('-')
            && !short.is_empty()
        {
            return Self::short(short);
        }
        self.positional(text)
    }

    fn long(&mut self, long: &str) -> Result<Option<Outcome>, CliError> {
        let (name, inline) = match long.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (long, None),
        };
        match name {
            "log-level" => {
                let raw = self.value("log-level", "<LEVEL>", inline)?;
                if self.log_level.is_some() {
                    return Err(CliError::Repeated {
                        spelling: "--log-level <LEVEL>",
                    });
                }
                self.log_level = Some(raw.parse().map_err(CliError::InvalidLogLevel)?);
                Ok(None)
            }
            "log-file" => {
                let raw = self.value("log-file", "<PATH>", inline)?;
                if self.log_file.is_some() {
                    return Err(CliError::Repeated {
                        spelling: "--log-file <PATH>",
                    });
                }
                self.log_file = Some(PathBuf::from(raw));
                Ok(None)
            }
            "verbose" => {
                if inline.is_some() {
                    return Err(CliError::UnexpectedValue { flag: "verbose" });
                }
                if self.verbose {
                    return Err(CliError::Repeated {
                        spelling: "--verbose",
                    });
                }
                self.verbose = true;
                Ok(None)
            }
            "version" => {
                if inline.is_some() {
                    return Err(CliError::UnexpectedValue { flag: "version" });
                }
                Ok(Some(Outcome::Version))
            }
            "help" => {
                if inline.is_some() {
                    return Err(CliError::UnexpectedValue { flag: "help" });
                }
                Ok(Some(Outcome::Help))
            }
            _ => self.subcommand_flag(name, inline),
        }
    }

    /// Long flags that only one subcommand understands; `long` holds the rest.
    fn subcommand_flag(
        &mut self,
        name: &str,
        inline: Option<&str>,
    ) -> Result<Option<Outcome>, CliError> {
        match name {
            "profile" => {
                let raw = self.value("profile", "<NAME>", inline)?;
                let profile = Profile::parse(&raw).map_err(CliError::InvalidProfile)?;
                match &mut self.command {
                    Some(
                        Partial::Daemon { profile: slot }
                        | Partial::Webdriver { profile: slot, .. },
                    ) => {
                        if slot.is_some() {
                            return Err(CliError::Repeated {
                                spelling: "--profile <NAME>",
                            });
                        }
                        *slot = Some(profile);
                        Ok(None)
                    }
                    _ => Err(unexpected(name)),
                }
            }
            "port" => {
                let raw = self.value("port", "<PORT>", inline)?;
                let port: u16 = raw.parse().map_err(|_| CliError::InvalidPort(raw))?;
                match &mut self.command {
                    Some(Partial::Webdriver { port: slot, .. }) => {
                        if slot.is_some() {
                            return Err(CliError::Repeated {
                                spelling: "--port <PORT>",
                            });
                        }
                        *slot = Some(port);
                        Ok(None)
                    }
                    _ => Err(unexpected(name)),
                }
            }
            "resolve" => {
                let raw = self.value("resolve", "<PATTERN=ADDR>", inline)?;
                match &mut self.command {
                    Some(Partial::Webdriver { network, .. }) => {
                        let rule = raw.parse().map_err(CliError::InvalidResolve)?;
                        network.resolve.push(rule);
                        Ok(None)
                    }
                    _ => Err(unexpected(name)),
                }
            }
            "tls-ca" => {
                let raw = self.value("tls-ca", "<PATH>", inline)?;
                match &mut self.command {
                    Some(Partial::Webdriver { network, .. }) => {
                        network.tls_cas.push(read_ca(&raw)?);
                        Ok(None)
                    }
                    _ => Err(unexpected(name)),
                }
            }
            "proxy" => {
                let raw = self.value("proxy", "<URI>", inline)?;
                let proxy = raw.parse::<Proxy>().map_err(CliError::InvalidProxy)?;
                match &mut self.command {
                    Some(Partial::Webdriver { network, .. }) => {
                        if network.proxy.is_some() {
                            return Err(CliError::Repeated {
                                spelling: "--proxy <URI>",
                            });
                        }
                        network.proxy = Some(proxy);
                        Ok(None)
                    }
                    _ => Err(unexpected(name)),
                }
            }
            _ => Err(CliError::UnexpectedArgument(format!("--{name}"))),
        }
    }

    fn short(short: &str) -> Result<Option<Outcome>, CliError> {
        match short {
            "v" | "V" => Ok(Some(Outcome::Version)),
            "h" => Ok(Some(Outcome::Help)),
            _ => Err(CliError::UnexpectedArgument(format!("-{short}"))),
        }
    }

    fn positional(&mut self, text: &str) -> Result<Option<Outcome>, CliError> {
        if let Some(Partial::Help) = self.command {
            return match text {
                "daemon" | "renderer" | "webdriver" | "help" => Ok(Some(Outcome::Help)),
                _ => Err(CliError::UnrecognizedSubcommand(text.to_owned())),
            };
        }
        if self.command.is_some() {
            return Err(CliError::UnexpectedArgument(text.to_owned()));
        }
        match text {
            "daemon" => {
                self.command = Some(Partial::Daemon { profile: None });
                Ok(None)
            }
            "renderer" => {
                self.command = Some(Partial::Renderer);
                Ok(None)
            }
            "webdriver" => {
                self.command = Some(Partial::Webdriver {
                    port: None,
                    profile: None,
                    network: Box::new(AgentOptions::default()),
                });
                Ok(None)
            }
            "help" => {
                self.command = Some(Partial::Help);
                Ok(None)
            }
            _ => Err(CliError::UnrecognizedSubcommand(text.to_owned())),
        }
    }

    fn value(
        &mut self,
        flag: &'static str,
        placeholder: &'static str,
        inline: Option<&str>,
    ) -> Result<String, CliError> {
        let missing = || CliError::MissingValue { flag, placeholder };
        if let Some(raw) = inline {
            if raw.is_empty() {
                // `--flag=` is never meaningful here, and every value-taking
                // flag wants the same message for it.
                return Err(missing());
            }
            return Ok(raw.to_owned());
        }
        let Some(next) = self.args.peek() else {
            return Err(missing());
        };
        let Some(text) = next.to_str() else {
            return Err(CliError::NotUtf8);
        };
        if text.starts_with('-') {
            return Err(missing());
        }
        match self.args.next() {
            Some(arg) => arg.into_string().map_err(|_| CliError::NotUtf8),
            None => Err(missing()),
        }
    }

    fn finish(self) -> Result<Outcome, CliError> {
        let command = match self.command {
            None => None,
            Some(Partial::Help) => return Ok(Outcome::Help),
            Some(Partial::Daemon { profile }) => Some(Command::Daemon {
                profile: profile.unwrap_or_default(),
            }),
            Some(Partial::Renderer) => Some(Command::Renderer),
            Some(Partial::Webdriver {
                port,
                profile,
                network,
            }) => {
                let port = port.ok_or(CliError::MissingPort)?;
                Some(Command::Webdriver {
                    port,
                    profile: profile.unwrap_or_default(),
                    network,
                })
            }
        };
        Ok(Outcome::Run(Box::new(Cli {
            log_level: self.log_level,
            log_file: self.log_file,
            verbose: self.verbose,
            command,
        })))
    }
}

fn unexpected(name: &str) -> CliError {
    CliError::UnexpectedArgument(format!("--{name}"))
}

/// Reads and parses one `--tls-ca` file.
///
/// The scanner is the inbound adapter, so the certificate is parsed before the
/// browser opens and a bad file is a usage error, not a dial-time failure.
fn read_ca(raw: &str) -> Result<TrustedCa, CliError> {
    let path = PathBuf::from(raw);
    let pem = std::fs::read(&path).map_err(|source| CliError::TlsCa {
        path: path.clone(),
        source: TlsCaError::Read(source),
    })?;
    TrustedCa::from_pem(&pem).map_err(|source| CliError::TlsCa {
        path: path.clone(),
        source: TlsCaError::Parse(source),
    })
}
