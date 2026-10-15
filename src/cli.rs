use crate::setup;
use std::{collections::HashSet, path::PathBuf};

pub const HELP: &str = "Tapo environmental telemetry\n\
Usage:\n\
  tapo-telemetry diagnose --hub IP [--credentials FILE]\n\
  tapo-telemetry setup --output-dir DIR [--config FILE] [--hub IP]\n\
      [--credentials FILE] [--database-credentials FILE]\n\
  tapo-telemetry check [--config FILE]\n\
  tapo-telemetry collect [--config FILE] [--once]\n\
Default collector config: /etc/tapo-telemetry/collector.json\n\
Initial setup requires --hub IP; --config reuses its saved hub unless overridden.\n\
Setup writes a new directory; use --config to retain settings from an older bundle.\n\
check validates target credentials, socket and schema without collecting or writing.\n\
Legacy diagnostic flags without a subcommand still work.";

pub enum Command {
    Help,
    Diagnose(Vec<String>),
    Setup(setup::Options),
    Check { config: PathBuf },
    Collect { config: PathBuf, once: bool },
}

pub fn parse(mut args: Vec<String>) -> Result<Command, &'static str> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Ok(Command::Help);
    }
    let Some(first) = args.first() else {
        return Ok(Command::Diagnose(args));
    };
    if first == "diagnose" {
        args.remove(0);
        return Ok(Command::Diagnose(args));
    }
    if !matches!(first.as_str(), "setup" | "check" | "collect") {
        return Ok(Command::Diagnose(args));
    }
    let command = args.remove(0);
    let mut setup = setup::Options {
        output: PathBuf::new(),
        existing: None,
        hub: None,
        credentials: None,
        database_credentials: None,
    };
    let mut config = PathBuf::from("/etc/tapo-telemetry/collector.json");
    let mut once = false;
    let mut seen = HashSet::new();
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        if !seen.insert(flag.clone()) {
            return Err("Repeated argument; use --help");
        }
        if flag == "--once" && command == "collect" {
            once = true;
            continue;
        }
        let allowed = if command == "setup" {
            matches!(
                flag.as_str(),
                "--output-dir" | "--config" | "--hub" | "--credentials" | "--database-credentials"
            )
        } else {
            flag == "--config"
        };
        if !allowed {
            return Err("Unknown argument; use --help");
        }
        let value = args
            .next()
            .filter(|v| !v.is_empty() && !v.starts_with("--"))
            .ok_or("Argument requires a value; use --help")?;
        match flag.as_str() {
            "--output-dir" => setup.output = value.into(),
            "--config" if command == "setup" => setup.existing = Some(value.into()),
            "--config" => config = value.into(),
            "--hub" => setup.hub = Some(value.parse().map_err(|_| "--hub must be an IP address")?),
            "--credentials" => setup.credentials = Some(value.into()),
            "--database-credentials" => setup.database_credentials = Some(value.into()),
            _ => unreachable!(),
        }
    }
    Ok(match command.as_str() {
        "setup" => {
            if setup.output.as_os_str().is_empty() {
                return Err("setup requires --output-dir DIR");
            }
            if setup.hub.is_none() && setup.existing.is_none() {
                return Err("setup requires --hub IP or an existing --config FILE");
            }
            Command::Setup(setup)
        }
        "check" => Command::Check { config },
        "collect" => Command::Collect { config, once },
        _ => unreachable!(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn command(args: &[&str]) -> Result<Command, &'static str> {
        parse(args.iter().map(|s| s.to_string()).collect())
    }
    #[test]
    fn initial_setup_requires_hub_and_accepts_config_for_rediscovery() {
        assert!(command(&["setup", "--output-dir", "new-bundle"]).is_err());
        for address in ["203.0.113.42", "2001:db8::42"] {
            match command(&["setup", "--hub", address, "--output-dir", "new-bundle"]).unwrap() {
                Command::Setup(options) => assert_eq!(options.hub, Some(address.parse().unwrap())),
                _ => panic!("wrong command"),
            }
        }
        match command(&[
            "setup",
            "--config",
            "old/collector.json",
            "--output-dir",
            "new-bundle",
        ])
        .unwrap()
        {
            Command::Setup(options) => assert!(options.hub.is_none()),
            _ => panic!("wrong command"),
        }
        assert!(command(&["setup", "--hub", "invalid", "--output-dir", "new-bundle"]).is_err());
    }

    #[test]
    fn setup_requires_output_and_preserves_directory_argument() {
        assert!(command(&["setup"]).is_err());
        assert!(command(&["setup", "--output-dir"]).is_err());
        match command(&[
            "setup",
            "--output-dir",
            "deploy///",
            "--config",
            "old/collector.json",
        ])
        .unwrap()
        {
            Command::Setup(options) => {
                assert_eq!(options.output, std::path::Path::new("deploy///"));
                assert_eq!(
                    options.existing.unwrap(),
                    std::path::Path::new("old/collector.json")
                );
            }
            _ => panic!("wrong command"),
        }
        assert!(command(&["setup", "--output-dir", "one", "--output-dir", "two"]).is_err());
    }
    #[test]
    fn collect_and_check_reject_wrong_flags_without_echoing_values() {
        assert!(matches!(
            command(&["collect", "--once"]).unwrap(),
            Command::Collect { once: true, .. }
        ));
        assert!(command(&["check", "--once"]).is_err());
        let error = command(&["collect", "secret-marker"]).err().unwrap();
        assert!(!error.contains("secret-marker"));
        assert!(matches!(
            command(&["--hub", "127.0.0.1"]).unwrap(),
            Command::Diagnose(_)
        ));
    }
}
