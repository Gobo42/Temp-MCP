use std::{env, io::Read, net::IpAddr, path::Path, process::ExitCode, time::Duration};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tapo::responses::{ChildDeviceHubResult, Status};
use tapo::{ApiClient, Error, TapoResponseError};

mod cli;
mod collector;
mod config;
mod setup;
mod storage;

const HELP: &str = "Read-only H100 data access test\n\
Usage: tapo-telemetry --hub IP [--credentials FILE]\n\
Default credentials: config/credentials.json; --hub is required.\n\
Reads hub metadata, current sensor readings, and available history.\n\
Exit: 0 = sensor read succeeded, 1 = failed or partial (including history failure).";

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Credentials {
    username: String,
    password: String,
}

fn parse_credentials(input: &str) -> Result<Credentials, &'static str> {
    let credentials: Credentials = serde_json::from_str(input)
        .map_err(|_| "Credentials must be JSON with string username and password fields")?;
    if credentials.username.trim().is_empty() || credentials.password.trim().is_empty() {
        return Err("Credential username and password must not be blank");
    }
    Ok(credentials)
}

fn read_credentials(path: &Path) -> Result<Credentials, &'static str> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "Cannot open credential file")?;
    let metadata = file
        .metadata()
        .map_err(|_| "Cannot inspect credential file")?;
    if !metadata.is_file() {
        return Err("Credential path must be a regular file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("Credential file must have owner-only permissions (chmod 600)");
        }
    }
    let mut input = String::new();
    file.read_to_string(&mut input)
        .map_err(|_| "Cannot read credential file as UTF-8")?;
    parse_credentials(&input)
}

struct Options {
    hub: IpAddr,
    credentials: std::path::PathBuf,
}

