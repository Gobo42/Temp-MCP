use crate::{
    config::{self, Loaded, Result},
    storage::{Batch, Database, Measurement, SensorBatch, Spool, batch_id, measurement_key},
};
use chrono::{DateTime, Utc};
use std::{future::Future, path::Path, time::Duration};
use tapo::{
    ApiClient,
    responses::{ChildDeviceHubResult, Status, TemperatureUnit},
};
use tokio::time::Instant;

pub(crate) const AUTH_REJECTED: &str =
    "Hub authentication rejected; verify credentials before restarting";

pub fn celsius(value: f32, unit: &TemperatureUnit) -> Result<f64> {
    let value = f64::from(value);
    let value = match unit {
        TemperatureUnit::Celsius => value,
        TemperatureUnit::Fahrenheit => (value - 32.0) * 5.0 / 9.0,
    };
    if !value.is_finite() || !(-273.15..=999.999).contains(&value) {
        return Err("Invalid temperature reading");
    }
    Ok(value)
}
pub fn complete_interval(start: DateTime<Utc>, retrieved: DateTime<Utc>) -> bool {
    start
        .checked_add_signed(chrono::Duration::seconds(900))
        .is_some_and(|end| end <= retrieved)
}
pub fn next_deadline(previous: Instant, finished: Instant, seconds: u64) -> Instant {
    let planned = previous + Duration::from_secs(seconds);
    if planned > finished {
        planned
    } else {
        finished + Duration::from_secs(seconds)
    }
}
async fn request<T>(
    future: impl Future<Output = std::result::Result<T, tapo::Error>>,
) -> Result<T> {
    match tokio::time::timeout(Duration::from_secs(30), future).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) if crate::authentication_failure(&error) => Err(AUTH_REJECTED),
        Ok(Err(_)) => Err("Hub request failed"),
        Err(_) => Err("Hub request timed out"),
    }
}
struct Poll {
    batch: Batch,
    auth_rejected: bool,
}
async fn poll(loaded: &Loaded, live: bool, history_due: bool) -> Result<Poll> {
    let config = &loaded.config;
    let hub = request(
        ApiClient::new(loaded.tapo.username.clone(), loaded.tapo.password.clone())
            .with_timeout(Duration::from_secs(10))
            .h100(config.hub.to_string()),
    )
    .await?;
    let hub_info = request(hub.get_device_info()).await?;
    let children = request(hub.get_child_device_list()).await?;
    let mut batch = Batch {
        version: 1,
        id: batch_id()?,
        hub_id: hub_info.device_id,
        collected_at: Utc::now(),
        sensors: Vec::new(),
    };
    let mut auth_rejected = false;
    for sensor in config.sensors.iter().filter(|s| s.enabled) {
        let mut item = SensorBatch {
            sensor: sensor.clone(),
            success: true,
            error: None,
            measurements: Vec::new(),
        };
        let device = children.iter().find(|d| d.device_id() == sensor.id);
        let available =
            matches!(device, Some(ChildDeviceHubResult::T31X(d)) if d.model == sensor.model);
        if auth_rejected || !available {
            item.success = false;
            item.error = Some(
                if auth_rejected {
                    AUTH_REJECTED
                } else {
                    "Configured environmental sensor is missing or its model changed"
                }
                .into(),
            );
            batch.sensors.push(item);
            continue;
        }
        let handler = hub.t31x_unchecked(sensor.id.clone());
        if live {
            let result = match request(handler.get_device_info()).await {
                Ok(reading) if reading.status != Status::Online => {
                    Err("Environmental sensor is offline")
                }
                Ok(reading) => {
                    if reading.current_humidity > 100 {
                        Err("Invalid humidity reading")
                    } else {
                        celsius(reading.current_temperature, &reading.temperature_unit).map(
                            |temperature_c| {
                                item.measurements.push(Measurement {
                                    key: measurement_key(
                                        "live",
                                        &batch.hub_id,
                                        &sensor.id,
                                        &batch.id,
                                    ),
                                    observed_at: None,
                                    collected_at: Utc::now(),
                                    temperature_c,
                                    humidity_pct: f64::from(reading.current_humidity),
                                    low_battery: Some(reading.at_low_battery),
                                    source: "live".into(),
                                    resolution_seconds: None,
                                });
                            },
                        )
                    }
                }
                Err(message) => Err(message),
            };
            if let Err(message) = result {
                item.success = false;
                item.error = Some(message.into());
                auth_rejected |= message == AUTH_REJECTED;
            }
        }
        if history_due && !auth_rejected {
            match request(handler.get_temperature_humidity_records()).await {
                Ok(history) => {
                    let collected_at = Utc::now();
                    for record in history.records {
                        if !complete_interval(record.recorded_at, history.retrieved_at) {
                            continue;
                        }
                        match celsius(record.temperature, &history.temperature_unit) {
                            Ok(temperature_c) if record.humidity <= 100 => {
                                item.measurements.push(Measurement {
                                    key: measurement_key(
                                        "history",
                                        &batch.hub_id,
                                        &sensor.id,
                                        &format!("{}:900", record.recorded_at.timestamp()),
                                    ),
                                    observed_at: Some(record.recorded_at),
                                    collected_at,
                                    temperature_c,
                                    humidity_pct: f64::from(record.humidity),
                                    low_battery: None,
                                    source: "history".into(),
                                    resolution_seconds: Some(900),
                                })
                            }
                            _ => {
                                item.success = false;
                                item.error = Some("Invalid historical reading".into());
                            }
                        }
                    }
                }
                Err(message) => {
                    item.success = false;
                    item.error = Some(message.into());
                    auth_rejected |= message == AUTH_REJECTED;
                }
            }
        }
        batch.sensors.push(item);
    }
    batch.validate()?;
    Ok(Poll {
        batch,
        auth_rejected,
    })
}

