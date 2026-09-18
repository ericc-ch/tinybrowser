//! Command-line boundary: four global flags, three subcommands, hand-scanned.
//!
//! Tokens scan once, left to right, so `--version` and `--help` short-circuit
//! where they appear and value errors surface in order. Behavior parity with
//! the removed clap surface (exit codes, streams, rejected forms) is pinned by
//! `tests/modes.rs`.

use std::ffi::OsString;
use std::iter::Peekable;
use std::path::PathBuf;

use browser::Profile;
use logging::Level;

/// Parsed command line: global flags plus an optional subcommand.
pub(crate) struct Cli {
    /// Minimum log level override.
    pub(crate) log_level: Option<Level>,
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
        /// Host-to-address rewrites.
        resolve: Vec<String>,
        /// Extra PEM certificate authorities.
        tls_ca: Vec<PathBuf>,
    },
}

/// What argument scanning selected.
pub(crate) enum Outcome {
    /// Print help and exit successfully.
    Help,
    /// Print the version and exit successfully.
    Version,
    /// Run with this command line.
    Run(Cli),
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
/// Returns a message for the first unrecognized, misplaced, repeated, or
/// invalid argument.
pub(crate) fn parse(args: impl Iterator<Item = OsString>) -> Result<Outcome, String> {
    Scanner::new(args).scan()
}

struct Scanner<I: Iterator<Item = OsString>> {
    args: Peekable<I>,
    end_of_flags: bool,
    log_level: Option<Level>,
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
        resolve: Vec<String>,
        tls_ca: Vec<PathBuf>,
    },
}

impl<I: Iterator<Item = OsString>> Scanner<I> {
    fn new(args: I) -> Self {
        Self {
            args: args.peekable(),
            end_of_flags: false,
            log_level: None,
            verbose: false,
            command: None,
        }
    }

    fn scan(mut self) -> Result<Outcome, String> {
        while let Some(arg) = self.args.next() {
            let text = arg
                .to_str()
                .ok_or_else(|| "Invalid UTF-8 was detected in one or more arguments".to_owned())?;
            if let Some(outcome) = self.token(text)? {
                return Ok(outcome);
            }
        }
        self.finish()
    }

