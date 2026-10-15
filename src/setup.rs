use crate::{
    config::{self, CollectorConfig, MariaDbConfig, Result, SensorConfig},
    read_credentials,
};
use std::{
    io::{BufRead, Write},
    net::IpAddr,
    path::PathBuf,
    time::Duration,
};
use tapo::{ApiClient, responses::ChildDeviceHubResult};

pub struct Options {
    pub output: PathBuf,
    pub existing: Option<PathBuf>,
    pub hub: Option<IpAddr>,
    pub credentials: Option<PathBuf>,
    pub database_credentials: Option<PathBuf>,
}

pub fn prompt(
    input: &mut impl BufRead,
    output: &mut impl Write,
    label: &str,
    default: &str,
) -> Result<String> {
    write!(output, "{label} [{default}]: ")
        .and_then(|_| output.flush())
        .map_err(|_| "Cannot write wizard prompt")?;
    let mut line = String::new();
    if input
        .read_line(&mut line)
        .map_err(|_| "Cannot read wizard input")?
        == 0
    {
        return Err("Setup cancelled: input ended");
    }
    let line = line.trim();
    Ok(if line.is_empty() {
        default.to_owned()
    } else {
        line.to_owned()
    })
}
pub fn confirm(
    input: &mut impl BufRead,
    output: &mut impl Write,
    label: &str,
    default: bool,
) -> Result<bool> {
    loop {
        match prompt(input, output, label, if default { "yes" } else { "no" })?
            .to_ascii_lowercase()
            .as_str()
        {
            "yes" | "y" => return Ok(true),
            "no" | "n" => return Ok(false),
            _ => writeln!(output, "Enter yes or no.").map_err(|_| "Cannot write wizard prompt")?,
        }
    }
}
fn number(
    input: &mut impl BufRead,
    output: &mut impl Write,
    label: &str,
    default: u64,
    maximum: u64,
) -> Result<u64> {
    loop {
        if let Ok(value) = prompt(input, output, label, &default.to_string())?.parse::<u64>()
            && (1..=maximum).contains(&value)
        {
            return Ok(value);
        }
        writeln!(output, "Enter a positive integer no larger than {maximum}.")
            .map_err(|_| "Cannot write wizard prompt")?;
    }
}
pub fn merge_sensors(
    existing: &[SensorConfig],
    discovered: &[SensorConfig],
) -> Result<Vec<SensorConfig>> {
    let mut sensors = existing.to_vec();
    for sensor in discovered {
        if let Some(previous) = sensors.iter().find(|s| s.id == sensor.id) {
            if previous.model != sensor.model {
                return Err("A configured sensor changed model; review the configuration");
            }
        } else {
            let mut sensor = sensor.clone();
            sensor.enabled = false;
            sensors.push(sensor);
        }
    }
    Ok(sensors)
}
pub fn configure(
    input: &mut impl BufRead,
    output: &mut impl Write,
    mut config: CollectorConfig,
    mut db: MariaDbConfig,
) -> Result<Option<(CollectorConfig, MariaDbConfig)>> {
    for sensor in &mut config.sensors {
        writeln!(
            output,
            "\n{} ({}, {})",
            sensor.name, sensor.model, sensor.id
        )
        .map_err(|_| "Cannot write wizard output")?;
        sensor.enabled = confirm(input, output, "Enable this sensor?", sensor.enabled)?;
        sensor.name = prompt(input, output, "Display name", &sensor.name)?;
        let room = prompt(
            input,
            output,
            "Room (use - to clear)",
            sensor.room.as_deref().unwrap_or("-"),
        )?;
        sensor.room = if room == "-" { None } else { Some(room) };
    }
    config.poll_interval_seconds = number(
        input,
        output,
        "Live poll interval (seconds)",
        config.poll_interval_seconds,
        86400,
    )?;
    config.history_interval_seconds = number(
        input,
        output,
        "History sync interval (seconds)",
        config.history_interval_seconds,
        86400,
    )?;
    db.socket = prompt(
        input,
        output,
        "MariaDB socket on automation box",
        db.socket.to_str().ok_or("Socket path is not UTF-8")?,
    )?
    .into();
    db.database = prompt(input, output, "MariaDB database", &db.database)?;
    config.spool_directory = prompt(
        input,
        output,
        "Spool directory on automation box",
        config
            .spool_directory
            .to_str()
            .ok_or("Spool path is not UTF-8")?,
    )?
    .into();
    if !config.spool_directory.is_absolute() {
        return Err("Wizard spool directory must be an absolute target path");
    }
    config.spool_limit_bytes = number(
        input,
        output,
        "Spool capacity (bytes)",
        config.spool_limit_bytes,
        u64::MAX,
    )?;
    config.credentials_file = "credentials.json".into();
    config.mariadb_file = "mariadb.json".into();
    db.credentials_file = "database-credentials.json".into();
    config.validate()?;
    db.validate()?;
    writeln!(output, "\nCollector settings:\n{}\nMariaDB settings:\n{}\nCredentials will be copied into separate protected files.",
        serde_json::to_string_pretty(&config).map_err(|_| "Cannot serialize settings")?, serde_json::to_string_pretty(&db).map_err(|_| "Cannot serialize settings")?).map_err(|_| "Cannot write wizard review")?;
    if confirm(input, output, "Save the four-file configuration?", false)? {
        Ok(Some((config, db)))
    } else {
        Ok(None)
    }
}

