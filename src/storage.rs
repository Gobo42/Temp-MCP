use crate::{
    Credentials,
    config::{self, Loaded, Result, SensorConfig},
};
use chrono::{DateTime, Datelike, Utc};
use mysql_async::{Conn, OptsBuilder, TxOpts, params, prelude::Queryable};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Measurement {
    pub key: String,
    pub observed_at: Option<DateTime<Utc>>,
    pub collected_at: DateTime<Utc>,
    pub temperature_c: f64,
    pub humidity_pct: f64,
    pub low_battery: Option<bool>,
    pub source: String,
    pub resolution_seconds: Option<u32>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SensorBatch {
    pub sensor: SensorConfig,
    pub success: bool,
    pub error: Option<String>,
    pub measurements: Vec<Measurement>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Batch {
    pub version: u32,
    pub id: String,
    pub hub_id: String,
    pub collected_at: DateTime<Utc>,
    pub sensors: Vec<SensorBatch>,
}
fn valid_time(time: DateTime<Utc>) -> bool {
    (1000..=9999).contains(&time.year())
}
impl Batch {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || !config::valid_id(&self.id)
            || !config::valid_id(&self.hub_id)
            || !valid_time(self.collected_at)
        {
            return Err("Invalid spool batch metadata");
        }
        let config = config::CollectorConfig::new(
            "127.0.0.1".parse().unwrap(),
            self.sensors.iter().map(|s| s.sensor.clone()).collect(),
        );
        config.validate()?;
        let mut keys = std::collections::HashSet::new();
        for sensor in &self.sensors {
            if sensor.error.as_ref().is_some_and(|s| s.len() > 2048) {
                return Err("Invalid sensor error in spool batch");
            }
            for reading in &sensor.measurements {
                if !config::valid_id(&reading.key)
                    || !keys.insert(&reading.key)
                    || !valid_time(reading.collected_at)
                    || reading.observed_at.is_some_and(|t| !valid_time(t))
                    || !reading.temperature_c.is_finite()
                    || !(-273.15..=999.999).contains(&reading.temperature_c)
                    || !reading.humidity_pct.is_finite()
                    || !(0.0..=100.0).contains(&reading.humidity_pct)
                    || match reading.source.as_str() {
                        "live" => {
                            reading.observed_at.is_some() || reading.resolution_seconds.is_some()
                        }
                        "history" => {
                            reading.observed_at.is_none() || reading.resolution_seconds != Some(900)
                        }
                        _ => true,
                    }
                {
                    return Err("Invalid measurement in spool batch");
                }
            }
        }
        Ok(())
    }
}
pub fn batch_id() -> Result<String> {
    let mut random = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut random))
        .map_err(|_| "Cannot generate batch identity")?;
    Ok(format!(
        "{:020}-{}",
        Utc::now().timestamp_micros(),
        random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    ))
}
pub fn measurement_key(source: &str, hub: &str, sensor: &str, identity: &str) -> String {
    let mut hash = Sha256::new();
    for part in [source, hub, sensor, identity] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

pub struct Spool {
    directory: PathBuf,
    limit: u64,
    _lock: File,
}
impl Spool {
    pub fn open(path: &Path, limit: u64) -> Result<Self> {
        if limit == 0 {
            return Err("Spool capacity must be positive");
        }
        let directory = config::normalize_directory(path)?;
        if !directory.exists() {
            config::private_directory(&directory)?;
            config::sync_directory(directory.parent().ok_or("Spool needs a parent directory")?)?;
        }
        let meta =
            fs::symlink_metadata(&directory).map_err(|_| "Cannot inspect spool directory")?;
        if !meta.is_dir()
            || meta.permissions().mode() & 0o077 != 0
            || meta.uid() != unsafe { libc::geteuid() }
        {
            return Err(
                "Spool must be a private directory owned by the collector user (chmod 700)",
            );
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(directory.join(".lock"))
            .map_err(|_| "Cannot open spool lock")?;
        if !lock
            .metadata()
            .map_err(|_| "Cannot inspect spool lock")?
            .is_file()
        {
            return Err("Spool lock must be a regular file");
        }
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("Another collector is using this spool");
        }
        let spool = Self {
            directory,
            limit,
            _lock: lock,
        };
        let probe = spool.directory.join(format!(".probe-{}", batch_id()?));
        config::write_new(&probe, b"")?;
        fs::remove_file(probe).map_err(|_| "Cannot remove spool probe")?;
        config::sync_directory(&spool.directory)?;
        spool.pending()?;
        Ok(spool)
    }
    fn entries(&self) -> Result<Vec<PathBuf>> {
        let mut paths = Vec::new();
        for entry in fs::read_dir(&self.directory).map_err(|_| "Cannot read spool directory")? {
            let entry = entry.map_err(|_| "Cannot read spool entry")?;
            let meta =
                fs::symlink_metadata(entry.path()).map_err(|_| "Cannot inspect spool entry")?;
            if !meta.is_file() || meta.file_type().is_symlink() {
                return Err("Spool entries must be regular files; symlinks are refused");
            }
            paths.push(entry.path());
        }
        paths.sort();
        Ok(paths)
    }
    pub fn enqueue(&mut self, batch: &Batch) -> Result<()> {
        batch.validate()?;
        let bytes = serde_json::to_vec(batch).map_err(|_| "Cannot serialize spool batch")?;
        let used = self.entries()?.iter().try_fold(0u64, |sum, p| {
            let size = fs::metadata(p)
                .map_err(|_| "Cannot inspect spool size")?
                .len();
            sum.checked_add(size).ok_or("Spool size overflow")
        })?;
        if used.saturating_add(bytes.len() as u64) > self.limit {
            return Err("Spool capacity exhausted; collection stopped, pending data retained");
        }
        let temporary = self.directory.join(format!(".{}.tmp", batch.id));
        let destination = self.directory.join(format!("{}.json", batch.id));
        config::write_new(&temporary, &bytes)?;
        let result = config::rename_new(&temporary, &destination)
            .and_then(|_| config::sync_directory(&self.directory));
        if temporary.exists() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
    pub fn pending(&self) -> Result<Vec<(PathBuf, Batch)>> {
        let mut batches = Vec::new();
        for path in self.entries()? {
            if path.extension().is_some_and(|ext| ext == "json") {
                let batch: Batch = config::read_json(&path)?;
                batch.validate()?;
                if path.file_stem().and_then(|s| s.to_str()) != Some(batch.id.as_str()) {
                    return Err("Spool batch identity does not match its filename");
                }
                batches.push((path, batch));
            }
        }
        Ok(batches)
    }
    pub fn acknowledge(&self, path: &Path) -> Result<()> {
        if path.parent() != Some(self.directory.as_path())
            || path.extension().is_none_or(|e| e != "json")
        {
            return Err("Cannot acknowledge a path outside the spool");
        }
        let meta = fs::symlink_metadata(path).map_err(|_| "Cannot inspect acknowledged batch")?;
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err("Acknowledged batch must be a regular file");
        }
        fs::remove_file(path).map_err(|_| "Cannot remove committed batch")?;
        config::sync_directory(&self.directory)
    }
}

pub struct Database {
    options: OptsBuilder,
}
impl Database {
    pub fn new(loaded: &Loaded) -> Result<Self> {
        loaded.mariadb.validate()?;
        let Credentials { username, password } = &loaded.database;
        Ok(Self { options: OptsBuilder::default().socket(Some(loaded.mariadb.socket.to_str().ok_or("Socket path is not UTF-8")?))
            .user(Some(username.clone())).pass(Some(password.clone())).db_name(Some(loaded.mariadb.database.clone()))
            .init(vec!["SET SESSION time_zone = '+00:00'", "SET SESSION sql_mode = 'STRICT_ALL_TABLES,NO_ZERO_DATE,NO_ZERO_IN_DATE,ERROR_FOR_DIVISION_BY_ZERO'", "SET SESSION innodb_lock_wait_timeout = 10"]) })
    }
    pub async fn validate(&self) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(15), async {
            let mut conn = Conn::new(self.options.clone()).await.map_err(|_| "Cannot connect to MariaDB through the configured socket")?;
            let versions: Vec<u32> = conn.query("SELECT version FROM schema_migrations ORDER BY version").await.map_err(|_| "Cannot read MariaDB schema version")?;
            if versions != [1] { return Err("MariaDB schema must be version 1; migrations require an administrator"); }
            let engines: Vec<(String, String)> = conn.query("SELECT TABLE_NAME, ENGINE FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME IN ('sensors','measurements','collector_status','schema_migrations') ORDER BY TABLE_NAME").await.map_err(|_| "Cannot inspect MariaDB schema")?;
            if engines.len() != 4 || engines.iter().any(|(_, engine)| engine != "InnoDB") { return Err("MariaDB schema requires all four version 1 InnoDB tables"); }
            for query in ["SELECT id,hub_id,model,name,location,enabled,first_seen,last_seen FROM sensors LIMIT 0", "SELECT measurement_key,sensor_id,observed_at,collected_at,temperature_c,humidity_pct,low_battery,source,resolution_seconds,batch_id FROM measurements LIMIT 0", "SELECT sensor_id,last_attempt,last_success,consecutive_failures,last_error FROM collector_status LIMIT 0"] {
                conn.query_drop(query).await.map_err(|_| "MariaDB schema columns do not match version 1")?;
            }
            conn.disconnect().await.map_err(|_| "Cannot close MariaDB connection")?;
            Ok(())
        }).await.map_err(|_| "MariaDB validation timed out")?
    }
    pub async fn insert(&self, batch: &Batch) -> Result<()> {
        batch.validate()?;
        tokio::time::timeout(Duration::from_secs(30), self.insert_inner(batch))
            .await
            .map_err(|_| "MariaDB transaction timed out; batch retained for replay")?
    }
    async fn insert_inner(&self, batch: &Batch) -> Result<()> {
        let mut conn = Conn::new(self.options.clone())
            .await
            .map_err(|_| "MariaDB connection failed; batch retained for replay")?;
        let mut tx = conn
            .start_transaction(TxOpts::default())
            .await
            .map_err(|_| "Cannot start MariaDB transaction")?;
        let at = sql_time(batch.collected_at);
        for item in &batch.sensors {
            let sensor = &item.sensor;
            tx.exec_drop("INSERT INTO sensors (id,hub_id,model,name,location,enabled,first_seen,last_seen) VALUES (:id,:hub,:model,:name,:room,:enabled,:at,:at) ON DUPLICATE KEY UPDATE hub_id=IF(VALUES(last_seen)>=last_seen,VALUES(hub_id),hub_id),model=IF(VALUES(last_seen)>=last_seen,VALUES(model),model),name=IF(VALUES(last_seen)>=last_seen,VALUES(name),name),location=IF(VALUES(last_seen)>=last_seen,VALUES(location),location),enabled=IF(VALUES(last_seen)>=last_seen,VALUES(enabled),enabled),last_seen=GREATEST(last_seen,VALUES(last_seen))", params! { "id" => &sensor.id, "hub" => &batch.hub_id, "model" => &sensor.model, "name" => &sensor.name, "room" => &sensor.room, "enabled" => sensor.enabled, "at" => &at }).await.map_err(|_| "Cannot upsert sensor metadata; batch retained")?;
            for measurement in &item.measurements {
                tx.exec_drop("INSERT INTO measurements (measurement_key,sensor_id,observed_at,collected_at,temperature_c,humidity_pct,low_battery,source,resolution_seconds,batch_id) VALUES (:key,:sensor,:observed,:collected,:temp,:humidity,:battery,:source,:resolution,:batch) ON DUPLICATE KEY UPDATE temperature_c=IF(VALUES(collected_at)>collected_at AND source='history',VALUES(temperature_c),temperature_c),humidity_pct=IF(VALUES(collected_at)>collected_at AND source='history',VALUES(humidity_pct),humidity_pct),batch_id=IF(VALUES(collected_at)>collected_at AND source='history',VALUES(batch_id),batch_id),collected_at=IF(source='history',GREATEST(collected_at,VALUES(collected_at)),collected_at)", params! { "key" => &measurement.key, "sensor" => &sensor.id, "observed" => measurement.observed_at.map(sql_time), "collected" => sql_time(measurement.collected_at), "temp" => measurement.temperature_c, "humidity" => measurement.humidity_pct, "battery" => measurement.low_battery, "source" => &measurement.source, "resolution" => measurement.resolution_seconds, "batch" => &batch.id }).await.map_err(|_| "Cannot insert measurements; transaction rolled back and batch retained")?;
            }
            tx.exec_drop("INSERT INTO collector_status (sensor_id,last_attempt,last_success,consecutive_failures,last_error) VALUES (:id,:at,:success_at,:failures,:error) ON DUPLICATE KEY UPDATE last_success=IF(VALUES(last_attempt)>COALESCE(last_attempt,'1000-01-01') AND :success,VALUES(last_success),last_success),consecutive_failures=IF(VALUES(last_attempt)>COALESCE(last_attempt,'1000-01-01'),IF(:success,0,consecutive_failures+1),consecutive_failures),last_error=IF(VALUES(last_attempt)>COALESCE(last_attempt,'1000-01-01'),VALUES(last_error),last_error),last_attempt=GREATEST(COALESCE(last_attempt,'1000-01-01'),VALUES(last_attempt))", params! { "id" => &sensor.id, "at" => &at, "success_at" => if item.success { Some(&at) } else { None }, "failures" => u32::from(!item.success), "error" => &item.error, "success" => item.success }).await.map_err(|_| "Cannot update collector status; batch retained")?;
        }
        tx.commit()
            .await
            .map_err(|_| "MariaDB commit failed; batch retained for idempotent replay")?;
        let _ = conn.disconnect().await;
        Ok(())
    }
    pub async fn drain(&self, spool: &Spool) -> Result<usize> {
        let pending = spool.pending()?;
        let count = pending.len();
        for (path, batch) in pending {
            self.insert(&batch).await?;
            spool.acknowledge(&path)?;
        }
        Ok(count)
    }
}
fn sql_time(time: DateTime<Utc>) -> String {
    time.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::TestDir;
    use chrono::TimeZone;

    fn sample_batch() -> Batch {
        Batch {
            version: 1,
            id: "batch-one".into(),
            hub_id: "hub-one".into(),
            collected_at: Utc.with_ymd_and_hms(2026, 10, 12, 12, 0, 0).unwrap(),
            sensors: vec![SensorBatch {
                sensor: SensorConfig {
                    id: "sensor-one".into(),
                    name: "Office".into(),
                    model: "T315".into(),
                    room: Some("Study".into()),
                    enabled: true,
                },
                success: true,
                error: None,
                measurements: vec![Measurement {
                    key: "live-key".into(),
                    observed_at: None,
                    collected_at: Utc.with_ymd_and_hms(2026, 10, 12, 12, 0, 0).unwrap(),
                    temperature_c: 20.0,
                    humidity_pct: 50.0,
                    low_battery: Some(false),
                    source: "live".into(),
                    resolution_seconds: None,
                }],
            }],
        }
    }
    #[test]
    fn spool_replays_after_restart_and_only_removes_acknowledged_batch() {
        let dir = TestDir::new();
        let path = dir.0.join("spool///");
        let mut spool = Spool::open(&path, 100_000).unwrap();
        spool.enqueue(&sample_batch()).unwrap();
        assert!(
            Spool::open(&path, 100_000).is_err(),
            "second collector must be excluded"
        );
        drop(spool);
        let spool = Spool::open(&path, 100_000).unwrap();
        let entries = spool.pending().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].1.sensors[0].measurements[0].temperature_c, 20.0);
        assert!(
            spool
                .acknowledge(std::path::Path::new("/tmp/not-a-batch"))
                .is_err()
        );
        spool.acknowledge(&entries[0].0).unwrap();
        assert!(spool.pending().unwrap().is_empty());
    }
    #[test]
    fn capacity_refusal_preserves_pending_data_and_corruption_stops_replay() {
        let dir = TestDir::new();
        let mut spool = Spool::open(&dir.0.join("spool"), 1).unwrap();
        assert!(spool.enqueue(&sample_batch()).is_err());
        assert!(spool.pending().unwrap().is_empty());
        drop(spool);
        let mut spool = Spool::open(&dir.0.join("spool"), 100_000).unwrap();
        spool.enqueue(&sample_batch()).unwrap();
        assert!(spool.enqueue(&sample_batch()).is_err());
        let path = spool.pending().unwrap()[0].0.clone();
        std::fs::write(&path, b"invalid").unwrap();
        assert!(spool.pending().is_err());
        assert!(path.exists());
    }
    #[test]
    fn rejects_invalid_measurements_and_distinguishes_event_identity() {
        let mut batch = sample_batch();
        batch.sensors[0].measurements[0].humidity_pct = 101.0;
        assert!(batch.validate().is_err());
        batch = sample_batch();
        batch.sensors[0].measurements[0].source = "history".into();
        assert!(batch.validate().is_err());
        assert_eq!(
            measurement_key("history", "hub", "sensor", "interval"),
            measurement_key("history", "hub", "sensor", "interval")
        );
        assert_ne!(
            measurement_key("live", "hub", "sensor", "batch-a"),
            measurement_key("live", "hub", "sensor", "batch-b")
        );
        assert_ne!(
            measurement_key("history", "hub", "sensor", "interval"),
            measurement_key("history", "hub", "other", "interval")
        );
    }
}
