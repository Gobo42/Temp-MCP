use crate::{Credentials, read_credentials};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::Write,
    net::IpAddr,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SensorConfig {
    pub id: String,
    pub model: String,
    pub name: String,
    pub room: Option<String>,
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CollectorConfig {
    pub version: u32,
    pub hub: IpAddr,
    pub credentials_file: PathBuf,
    pub mariadb_file: PathBuf,
    pub poll_interval_seconds: u64,
    pub history_interval_seconds: u64,
    pub spool_directory: PathBuf,
    pub spool_limit_bytes: u64,
    pub sensors: Vec<SensorConfig>,
}

impl CollectorConfig {
    pub fn new(hub: IpAddr, sensors: Vec<SensorConfig>) -> Self {
        Self {
            version: 1,
            hub,
            credentials_file: "credentials.json".into(),
            mariadb_file: "mariadb.json".into(),
            poll_interval_seconds: 60,
            history_interval_seconds: 1800,
            spool_directory: "/var/lib/tapo-telemetry/spool".into(),
            spool_limit_bytes: 100_000_000,
            sensors,
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err("Unsupported collector configuration version");
        }
        if !(1..=86400).contains(&self.poll_interval_seconds)
            || !(1..=86400).contains(&self.history_interval_seconds)
        {
            return Err("Polling intervals must be between 1 and 86400 seconds");
        }
        if self.spool_limit_bytes == 0
            || self.spool_directory.as_os_str().is_empty()
            || self.credentials_file.as_os_str().is_empty()
            || self.mariadb_file.as_os_str().is_empty()
        {
            return Err("Configuration paths and spool limit must not be empty or zero");
        }
        let mut ids = HashSet::new();
        for sensor in &self.sensors {
            if !valid_id(&sensor.id) || !ids.insert(&sensor.id) {
                return Err(
                    "Sensor IDs must be unique ASCII letters/digits/hyphens/underscores, at most 128 characters",
                );
            }
            if !matches!(sensor.model.as_str(), "T310" | "T315") {
                return Err("Only T310 and T315 sensors are supported");
            }
            if sensor.name.trim().is_empty()
                || sensor.name.chars().count() > 128
                || sensor.name.chars().any(char::is_control)
                || sensor.room.as_ref().is_some_and(|room| {
                    room.chars().count() > 64 || room.chars().any(char::is_control)
                })
            {
                return Err(
                    "Sensor names must be nonblank and at most 128 characters; rooms at most 64; no control characters",
                );
            }
        }
        if !self.sensors.iter().any(|s| s.enabled) {
            return Err("Enable at least one environmental sensor");
        }
        Ok(())
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MariaDbConfig {
    pub socket: PathBuf,
    pub database: String,
    pub credentials_file: PathBuf,
}
impl Default for MariaDbConfig {
    fn default() -> Self {
        Self {
            socket: "/run/mysqld/mysqld.sock".into(),
            database: "tapoagent".into(),
            credentials_file: "database-credentials.json".into(),
        }
    }
}
impl MariaDbConfig {
    pub fn validate(&self) -> Result<()> {
        if !self.socket.is_absolute() || self.socket.to_str().is_none_or(|s| s.contains('\0')) {
            return Err("MariaDB socket must be an absolute path");
        }
        if self.database.is_empty()
            || self.database.len() > 64
            || !self
                .database
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(
                "Database name must contain only ASCII letters, digits or underscores, at most 64 characters",
            );
        }
        if self.credentials_file.as_os_str().is_empty() {
            return Err("Database credential file path is required");
        }
        Ok(())
    }
}

pub struct Loaded {
    pub config: CollectorConfig,
    pub mariadb: MariaDbConfig,
    pub tapo: Credentials,
    pub database: Credentials,
    pub spool: PathBuf,
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "Cannot open configuration file")?;
    if !file
        .metadata()
        .map_err(|_| "Cannot inspect configuration file")?
        .is_file()
    {
        return Err("Configuration must be a regular file");
    }
    serde_json::from_reader(file).map_err(|_| "Invalid configuration JSON or unexpected fields")
}
pub fn resolve(file: &Path, reference: &Path) -> PathBuf {
    file.parent().unwrap_or(Path::new(".")).join(reference)
}
pub fn load(file: &Path) -> Result<Loaded> {
    let file = std::path::absolute(file).map_err(|_| "Cannot resolve configuration path")?;
    let config: CollectorConfig = read_json(&file)?;
    config.validate()?;
    let mariadb_file = resolve(&file, &config.mariadb_file);
    let mariadb: MariaDbConfig = read_json(&mariadb_file)?;
    mariadb.validate()?;
    let tapo = read_credentials(&resolve(&file, &config.credentials_file))?;
    let database = read_credentials(&resolve(&mariadb_file, &mariadb.credentials_file))?;
    let spool = resolve(&file, &config.spool_directory);
    Ok(Loaded {
        config,
        mariadb,
        tapo,
        database,
        spool,
    })
}

pub fn normalize_directory(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err("Directory path must not be empty");
    }
    let absolute = std::path::absolute(path).map_err(|_| "Cannot resolve directory path")?;
    // Components remove trailing separators without corrupting filesystem roots.
    let normalized: PathBuf = absolute.components().collect();
    for ancestor in normalized.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err("Symlink directories are not allowed");
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("Cannot inspect directory path"),
        }
    }
    Ok(normalized)
}

