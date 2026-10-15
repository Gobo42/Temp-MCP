-- Isolated test-server fixture only. Never run against the automation database.
DROP DATABASE IF EXISTS mcp_tapo_fixture;
CREATE DATABASE mcp_tapo_fixture CHARACTER SET utf8mb4;
USE mcp_tapo_fixture;
SET SESSION time_zone='+00:00';
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


INSERT INTO sensors VALUES
('a','fixture-hub','T315','Room A',NULL,1,UTC_TIMESTAMP(3),UTC_TIMESTAMP(3)),
('b','fixture-hub','T315','Room B',NULL,1,UTC_TIMESTAMP(3),UTC_TIMESTAMP(3)),
('c','fixture-hub','T315','Room C',NULL,1,UTC_TIMESTAMP(3),UTC_TIMESTAMP(3)),
('d','fixture-hub','T315','Future',NULL,1,UTC_TIMESTAMP(3),UTC_TIMESTAMP(3)),
('e','fixture-hub','T315','Stale',NULL,1,UTC_TIMESTAMP(3),UTC_TIMESTAMP(3)),
('f','fixture-hub','T315','Ties',NULL,1,UTC_TIMESTAMP(3),UTC_TIMESTAMP(3));
INSERT INTO measurements(id,measurement_key,sensor_id,observed_at,collected_at,temperature_c,humidity_pct,low_battery,source,resolution_seconds,batch_id) VALUES
(1,'live-a','a',NULL,UTC_TIMESTAMP(3)-INTERVAL 60 SECOND,20.5,50.0,0,'live',NULL,'fixture'),
(2,'future-d','d',NULL,UTC_TIMESTAMP(3)+INTERVAL 300 SECOND,22.0,50.0,NULL,'live',NULL,'fixture'),
(3,'stale-e','e',NULL,UTC_TIMESTAMP(3)-INTERVAL 3600 SECOND,19.0,60.0,1,'live',NULL,'fixture'),
(20,'tie-f-20','f',NULL,UTC_TIMESTAMP(3),21.0,40.0,0,'live',NULL,'fixture'),
(21,'tie-f-21','f',NULL,UTC_TIMESTAMP(3),22.0,41.0,0,'live',NULL,'fixture'),
(30,'history-a-1','a','2026-10-12 00:00:00',UTC_TIMESTAMP(3),20.0,50.0,NULL,'history',900,'fixture'),
(31,'history-a-2','a','2026-10-12 00:15:00',UTC_TIMESTAMP(3),21.0,51.0,NULL,'history',900,'fixture'),
(32,'history-a-3','a','2026-10-12 00:30:00',UTC_TIMESTAMP(3),22.0,52.0,NULL,'history',900,'fixture');
INSERT INTO collector_status(sensor_id,last_attempt,last_success,consecutive_failures,last_error) VALUES
('a',UTC_TIMESTAMP(3),UTC_TIMESTAMP(3),0,NULL),
('d',UTC_TIMESTAMP(3),UTC_TIMESTAMP(3),0,NULL),
('e',UTC_TIMESTAMP(3),UTC_TIMESTAMP(3)-INTERVAL 3600 SECOND,1,'fixture_failure');
CREATE USER IF NOT EXISTS 'mcp_tapo_fixture'@'localhost' IDENTIFIED BY 'fixture-only-password';
GRANT SELECT ON mcp_tapo_fixture.sensors TO 'mcp_tapo_fixture'@'localhost';
GRANT SELECT ON mcp_tapo_fixture.measurements TO 'mcp_tapo_fixture'@'localhost';
GRANT SELECT ON mcp_tapo_fixture.collector_status TO 'mcp_tapo_fixture'@'localhost';
GRANT SELECT ON mcp_tapo_fixture.schema_migrations TO 'mcp_tapo_fixture'@'localhost';