pub async fn check(path: &Path) -> Result<()> {
    let loaded = config::load(path)?;
    Database::new(&loaded)?.validate().await?;
    println!(
        "Configuration, protected credentials, MariaDB socket authentication and schema version 1 are valid."
    );
    Ok(())
}
pub async fn run(path: &Path, once: bool) -> Result<()> {
    let loaded = config::load(path)?;
    let database = Database::new(&loaded)?;
    database.validate().await?;
    let mut spool = Spool::open(&loaded.spool, loaded.config.spool_limit_bytes)?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|_| "Cannot register shutdown signal")?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|_| "Cannot register shutdown signal")?;
    if let Err(message) = database.drain(&spool).await {
        eprintln!("{message}");
    }
    println!(
        "Collector started for {} enabled sensors.",
        loaded.config.sensors.iter().filter(|s| s.enabled).count()
    );
    let mut live_at = Instant::now();
    let mut history_at = live_at;
    loop {
        let now = Instant::now();
        let live = now >= live_at;
        let history_due = now >= history_at;
        let mut failed = false;
        let mut auth_rejected = false;
        if live || history_due {
            match poll(&loaded, live, history_due).await {
                Ok(result) => {
                    failed = result.batch.sensors.iter().any(|s| !s.success);
                    auth_rejected = result.auth_rejected;
                    for sensor in &result.batch.sensors {
                        if let Some(error) = &sensor.error {
                            eprintln!("Sensor {}: {error}", sensor.sensor.id);
                        }
                    }
                    let count: usize = result
                        .batch
                        .sensors
                        .iter()
                        .map(|s| s.measurements.len())
                        .sum();
                    spool.enqueue(&result.batch)?;
                    println!(
                        "Saved durable batch {} with {count} measurements.",
                        result.batch.id
                    );
                }
                Err(message) => {
                    eprintln!("{message}");
                    failed = true;
                    auth_rejected = message == AUTH_REJECTED;
                }
            }
            let finished = Instant::now();
            if live {
                live_at = next_deadline(live_at, finished, loaded.config.poll_interval_seconds);
            }
            if history_due {
                history_at =
                    next_deadline(history_at, finished, loaded.config.history_interval_seconds);
            }
        }
        match database.drain(&spool).await {
            Ok(count) if count > 0 => println!("Committed {count} batches to MariaDB."),
            Ok(_) => {}
            Err(message) => {
                eprintln!("{message}");
                failed = true;
            }
        }
        if auth_rejected {
            return Err(AUTH_REJECTED);
        }
        if once {
            return if failed {
                Err("Collection cycle was partial; inspect errors and pending spool batches")
            } else {
                Ok(())
            };
        }
        tokio::select! {
            _ = tokio::time::sleep_until(live_at.min(history_at)) => {},
            _ = interrupt.recv() => break,
            _ = terminate.recv() => break,
        }
    }
    println!("Collector stopped; pending batches remain in the spool.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    #[test]
    fn temperatures_normalize_and_reject_nonfinite_or_out_of_range_values() {
        assert!(
            (celsius(73.0, &TemperatureUnit::Fahrenheit).unwrap() - 22.7777777778).abs() < 0.00001
        );
        assert_eq!(celsius(20.0, &TemperatureUnit::Celsius).unwrap(), 20.0);
        assert!(celsius(f32::NAN, &TemperatureUnit::Celsius).is_err());
        assert!(celsius(-300.0, &TemperatureUnit::Celsius).is_err());
    }
    #[test]
    fn incomplete_history_is_not_persisted_until_interval_finishes() {
        let start = Utc.with_ymd_and_hms(2026, 10, 12, 12, 0, 0).unwrap();
        assert!(!complete_interval(
            start,
            start + chrono::Duration::seconds(899)
        ));
        assert!(complete_interval(
            start,
            start + chrono::Duration::seconds(900)
        ));
    }
    #[test]
    fn schedules_skip_missed_ticks_without_overlapping_or_hot_loop() {
        let now = tokio::time::Instant::now();
        assert_eq!(
            next_deadline(now, now + Duration::from_secs(10), 60),
            now + Duration::from_secs(60)
        );
        assert_eq!(
            next_deadline(now, now + Duration::from_secs(80), 60),
            now + Duration::from_secs(140)
        );
    }
}