pub fn private_directory(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder
        .mode(0o700)
        .create(path)
        .map_err(|_| "Cannot create private directory")
}
pub fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| "Cannot create file without replacement")?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "Cannot write and synchronize file")
}
pub fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|_| "Cannot synchronize directory")
}
pub fn rename_new(from: &Path, to: &Path) -> Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let from = CString::new(from.as_os_str().as_bytes()).map_err(|_| "Invalid source path")?;
    let to = CString::new(to.as_os_str().as_bytes()).map_err(|_| "Invalid destination path")?;
    // Linux no-replace rename preserves refusal even if another process creates the destination.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err("Cannot publish file or directory without replacement");
    }
    Ok(())
}

pub fn publish_bundle(
    destination: &Path,
    config: &CollectorConfig,
    db: &MariaDbConfig,
    tapo: &Credentials,
    database: &Credentials,
) -> Result<()> {
    config.validate()?;
    db.validate()?;
    if config.credentials_file != Path::new("credentials.json")
        || config.mariadb_file != Path::new("mariadb.json")
        || db.credentials_file != Path::new("database-credentials.json")
    {
        return Err("Published bundle must use portable relative file references");
    }
    let destination = normalize_directory(destination)?;
    if fs::symlink_metadata(&destination).is_ok() {
        return Err("Output directory already exists; choose a new destination");
    }
    let parent = destination
        .parent()
        .ok_or("Cannot publish to filesystem root")?;
    if !parent.is_dir() {
        return Err("Output parent directory must already exist");
    }
    let stage = parent.join(format!(
        ".tapo-setup-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "Invalid system clock")?
            .as_nanos()
    ));
    private_directory(&stage)?;
    let result = (|| {
        for (name, value) in [
            ("collector.json", serde_json::to_vec_pretty(config)),
            ("mariadb.json", serde_json::to_vec_pretty(db)),
            ("credentials.json", serde_json::to_vec_pretty(tapo)),
            (
                "database-credentials.json",
                serde_json::to_vec_pretty(database),
            ),
        ] {
            write_new(
                &stage.join(name),
                &value.map_err(|_| "Cannot serialize configuration")?,
            )?;
        }
        sync_directory(&stage)?;
        rename_new(&stage, &destination)?;
        sync_directory(parent)
    })();
    if stage.exists() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    pub struct TestDir(pub std::path::PathBuf);
    impl TestDir {
        pub fn new() -> Self {
            let path = std::env::current_dir()
                .unwrap()
                .join("target")
                .join(format!(
                    "config-test-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn sample() -> CollectorConfig {
        CollectorConfig::new(
            "192.0.2.10".parse().unwrap(),
            vec![SensorConfig {
                id: "abc123".into(),
                model: "T315".into(),
                name: "Room A".into(),
                room: Some("Room A".into()),
                enabled: true,
            }],
        )
    }
    fn credentials() -> Credentials {
        Credentials {
            username: "test-user".into(),
            password: " space-sensitive ".into(),
        }
    }

    #[test]
    fn bundle_moves_without_edits_and_preserves_credentials() {
        let dir = TestDir::new();
        publish_bundle(
            &dir.0.join("first"),
            &sample(),
            &MariaDbConfig::default(),
            &credentials(),
            &credentials(),
        )
        .unwrap();
        std::fs::rename(dir.0.join("first"), dir.0.join("moved")).unwrap();
        let loaded = load(&dir.0.join("moved/collector.json")).unwrap();
        assert_eq!(loaded.database.password, " space-sensitive ");
        assert_eq!(loaded.config.sensors[0].id, "abc123");
        assert_eq!(
            loaded.mariadb.socket,
            std::path::Path::new("/run/mysqld/mysqld.sock")
        );
        assert_eq!(
            loaded.spool,
            std::path::Path::new("/var/lib/tapo-telemetry/spool")
        );
    }

    #[test]
    fn directory_arguments_normalize_without_authorizing_overwrite() {
        let dir = TestDir::new();
        for (i, relative, suffix) in [
            (0, false, ""),
            (1, false, "/"),
            (2, false, "///"),
            (3, true, ""),
            (4, true, "/"),
            (5, true, "///"),
        ] {
            let absolute = dir.0.join(format!("destination-{i}"));
            let path = if relative {
                absolute
                    .strip_prefix(std::env::current_dir().unwrap())
                    .unwrap()
                    .to_path_buf()
            } else {
                absolute.clone()
            };
            let argument = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
            publish_bundle(
                &argument,
                &sample(),
                &MariaDbConfig::default(),
                &credentials(),
                &credentials(),
            )
            .unwrap();
            assert!(absolute.join("collector.json").is_file());
            assert!(
                publish_bundle(
                    &argument,
                    &sample(),
                    &MariaDbConfig::default(),
                    &credentials(),
                    &credentials()
                )
                .is_err()
            );
            assert!(absolute.join("credentials.json").is_file());
        }
        assert!(
            publish_bundle(
                std::path::Path::new("/"),
                &sample(),
                &MariaDbConfig::default(),
                &credentials(),
                &credentials()
            )
            .is_err()
        );
        std::fs::create_dir(dir.0.join("existing-empty")).unwrap();
        assert!(
            publish_bundle(
                &dir.0.join("existing-empty///"),
                &sample(),
                &MariaDbConfig::default(),
                &credentials(),
                &credentials()
            )
            .is_err()
        );
        assert!(
            publish_bundle(
                &dir.0.join("missing-parent/new"),
                &sample(),
                &MariaDbConfig::default(),
                &credentials(),
                &credentials()
            )
            .is_err()
        );
    }

    #[test]
    fn symlinks_and_public_credentials_are_refused() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = TestDir::new();
        symlink(dir.0.join("absent"), dir.0.join("link")).unwrap();
        assert!(
            publish_bundle(
                &dir.0.join("link///"),
                &sample(),
                &MariaDbConfig::default(),
                &credentials(),
                &credentials()
            )
            .is_err()
        );
        symlink(&dir.0, dir.0.join("parent-link")).unwrap();
        assert!(
            publish_bundle(
                &dir.0.join("parent-link/new"),
                &sample(),
                &MariaDbConfig::default(),
                &credentials(),
                &credentials()
            )
            .is_err()
        );
        publish_bundle(
            &dir.0.join("bundle"),
            &sample(),
            &MariaDbConfig::default(),
            &credentials(),
            &credentials(),
        )
        .unwrap();
        let secret = dir.0.join("bundle/credentials.json");
        assert_eq!(
            std::fs::metadata(&secret).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load(&dir.0.join("bundle/collector.json")).is_err());
    }

    #[test]
    fn rejects_invalid_configuration_before_publication() {
        let mut config = sample();
        config.poll_interval_seconds = 0;
        assert!(config.validate().is_err());
        config = sample();
        config.sensors.push(config.sensors[0].clone());
        assert!(config.validate().is_err());
        config = sample();
        config.sensors[0].id = "unsafe/id".into();
        assert!(config.validate().is_err());
        config = sample();
        config.sensors[0].model = "S200D".into();
        assert!(config.validate().is_err());
        config = sample();
        config.sensors[0].enabled = false;
        assert!(config.validate().is_err());
        let db = MariaDbConfig {
            socket: "relative/socket".into(),
            ..MariaDbConfig::default()
        };
        assert!(db.validate().is_err());
    }
}