    fn token(&mut self, text: &str) -> Result<Option<Outcome>, String> {
        if self.end_of_flags {
            // clap treats tokens after `--` as positional values; there are
            // none, so even a subcommand name is an error.
            return Err(format!("unexpected argument '{text}' found"));
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

    fn long(&mut self, long: &str) -> Result<Option<Outcome>, String> {
        let (name, inline) = match long.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (long, None),
        };
        match name {
            "log-level" => {
                let raw = self.value("log-level", "<LEVEL>", inline)?;
                if self.log_level.is_some() {
                    return Err(
                        "the argument '--log-level <LEVEL>' was provided more than once".into(),
                    );
                }
                self.log_level = Some(
                    raw.parse()
                        .map_err(|error: logging::ParseLevelError| error.to_string())?,
                );
                Ok(None)
            }
            "verbose" => {
                if inline.is_some() {
                    return Err("unexpected value for '--verbose'".into());
                }
                if self.verbose {
                    return Err("the argument '--verbose' was provided more than once".into());
                }
                self.verbose = true;
                Ok(None)
            }
            "version" => {
                if inline.is_some() {
                    return Err("unexpected value for '--version'".into());
                }
                Ok(Some(Outcome::Version))
            }
            "help" => {
                if inline.is_some() {
                    return Err("unexpected value for '--help'".into());
                }
                Ok(Some(Outcome::Help))
            }
            "profile" => {
                let raw = self.value("profile", "<NAME>", inline)?;
                let profile = Profile::parse(&raw).map_err(|error| error.to_string())?;
                match &mut self.command {
                    Some(
                        Partial::Daemon { profile: slot }
                        | Partial::Webdriver { profile: slot, .. },
                    ) => {
                        if slot.is_some() {
                            return Err(
                                "the argument '--profile <NAME>' was provided more than once"
                                    .into(),
                            );
                        }
                        *slot = Some(profile);
                        Ok(None)
                    }
                    _ => Err("unexpected argument '--profile' found".into()),
                }
            }
            "port" => {
                let raw = self.value("port", "<PORT>", inline)?;
                let port: u16 = raw
                    .parse()
                    .map_err(|_| format!("invalid value '{raw}' for '--port <PORT>'"))?;
                match &mut self.command {
                    Some(Partial::Webdriver { port: slot, .. }) => {
                        if slot.is_some() {
                            return Err(
                                "the argument '--port <PORT>' was provided more than once".into()
                            );
                        }
                        *slot = Some(port);
                        Ok(None)
                    }
                    _ => Err("unexpected argument '--port' found".into()),
                }
            }
            "resolve" => {
                let raw = self.value("resolve", "<PATTERN=ADDR>", inline)?;
                match &mut self.command {
                    Some(Partial::Webdriver { resolve, .. }) => {
                        resolve.push(raw);
                        Ok(None)
                    }
                    _ => Err("unexpected argument '--resolve' found".into()),
                }
            }
            "tls-ca" => {
                let raw = self.value("tls-ca", "<PATH>", inline)?;
                match &mut self.command {
                    Some(Partial::Webdriver { tls_ca, .. }) => {
                        tls_ca.push(PathBuf::from(raw));
                        Ok(None)
                    }
                    _ => Err("unexpected argument '--tls-ca' found".into()),
                }
            }
            _ => Err(format!("unexpected argument '--{name}' found")),
        }
    }

    fn short(short: &str) -> Result<Option<Outcome>, String> {
        match short {
            "v" | "V" => Ok(Some(Outcome::Version)),
            "h" => Ok(Some(Outcome::Help)),
            _ => Err(format!("unexpected argument '-{short}' found")),
        }
    }

    fn positional(&mut self, text: &str) -> Result<Option<Outcome>, String> {
        if let Some(Partial::Help) = self.command {
            return match text {
                "daemon" | "renderer" | "webdriver" | "help" => Ok(Some(Outcome::Help)),
                _ => Err(format!("unrecognized subcommand '{text}'")),
            };
        }
        if self.command.is_some() {
            return Err(format!("unexpected argument '{text}' found"));
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
                    resolve: Vec::new(),
                    tls_ca: Vec::new(),
                });
                Ok(None)
            }
            "help" => {
                self.command = Some(Partial::Help);
                Ok(None)
            }
            _ => Err(format!("unrecognized subcommand '{text}'")),
        }
    }

    fn value(
        &mut self,
        flag: &str,
        placeholder: &str,
        inline: Option<&str>,
    ) -> Result<String, String> {
        if let Some(raw) = inline {
            return Ok(raw.to_owned());
        }
        let missing =
            || format!("a value is required for '--{flag} {placeholder}' but none was supplied");
        let Some(next) = self.args.peek() else {
            return Err(missing());
        };
        let Some(text) = next.to_str() else {
            return Err("Invalid UTF-8 was detected in one or more arguments".into());
        };
        if text.starts_with('-') {
            return Err(missing());
        }
        match self.args.next() {
            Some(arg) => arg
                .into_string()
                .map_err(|_| "Invalid UTF-8 was detected in one or more arguments".into()),
            None => Err(missing()),
        }
    }

    fn finish(self) -> Result<Outcome, String> {
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
                resolve,
                tls_ca,
            }) => {
                let Some(port) = port else {
                    return Err(
                        "the following required arguments were not provided:\n  --port <PORT>"
                            .into(),
                    );
                };
                Some(Command::Webdriver {
                    port,
                    profile: profile.unwrap_or_default(),
                    resolve,
                    tls_ca,
                })
            }
        };
        Ok(Outcome::Run(Cli {
            log_level: self.log_level,
            verbose: self.verbose,
            command,
        }))
    }
}
