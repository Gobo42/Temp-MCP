-- Initial MariaDB bootstrap for Tapo Environmental Telemetry System.
-- Run as an administrator on the MariaDB host through its Unix socket.
-- Supply @tapoagent_password in the same client session before this file.
-- config/provision-tapoagent.sql is a protected copy with that value supplied.
-- Refuses existing databases/accounts. DDL is not transactional: stop on errors
-- and inspect partial results before retrying. Do not use the client --force flag.

SET SESSION time_zone = '+00:00';

DELIMITER //
BEGIN NOT ATOMIC
    IF @tapoagent_password IS NULL OR CHAR_LENGTH(@tapoagent_password) < 16 THEN
        SIGNAL SQLSTATE '45000'
            SET MESSAGE_TEXT = 'Supply @tapoagent_password with at least 16 characters';
    END IF;
    IF EXISTS (
        SELECT 1 FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = 'tapoagent'
    ) THEN
        SIGNAL SQLSTATE '45000'
            SET MESSAGE_TEXT = 'Database tapoagent already exists; refusing bootstrap';
    END IF;
    IF EXISTS (SELECT 1 FROM mysql.user WHERE User = 'tapoagent') THEN
        SIGNAL SQLSTATE '45000'
            SET MESSAGE_TEXT = 'User tapoagent already exists; refusing bootstrap';
    END IF;
END//
DELIMITER ;

CREATE DATABASE `tapoagent`
    CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;

USE `tapoagent`;

CREATE TABLE schema_migrations (
    version INT UNSIGNED NOT NULL,
    description VARCHAR(255) NOT NULL,
    applied_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    PRIMARY KEY (version)
) ENGINE = InnoDB;

CREATE TABLE sensors (
    id VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    hub_id VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    model VARCHAR(64) NOT NULL,
    name VARCHAR(128) NOT NULL,
    location VARCHAR(64) NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    first_seen DATETIME(3) NOT NULL,
    last_seen DATETIME(3) NOT NULL,
    PRIMARY KEY (id),
    KEY idx_sensors_hub (hub_id),
    CONSTRAINT chk_sensors_enabled CHECK (enabled IN (0, 1))
) ENGINE = InnoDB;

-- Live snapshots have no device observation time. History uses library-assigned
-- interval starts, with resolution_seconds identifying the aggregation interval.
-- measurement_key identifies an event/interval, never just its numeric values.
CREATE TABLE measurements (
    id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    measurement_key VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    sensor_id VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    observed_at DATETIME(3) NULL,
    collected_at DATETIME(3) NOT NULL,
    temperature_c DECIMAL(6,3) NOT NULL,
    humidity_pct DECIMAL(5,2) NOT NULL,
    low_battery BOOLEAN NULL,
    source VARCHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    resolution_seconds INT UNSIGNED NULL,
    batch_id VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_measurements_key (measurement_key),
    KEY idx_measurements_collected (sensor_id, collected_at),
    KEY idx_measurements_observed (sensor_id, observed_at),
    CONSTRAINT fk_measurements_sensor FOREIGN KEY (sensor_id)
        REFERENCES sensors (id) ON DELETE RESTRICT ON UPDATE RESTRICT,
    CONSTRAINT chk_measurements_humidity CHECK (humidity_pct BETWEEN 0 AND 100),
    CONSTRAINT chk_measurements_battery CHECK (low_battery IN (0, 1)),
    CONSTRAINT chk_measurements_source CHECK (source IN ('live', 'history')),
    CONSTRAINT chk_measurements_resolution CHECK (
        resolution_seconds IS NULL OR resolution_seconds > 0
    ),
    CONSTRAINT chk_measurements_history CHECK (
        source <> 'history' OR (observed_at IS NOT NULL AND resolution_seconds IS NOT NULL)
    )
) ENGINE = InnoDB;

CREATE TABLE collector_status (
    sensor_id VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
    last_attempt DATETIME(3) NULL,
    last_success DATETIME(3) NULL,
    consecutive_failures INT UNSIGNED NOT NULL DEFAULT 0,
    last_error TEXT NULL,
    updated_at DATETIME(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3)
        ON UPDATE CURRENT_TIMESTAMP(3),
    PRIMARY KEY (sensor_id),
    CONSTRAINT fk_collector_status_sensor FOREIGN KEY (sensor_id)
        REFERENCES sensors (id) ON DELETE RESTRICT ON UPDATE RESTRICT
) ENGINE = InnoDB;

INSERT INTO schema_migrations (version, description)
VALUES (1, 'Initial sensors, measurements, and collector status schema');

-- Quote the session variable as a SQL literal; keep the password out of CLI args.
SET @create_tapoagent = CONCAT(
    'CREATE USER ''tapoagent''@''localhost'' IDENTIFIED BY ',
    QUOTE(@tapoagent_password)
);
PREPARE create_tapoagent FROM @create_tapoagent;
EXECUTE create_tapoagent;
DEALLOCATE PREPARE create_tapoagent;
SET @create_tapoagent = NULL;
SET @tapoagent_password = NULL;

GRANT SELECT, INSERT, UPDATE ON `tapoagent`.`sensors`
    TO 'tapoagent'@'localhost';
GRANT SELECT, INSERT, UPDATE ON `tapoagent`.`measurements`
    TO 'tapoagent'@'localhost';
GRANT SELECT, INSERT, UPDATE ON `tapoagent`.`collector_status`
    TO 'tapoagent'@'localhost';
GRANT SELECT ON `tapoagent`.`schema_migrations`
    TO 'tapoagent'@'localhost';

-- Report names and permissions without returning authentication material.
SELECT TABLE_NAME, ENGINE
FROM information_schema.TABLES
WHERE TABLE_SCHEMA = 'tapoagent'
ORDER BY TABLE_NAME;

SELECT TABLE_NAME, PRIVILEGE_TYPE
FROM information_schema.TABLE_PRIVILEGES
WHERE GRANTEE = '''tapoagent''@''localhost''' AND TABLE_SCHEMA = 'tapoagent'
ORDER BY TABLE_NAME, PRIVILEGE_TYPE;
