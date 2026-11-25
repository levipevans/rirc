//! Command line configuration.

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub nick: String,
    pub sasl: Option<(String, String)>,
    pub pass: Option<String>,
    pub channels: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub demo: bool,
    pub snapshot: bool,
    pub help: bool,
    pub version: bool,
    pub nick: String,
    pub start: Option<ServerConfig>,
    pub plugins_enabled: bool,
    pub plugin_dirs: Vec<String>,
}

pub const USAGE: &str = "\
rirc — a vim-flavoured IRCv3 client for the terminal

USAGE:
    rirc [OPTIONS]

OPTIONS:
    -s, --server <HOST[:PORT]>   Connect on startup (TLS by default)
    -n, --nick <NAME>            Nickname (default: $RIRC_NICK or $USERNAME)
        --port <PORT>            Port override (implies plaintext for 6667)
        --tls                    Force TLS
        --no-tls                 Disable TLS
        --sasl <USER:PASS>       SASL PLAIN credentials. With no PASS, the
                                 password is read from $RIRC_PASS
        --pass <PASS>            Server password (PASS command)
    -c, --channel <CHANNEL>      Auto-join; repeat or comma-separate
        --plugin-dir <DIR>       Extra plugin directory; repeatable
        --no-plugins             Disable the Python plugin host
        --demo                   Start with a local demo session
        --snapshot               Render a demo frame to stdout and exit
    -h, --help                   Show this help
    -V, --version                Print version

EXAMPLES:
    rirc -s irc.libera.chat -n mynick -c '#rust,#vim'
    rirc --demo
";

fn sanitize_nick(nick: &str) -> String {
    let cleaned: String = nick
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    let trimmed = cleaned.trim_start_matches(|c: char| c.is_ascii_digit());
    if trimmed.is_empty() {
        "rirc".to_string()
    } else {
        trimmed.chars().take(24).collect()
    }
}

fn default_nick() -> String {
    for key in ["RIRC_NICK", "USERNAME", "USER"] {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                return sanitize_nick(&value);
            }
        }
    }
    "rirc".to_string()
}

fn split_host_port(spec: &str) -> (String, Option<u16>) {
    if let Some((host, port)) = spec.rsplit_once(':') {
        if !host.contains(':') {
            if let Ok(port) = port.parse::<u16>() {
                return (host.to_string(), Some(port));
            }
        }
    }
    (spec.to_string(), None)
}

fn channels_from(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty() && !c.chars().any(char::is_control))
        .map(|c| {
            if c.starts_with('#') || c.starts_with('&') || c.starts_with('+') || c.starts_with('!')
            {
                c.to_string()
            } else {
                format!("#{c}")
            }
        })
        .collect()
}

pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<Config, String> {
    let mut config = Config {
        nick: default_nick(),
        plugins_enabled: true,
        ..Default::default()
    };

    let mut server: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut tls: Option<bool> = None;
    let mut channels: Vec<String> = Vec::new();
    let mut sasl: Option<(String, String)> = None;
    let mut pass: Option<String> = None;
    let mut nick: Option<String> = None;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => config.help = true,
            "-V" | "--version" => config.version = true,
            "--demo" => config.demo = true,
            "--snapshot" => config.snapshot = true,
            "--no-plugins" => config.plugins_enabled = false,
            "--plugin-dir" => {
                config.plugin_dirs.push(
                    iter.next()
                        .ok_or_else(|| "--plugin-dir needs a value".to_string())?,
                );
            }
            "--tls" => tls = Some(true),
            "--no-tls" => tls = Some(false),
            "-s" | "--server" => {
                server = Some(
                    iter.next()
                        .ok_or_else(|| "--server needs a value".to_string())?,
                );
            }
            "-c" | "--channel" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--channel needs a value".to_string())?;
                channels.extend(channels_from(&value));
            }
            "-n" | "--nick" => {
                nick = Some(
                    iter.next()
                        .ok_or_else(|| "--nick needs a value".to_string())?,
                );
            }
            "--port" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--port needs a value".to_string())?;
                port = Some(value.parse().map_err(|_| format!("bad port: {value}"))?);
            }
            "--sasl" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--sasl needs USER:PASS".to_string())?;
                let (user, password) = match value.split_once(':') {
                    Some((user, password)) => (user.to_string(), password.to_string()),
                    None => (value.clone(), String::new()),
                };
                let password = if password.is_empty() {
                    std::env::var("RIRC_PASS").unwrap_or_default()
                } else {
                    password
                };
                if user.is_empty() || password.is_empty() {
                    return Err("--sasl needs USER:PASS (or $RIRC_PASS)".to_string());
                }
                sasl = Some((user, password));
            }
            "--pass" => {
                pass = Some(
                    iter.next()
                        .ok_or_else(|| "--pass needs a value".to_string())?,
                );
            }
            other => {
                if let Some(value) = other.strip_prefix("--server=") {
                    server = Some(value.to_string());
                } else if let Some(value) = other.strip_prefix("--nick=") {
                    nick = Some(value.to_string());
                } else if let Some(value) = other.strip_prefix("--channel=") {
                    channels.extend(channels_from(value));
                } else if let Some(value) = other.strip_prefix("--plugin-dir=") {
                    config.plugin_dirs.push(value.to_string());
                } else if other.starts_with('-') {
                    return Err(format!("unknown option: {other}"));
                } else if server.is_none() {
                    server = Some(other.to_string());
                } else {
                    return Err(format!("unexpected argument: {other}"));
                }
            }
        }
    }

    if let Some(nick) = nick {
        config.nick = sanitize_nick(&nick);
    }

    if let Some(spec) = server {
        let (host, spec_port) = split_host_port(&spec);
        if host.is_empty() {
            return Err("empty server host".to_string());
        }
        let want_tls = tls.unwrap_or(true);
        let port = port
            .or(spec_port)
            .unwrap_or(if want_tls { 6697 } else { 6667 });
        let tls = tls.unwrap_or(port != 6667);
        if !tls && (sasl.is_some() || pass.is_some()) {
            return Err(
                "refusing to send credentials over a plaintext connection; use --tls".to_string(),
            );
        }
        if pass
            .as_deref()
            .map(|value| value.chars().any(char::is_control))
            .unwrap_or(false)
        {
            return Err("--pass contains control characters".to_string());
        }
        if let Some((user, secret)) = &sasl {
            if user.chars().any(char::is_control) || secret.chars().any(char::is_control) {
                return Err("--sasl contains control characters".to_string());
            }
        }
        config.start = Some(ServerConfig {
            host,
            port,
            tls,
            nick: config.nick.clone(),
            sasl,
            pass,
            channels,
        });
    } else if !channels.is_empty() {
        return Err("--channel needs --server".to_string());
    } else if sasl.is_some() || pass.is_some() {
        return Err("--sasl and --pass need --server".to_string());
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_server_and_channels() {
        let cfg = parse_args(args(&[
            "-s",
            "irc.libera.chat:6697",
            "-n",
            "tester",
            "-c",
            "#rust,#vim",
        ]))
        .unwrap();
        let server = cfg.start.unwrap();
        assert_eq!(server.host, "irc.libera.chat");
        assert_eq!(server.port, 6697);
        assert!(server.tls);
        assert_eq!(server.channels, vec!["#rust", "#vim"]);
        assert_eq!(server.nick, "tester");
    }

    #[test]
    fn port_6667_is_plain() {
        let cfg = parse_args(args(&["-s", "example.org:6667"])).unwrap();
        assert!(!cfg.start.unwrap().tls);
    }

    #[test]
    fn no_tls_defaults_to_6667() {
        let cfg = parse_args(args(&["-s", "example.org", "--no-tls"])).unwrap();
        let server = cfg.start.unwrap();
        assert_eq!(server.port, 6667);
        assert!(!server.tls);
    }

    #[test]
    fn sanitizes_nick() {
        let cfg = parse_args(args(&["--nick", "9weird nick!"])).unwrap();
        assert_eq!(cfg.nick, "weirdnick");
    }

    #[test]
    fn refuses_credentials_without_tls() {
        assert!(parse_args(args(
            &["-s", "example.org", "--no-tls", "--pass", "secret",]
        ))
        .is_err());
        assert!(parse_args(args(&["-s", "example.org:6667", "--sasl", "user:secret",])).is_err());
        assert!(parse_args(args(&["--pass", "secret"])).is_err());
    }

    #[test]
    fn allows_credentials_with_tls() {
        let cfg = parse_args(args(&["-s", "example.org", "--pass", "secret"])).unwrap();
        assert!(cfg.start.unwrap().tls);
    }

    #[test]
    fn parses_plugin_options() {
        let cfg = parse_args(args(&["--plugin-dir", "one", "--plugin-dir=two"])).unwrap();
        assert!(cfg.plugins_enabled);
        assert_eq!(cfg.plugin_dirs, vec!["one", "two"]);

        let cfg = parse_args(args(&["--no-plugins"])).unwrap();
        assert!(!cfg.plugins_enabled);
    }
}