fn options(args: impl Iterator<Item = String>) -> Result<Option<Options>, &'static str> {
    let mut hub = None;
    let mut credentials = std::path::PathBuf::from("config/credentials.json");
    let mut args = args;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--help" | "-h" => return Ok(None),
            "--hub" => {
                hub = Some(
                    args.next()
                        .ok_or("--hub requires an IP address")?
                        .parse()
                        .map_err(|_| "--hub must be an IP address")?,
                );
            }
            "--credentials" => {
                credentials = args
                    .next()
                    .ok_or("--credentials requires a file path")?
                    .into();
            }
            _ => return Err("Unknown argument; use --help"),
        }
    }
    Ok(Some(Options {
        hub: hub.ok_or("Diagnostic requires --hub IP")?,
        credentials,
    }))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let command = match cli::parse(env::args().skip(1).collect()) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let args = match command {
        cli::Command::Help => {
            println!("{}", cli::HELP);
            return ExitCode::SUCCESS;
        }
        cli::Command::Diagnose(args) => args,
        cli::Command::Setup(options) => {
            return match setup::run(options).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(message) => {
                    eprintln!("{message}");
                    ExitCode::FAILURE
                }
            };
        }
        cli::Command::Check { config } => return command_result(collector::check(&config).await),
        cli::Command::Collect { config, once } => {
            return command_result(collector::run(&config, once).await);
        }
    };
    let result = match options(args.into_iter()) {
        Ok(None) => {
            println!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Ok(Some(options)) => match read_credentials(&options.credentials) {
            Ok(credentials) => match tokio::time::timeout(
                Duration::from_secs(60),
                diagnose(options.hub, credentials),
            )
            .await
            {
                Ok(report) => report,
                Err(_) => failure("diagnostic", json!({"kind":"overall_timeout"})),
            },
            Err(message) => failure("credentials", json!({"message":message})),
        },
        Err(message) => failure("arguments", json!({"message":message})),
    };
    let success = result["status"] == "ok";
    println!(
        "{}",
        serde_json::to_string_pretty(&result).expect("JSON value is serializable")
    );
    if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn command_result(result: config::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            if message == collector::AUTH_REJECTED {
                ExitCode::from(78)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

fn failure(stage: &str, error: Value) -> Value {
    json!({"status":"failed", "stage":stage, "error":error})
}

async fn diagnose(ip: IpAddr, credentials: Credentials) -> Value {
    let started_at = Utc::now();
    let hub = match ApiClient::new(credentials.username, credentials.password)
        .with_timeout(Duration::from_secs(10))
        .h100(ip.to_string())
        .await
    {
        Ok(hub) => hub,
        Err(error) => return failure("authentication", safe_error(&error)),
    };
    let info = match hub.get_device_info().await {
        Ok(info) => info,
        Err(error) => return failure("hub_metadata", safe_error(&error)),
    };
    let children = match hub.get_child_device_list().await {
        Ok(children) => children,
        Err(error) => return failure("sensor_discovery", safe_error(&error)),
    };
    let discovery_collected_at = Utc::now();
    let mut sensors = Vec::new();
    let mut has_reading = false;
    let mut partial = false;
    for child in children {
        let mut sensor = json!({
            "device_id":child.device_id(), "model":child.model(), "name":child.nickname(),
        });
        if let ChildDeviceHubResult::T31X(device) = child {
            // The stable ID and model were just verified by discovery.
            let handler = hub.t31x_unchecked(device.device_id.clone());
            match handler.get_device_info().await {
                Ok(reading) => {
                    if reading.status == Status::Online {
                        has_reading = true;
                    } else {
                        partial = true;
                    }
                    sensor["current"] = json!({
                        "collected_at":Utc::now(), "observed_at":null,
                        "temperature":reading.current_temperature,
                        "temperature_unit":reading.temperature_unit,
                        "humidity_percent":reading.current_humidity,
                        "low_battery":reading.at_low_battery,
                        "status":reading.status,
                        "report_interval_seconds":reading.report_interval,
                    });
                }
                Err(error) => {
                    partial = true;
                    sensor["current"] = json!({"status":"failed", "error":safe_error(&error)});
                    if authentication_failure(&error) {
                        sensors.push(sensor);
                        break;
                    }
                }
            }
            match handler.get_temperature_humidity_records().await {
                Ok(history) => {
                    sensor["history"] = json!({
                        "status":"ok", "collected_at":Utc::now(),
                        "hub_retrieved_at":history.retrieved_at,
                        "temperature_unit":history.temperature_unit,
                        "interval_seconds":900,
                        "timestamp_semantics":"Library interprets hub time as UTC and assigns interval starts; latest interval may be incomplete.",
                        "aggregation":"15-minute averages according to upstream; not independently verified.",
                        "record_count":history.records.len(), "records":history.records,
                    });
                }
                Err(error) => {
                    partial = true;
                    sensor["history"] = json!({"status":"failed", "error":safe_error(&error)});
                    if authentication_failure(&error) {
                        sensors.push(sensor);
                        break;
                    }
                }
            }
        } else {
            sensor["current"] = json!({"status":"unsupported_sensor"});
        }
        sensors.push(sensor);
    }
    json!({
        "status":if has_reading && !partial { "ok" } else { "partial" },
        "authenticated":true,
        "library":{"name":"tapo", "version":"0.11.1"},
        "started_at":started_at, "finished_at":Utc::now(),
        "discovery_collected_at":discovery_collected_at,
        "hub":{
            "ip":ip.to_string(), "device_id":info.device_id, "model":info.model,
            "hardware_version":info.hw_ver, "firmware_version":info.fw_ver,
        },
        "has_environmental_reading":has_reading,
        "sensors":sensors,
    })
}

fn authentication_failure(error: &Error) -> bool {
    matches!(error, Error::Tapo(TapoResponseError::Unauthorized { .. }))
}

fn safe_error(error: &Error) -> Value {
    // Descriptions and URLs can contain response or session material.
    match error {
        Error::Tapo(TapoResponseError::Unauthorized { kind, .. }) => json!({"kind":kind}),
        Error::Tapo(TapoResponseError::DeviceError { code, kind }) => {
            json!({"kind":kind,"code":code})
        }
        Error::Http(error) if error.is_timeout() => json!({"kind":"request_timeout"}),
        Error::Http(error) if error.is_connect() => json!({"kind":"connection_failed"}),
        Error::Serde(_) => json!({"kind":"response_parse_failed"}),
        Error::UnsupportedProtocol { protocol, .. } => {
            json!({"kind":"unsupported_protocol", "protocol":protocol})
        }
        _ => json!({"kind":"library_error"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_hub_credentials_have_a_nonrestart_exit_code() {
        assert_eq!(
            command_result(Err(collector::AUTH_REJECTED)),
            ExitCode::from(78)
        );
    }

    #[test]
    fn credentials_preserve_password_and_username() {
        let credentials = parse_credentials(
            r#"{"username":"owner@example.test","password":" space-sensitive "}"#,
        )
        .unwrap_or_else(|message| panic!("{message}"));
        assert_eq!(credentials.username, "owner@example.test");
        assert_eq!(credentials.password, " space-sensitive ");
    }

    #[test]
    fn credentials_reject_missing_wrong_type_and_blank_fields() {
        for input in [
            r#"{}"#,
            r#"{"username":"owner","password":null}"#,
            r#"{"username":false,"password":"secret"}"#,
            r#"{"username":"  ","password":"secret"}"#,
            r#"{"username":"owner","password":"  "}"#,
        ] {
            assert!(parse_credentials(input).is_err());
        }
    }

    #[test]
    fn malformed_credentials_do_not_echo_secrets() {
        let message =
            parse_credentials(r#"{"username":"owner","password":"secret-marker", BROKEN}"#)
                .err()
                .unwrap();
        assert!(!message.contains("secret-marker"));
        assert!(!message.contains("owner"));
    }

    #[test]
    fn authentication_errors_preserve_kind_without_response_material() {
        let error = Error::Tapo(TapoResponseError::Unauthorized {
            kind: "TPAP_CREDENTIALS",
            description: "password=secret-marker; session=token-marker".into(),
        });
        let output = safe_error(&error);
        assert_eq!(output["kind"], "TPAP_CREDENTIALS");
        assert!(!output.to_string().contains("secret-marker"));
        assert!(!output.to_string().contains("token-marker"));
    }

    #[test]
    fn unstructured_errors_are_redacted_and_device_codes_are_preserved() {
        let error = Error::Tapo(TapoResponseError::ResponseError {
            description: "response contains secret-marker".into(),
        });
        assert_eq!(safe_error(&error), json!({"kind":"library_error"}));
        let error = Error::Tapo(TapoResponseError::DeviceError {
            code: -1001,
            kind: "TEST_ERROR",
        });
        assert_eq!(safe_error(&error)["code"], -1001);
    }

    #[test]
    fn diagnostic_requires_explicit_hub_for_ipv4_and_ipv6() {
        assert!(options(std::iter::empty()).is_err());
        assert!(
            options(
                ["--credentials", "credentials.json"]
                    .into_iter()
                    .map(String::from)
            )
            .is_err()
        );
        for address in ["203.0.113.42", "2001:db8::42"] {
            let options = options(["--hub", address].into_iter().map(String::from))
                .unwrap()
                .unwrap();
            assert_eq!(options.hub, address.parse::<IpAddr>().unwrap());
        }
    }

    #[test]
    fn options_validate_ip_and_do_not_echo_unknown_arguments() {
        for args in [
            vec!["--hub"],
            vec!["--hub", "not-an-ip"],
            vec!["--credentials"],
            vec!["secret-marker"],
        ] {
            let message = options(args.into_iter().map(String::from)).err().unwrap();
            assert!(!message.contains("secret-marker"));
        }
        let options = options(
            ["--hub", "127.0.0.1", "--credentials", "a file.json"]
                .into_iter()
                .map(String::from),
        )
        .unwrap()
        .unwrap();
        assert_eq!(options.hub.to_string(), "127.0.0.1");
        assert_eq!(options.credentials, Path::new("a file.json"));
    }

    #[cfg(unix)]
    #[test]
    fn credential_file_rejects_public_permissions_and_nonfiles() {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let path =
            env::temp_dir().join(format!("tapo-credentials-test-{}.json", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        use std::io::Write;
        file.write_all(br#"{"username":"owner","password":"secret-marker"}"#)
            .unwrap();
        assert!(read_credentials(&path).is_ok());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let message = read_credentials(&path).err().unwrap();
        assert!(message.contains("owner-only"));
        assert!(!message.contains("secret-marker"));
        std::fs::remove_file(&path).unwrap();
        assert!(read_credentials(&path).is_err());
        assert!(read_credentials(&env::temp_dir()).is_err());
    }
}