pub async fn run(options: Options) -> Result<()> {
    let destination = config::normalize_directory(&options.output)?;
    if std::fs::symlink_metadata(&destination).is_ok() {
        return Err("Output directory already exists; choose a new destination");
    }
    if !destination.parent().is_some_and(|p| p.is_dir()) {
        return Err("Output parent directory must already exist");
    }
    let previous = options
        .existing
        .as_ref()
        .map(|path| config::load(path))
        .transpose()?;
    let hub = options
        .hub
        .or_else(|| previous.as_ref().map(|p| p.config.hub))
        .ok_or("setup requires --hub IP or an existing --config FILE")?;
    let tapo = match options.credentials {
        Some(path) => read_credentials(&path)?,
        None => match &previous {
            Some(p) => p.tapo.clone(),
            None => read_credentials(std::path::Path::new("config/credentials.json"))?,
        },
    };
    let database = match options.database_credentials {
        Some(path) => read_credentials(&path)?,
        None => match &previous {
            Some(p) => p.database.clone(),
            None => read_credentials(std::path::Path::new("config/database-credentials.json"))?,
        },
    };
    println!("Reading environmental sensors from {hub}...");
    let discovered = tokio::time::timeout(Duration::from_secs(60), async {
        let handler = ApiClient::new(tapo.username.clone(), tapo.password.clone())
            .with_timeout(Duration::from_secs(10))
            .h100(hub.to_string())
            .await
            .map_err(|_| "Hub authentication failed")?;
        let children = handler
            .get_child_device_list()
            .await
            .map_err(|_| "Hub discovery failed")?;
        let mut sensors = Vec::new();
        for child in children {
            if let ChildDeviceHubResult::T31X(sensor) = child {
                println!(
                    "{} ({}, {}):",
                    sensor.nickname, sensor.model, sensor.device_id
                );
                match handler
                    .t31x_unchecked(sensor.device_id.clone())
                    .get_device_info()
                    .await
                {
                    Ok(reading) => println!(
                        "  {:?}, {} {:?}, {}% humidity, low battery: {}",
                        reading.status,
                        reading.current_temperature,
                        reading.temperature_unit,
                        reading.current_humidity,
                        reading.at_low_battery
                    ),
                    Err(error) => println!("  reading unavailable: {}", crate::safe_error(&error)),
                }
                sensors.push(SensorConfig {
                    id: sensor.device_id,
                    model: sensor.model,
                    name: sensor.nickname,
                    room: None,
                    enabled: false,
                });
            } else {
                println!("Skipping {} ({})", child.nickname(), child.model());
            }
        }
        Ok::<_, &'static str>(sensors)
    })
    .await
    .map_err(|_| "Hub discovery timed out")??;
    let (mut collector, db) = match previous {
        Some(p) => (p.config, p.mariadb),
        None => (CollectorConfig::new(hub, vec![]), MariaDbConfig::default()),
    };
    collector.hub = hub;
    for sensor in &collector.sensors {
        if !discovered.iter().any(|s| s.id == sensor.id) {
            println!(
                "Keeping previously configured sensor {} (not discovered now)",
                sensor.id
            );
        }
    }
    collector.sensors = merge_sensors(&collector.sensors, &discovered)?;
    if collector.sensors.is_empty() {
        return Err("No supported environmental sensors discovered");
    }
    let result = configure(
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
        collector,
        db,
    )?;
    if let Some((collector, db)) = result {
        config::publish_bundle(&destination, &collector, &db, &tapo, &database)?;
        println!(
            "Saved configuration to {}. Database connection will be checked on the automation box.",
            destination.display()
        );
    } else {
        println!("Setup cancelled; no files saved.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn prompts_cancel_on_eof_and_do_not_default_to_approval() {
        assert!(prompt(&mut Cursor::new(b""), &mut Vec::new(), "Save", "no").is_err());
        assert!(!confirm(&mut Cursor::new(b"\n"), &mut Vec::new(), "Save", false).unwrap());
    }
    #[test]
    fn rediscovery_preserves_existing_and_leaves_new_sensors_disabled() {
        let old = vec![
            SensorConfig {
                id: "existing".into(),
                model: "T315".into(),
                name: "Sensor A".into(),
                room: Some("Room A".into()),
                enabled: true,
            },
            SensorConfig {
                id: "missing".into(),
                model: "T310".into(),
                name: "Room A".into(),
                room: None,
                enabled: true,
            },
        ];
        let discovery = vec![
            SensorConfig {
                id: "existing".into(),
                model: "T315".into(),
                name: "Factory name".into(),
                room: None,
                enabled: false,
            },
            SensorConfig {
                id: "new".into(),
                model: "T315".into(),
                name: "New sensor".into(),
                room: None,
                enabled: true,
            },
        ];
        let merged = merge_sensors(&old, &discovery).unwrap();
        assert_eq!(merged[0].name, "Sensor A");
        assert_eq!(merged[0].room.as_deref(), Some("Room A"));
        assert!(merged[0].enabled);
        assert_eq!(merged[1].id, "missing");
        assert!(!merged[2].enabled);
    }
    #[test]
    fn scripted_review_can_save_or_cancel_without_database_access() {
        let sensors = vec![SensorConfig {
            id: "one".into(),
            model: "T315".into(),
            name: "Sensor".into(),
            room: None,
            enabled: false,
        }];
        let config = CollectorConfig::new("192.0.2.10".parse().unwrap(), sensors);
        let mut output = Vec::new();
        let input = b"yes\nOffice\nStudy\n30\n600\n\n\n\n\nyes\n";
        let result = configure(
            &mut Cursor::new(input),
            &mut output,
            config.clone(),
            MariaDbConfig::default(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(result.0.sensors[0].room.as_deref(), Some("Study"));
        assert_eq!(result.0.poll_interval_seconds, 30);
        assert_eq!(result.1.database, "tapoagent");
        let input = b"yes\n\n\n\n\n\n\n\n\nno\n";
        assert!(
            configure(
                &mut Cursor::new(input),
                &mut Vec::new(),
                config,
                MariaDbConfig::default()
            )
            .unwrap()
            .is_none()
        );
    }
}
