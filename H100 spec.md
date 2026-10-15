# Tapo Environmental Telemetry System
**Original design outline — v1.0**

This historical outline contains proposals, not an implementation status report.
See [README.md](README.md) for implemented collector and MCP integration behavior.

**Status:** Historical concept reference  
**Platform:** Debian 13  
**Primary language:** Go  
**Database:** MariaDB  
**Buffer:** Filesystem-backed persistent queue  
**Deployment:** Single host, systemd  
**External interface:** Model Context Protocol (MCP)

---

## 1. Project Objective

Develop a local-first environmental telemetry system that collects temperature and humidity measurements from TP-Link Tapo sensors connected to an H100 hub.

The system will persist historical measurements in MariaDB and expose selected data through an independent, authenticated, read-only MCP service.

The eventual objective is to allow ChatGPT to analyze environmental conditions throughout an building, including temperature differences, humidity, cooling effectiveness, and relationships to HVAC operation and electricity consumption.

The system is for monitoring and analysis only. It must not provide smart-home control capabilities.

### Design principles

- Prefer simple, maintainable architecture.
- Use Go for application services.
- Use MariaDB for persistent storage.
- Avoid additional databases and message brokers.
- Separate collection from MCP access.
- Prefer local communication with the H100.
- Use narrowly scoped permissions.
- Support reliable operation without clustering or redundancy.
- Avoid speculative features and unnecessary abstractions.

## 2. Hardware

| Device | Location | Purpose |
|---|---|---|
| Tapo H100 | Building | Sensor hub |
| Existing Tapo sensor | Room A | Sleep environment |
| Tapo T315 | Room B | Computer-generated heat |
| Tapo T310 | Room C | Occupant comfort |

All three sensors communicate through the existing H100.

Sensors may be added, removed, or renamed without requiring code changes.

Stable device identifiers must be used instead of display names as database keys.

## 3. Deployment Architecture

A single existing Debian 13 host in the DMZ will run:

1. Tapo collector
2. Local filesystem spool
3. MariaDB
4. Read-only MCP server

The collector and MCP server are independent systemd services.

### Data flow

Tapo sensors → H100 → Go collector → filesystem spool → MariaDB → read-only MCP server → authenticated HTTPS → ChatGPT.

The collector initiates communication with the H100 over narrowly permitted network paths.

MariaDB runs locally on the DMZ host and should use a Unix socket or loopback connection.

The MCP server connects to MariaDB using a separate account with SELECT-only privileges.

The MCP server must not communicate directly with the H100.

### Network boundaries

The implementation must document the H100's required ports, protocols, and authentication requirements.

The DMZ host must not receive unrestricted access to the internal LAN.

External MCP access will eventually use the existing AWS/nginx frontend or another supported authenticated HTTPS gateway.

Do not expose the H100, MariaDB, or collector directly to the internet.

## 4. Tapo Protocol Adapter

### Requirements

The adapter must:

- Authenticate to the H100.
- Discover paired sensors.
- Retrieve current temperature and humidity.
- Retrieve sensor identifiers and metadata.
- Retrieve battery status when available.
- Retrieve historical records if supported.
- Detect unavailable sensors.
- Recover from authentication or connection failures.

The adapter must not implement device-control operations.

### Candidate libraries

Go:

https://github.com/fabiankachlock/tapo-api

Rust:

https://github.com/mihai-dinculescu/tapo

Codex must inspect current upstream implementations and determine compatibility with the installed H100 firmware, including TPAP authentication where applicable.

**Do not assume that documented library support guarantees compatibility with the actual hub.**

### Implementation preference

Use Go directly if reliable local communication is available.

If the Go library is insufficient, use a small Rust adapter based on an existing maintained library.

Keep scheduling, persistence, configuration, and database logic in Go.

The protocol implementation must remain replaceable without changing the collector's storage logic.

Do not reimplement Tapo encryption or authentication unless existing libraries have been demonstrated to be unsuitable.

## 5. Collector Service

### Polling

Default interval: 60 seconds.

Alternative interval: 120 seconds.

The interval must be configurable.

Polling must not overlap. If a poll exceeds its interval, skip or delay the next cycle rather than launching concurrent requests against the hub.

Do not assume that polling the H100 every 60 seconds produces a new physical measurement every 60 seconds.

### Measurement timestamps

Track:

- **Observed time:** Device-reported measurement time, when available.
- **Collection time:** Time the collector retrieved the reading.
- **Source:** Live polling or historical retrieval.
- **Resolution:** Measurement or aggregation interval, when known.

If the device does not expose the original measurement time, store collection time and identify the reading as a polling snapshot.

Repeated numerical values are valid and must not automatically be discarded.

### Historical retrieval

Investigate whether the H100 supports retrieving recent historical measurements.

If available, use historical records for backfill after collector outages.

Proposed historical synchronization interval: 30 minutes.

Actual retention, granularity, and timestamp behavior must be established during Phase 0.

Historical averages must be distinguishable from instantaneous readings.

### Error handling

If one sensor fails, continue polling the others.

If the H100 is unavailable, retry with bounded exponential backoff.

Do not fabricate missing measurements.

Record failures and expose collection health.

Temporary failures must not crash the collector.

## 6. Filesystem-Backed Write Queue

### Purpose

Provide durable buffering during MariaDB restarts, maintenance, or temporary unavailability.

No SQLite, Redis, or separate queue service is required.

### Format

Use one JSON batch file per polling cycle.

Each batch contains:

- Schema version
- Stable batch identifier
- Creation timestamp
- Array of measurements

A batch may contain fewer than three measurements if individual sensors are unavailable.

### Directory layout

- `/var/lib/tapo-telemetry/spool/tmp/`
- `/var/lib/tapo-telemetry/spool/pending/`
- `/var/lib/tapo-telemetry/spool/failed/`

### Write procedure

1. Collect and validate readings.
2. Serialize the batch.
3. Write to a temporary file.
4. Flush and synchronize the file.
5. Atomically rename into the pending directory.
6. Synchronize the directory.
7. Attempt insertion into MariaDB.
8. Delete the pending file only after successful database commit.

Temporary and pending directories must reside on the same filesystem.

### Recovery

On startup:

- Scan pending batches.
- Validate their contents.
- Replay them in deterministic order.
- Retry transient database failures.
- Quarantine malformed batches.
- Report errors without silently discarding valid data.

A crash after database commit but before queue-file deletion can cause replay.

Therefore, database insertion must be idempotent.

### Capacity

Default maximum pending payload size: 100 MB.

Track pending batch count, pending bytes, oldest batch age, and failed batches.

If the queue reaches capacity, report an explicit error rather than silently dropping data.

This is a best-effort telemetry service, not a safety-critical recording system.

## 7. MariaDB Storage

Use InnoDB, UTF-8, UTC timestamps, parameterized queries, and versioned SQL migrations.

### sensors

Required fields:

| Column | Type |
|---|---|
| id | VARCHAR(128) PRIMARY KEY |
| hub_id | VARCHAR(128) |
| model | VARCHAR(64) |
| name | VARCHAR(128) |
| location | VARCHAR(64) |
| enabled | BOOLEAN |
| first_seen | DATETIME(3) |
| last_seen | DATETIME(3) |

Sensor names and room assignments may change without changing device identity.

### measurements

Required fields:

| Column | Type |
|---|---|
| id | BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT |
| measurement_key | VARCHAR(128) UNIQUE |
| sensor_id | VARCHAR(128) |
| observed_at | DATETIME(3) NULL |
| collected_at | DATETIME(3) |
| temperature_c | DECIMAL(6,3) |
| humidity_pct | DECIMAL(5,2) |
| source | VARCHAR(32) |
| resolution_seconds | INT NULL |
| batch_id | VARCHAR(128) |

Indexes:

- `(sensor_id, collected_at)`
- `(sensor_id, observed_at)`
- Unique measurement key

The measurement key must be stable across queue replays.

For polling snapshots, derive it from a stable collection-event identifier and sensor identifier.

For historical measurements, use a source record identifier where available, or a deterministic combination of sensor, source, interval, and timestamp.

Never deduplicate based solely on matching temperature and humidity values.

### collector_status

Required fields:

| Column | Type |
|---|---|
| sensor_id | VARCHAR(128) PRIMARY KEY |
| last_attempt | DATETIME(3) |
| last_success | DATETIME(3) |
| consecutive_failures | INT |
| last_error | TEXT |
| updated_at | DATETIME(3) |

Collector status should not require inserting fabricated measurement records.

### Database permissions

Create separate database accounts.

**Collector account:** SELECT, INSERT, and UPDATE on required tables.

**MCP account:** SELECT only on approved tables or views.

**Migration account:** Schema modification privileges.

Runtime services must not use administrative database credentials.

Schema migrations must not run automatically under the MCP account.

## 8. MCP Server

### Objective

Provide deterministic, read-only access to environmental telemetry.

The MCP server is a separate Go service, not an autonomous AI agent.

It must not access Tapo credentials or execute device-control commands.

### MCP tools

**list_sensors**

Returns available sensors, room assignments, models, and data freshness.

**get_current_conditions**

Returns latest temperature, humidity, measurement timestamp, collection timestamp, and stale status.

**get_sensor_history**

Returns bounded historical measurements for a sensor and time range.

**get_room_statistics**

Returns minimum, maximum, and average temperature and humidity, sample count, and data coverage.

**compare_rooms**

Compares readings across two or more rooms using aligned time intervals.

**get_collection_health**

Returns sensor freshness, last successful collection, and available collector-health information.

### Query restrictions

- No arbitrary SQL tool.
- No SQL supplied by an LLM.
- All queries parameterized.
- Validate sensor identifiers and date ranges.
- Bound result sizes.
- Enforce query timeouts.
- Support aggregation for longer date ranges.
- Do not interpolate missing data without explicitly indicating it.

### Freshness

Every current-condition response must contain timestamps.

Default stale threshold: five minutes.

A stale measurement must not be presented as a current temperature.

### MCP transport and authentication

Use a maintained Go MCP SDK.

Select a ChatGPT-compatible remote MCP transport during the MCP implementation phase.

Initially bind to localhost only.

For remote deployment:

- Require HTTPS.
- Use a supported authentication mechanism.
- Validate authorization independently of the LLM.
- Do not expose database credentials.
- Do not allow unauthenticated access.
- Verify compatibility with ChatGPT before finalizing gateway configuration.

The MCP service must remain read-only at both the application and database permission levels.

## 9. Configuration

Use YAML for nonsecret configuration.

Use environment variables or protected systemd credentials for secrets.

Required configuration includes:

- H100 address
- Tapo authentication settings
- Polling interval
- Historical synchronization interval
- MariaDB connection settings
- Spool directory
- Spool capacity
- Logging level
- Sensor location mappings
- MCP listener address
- MCP query limits

Do not log secrets.

Configuration validation must occur before the service starts polling.

## 10. Service Management

Use systemd for both Go services.

Requirements:

- Dedicated unprivileged service accounts.
- Automatic restart after unexpected termination.
- Appropriate startup ordering.
- Graceful SIGTERM handling.
- Bounded shutdown time.
- journald logging.
- No interactive startup requirements.
- Restricted filesystem permissions.

The collector must tolerate MariaDB being unavailable at startup.

The MCP server must report database unavailability cleanly.

No Docker or Kubernetes requirement.

## 11. Monitoring and Diagnostics

The collector should expose or log:

- Last successful H100 connection
- Last successful poll per sensor
- Poll duration
- Poll failure count
- Database connectivity state
- Queue depth and age
- Historical synchronization status

Avoid verbose per-reading logs during normal operation.

Use structured logging where practical.

Provide a simple local diagnostic command or documented procedure to test hub connectivity.

No custom dashboard is required.

## 12. Testing

### Unit tests

- Configuration parsing
- Temperature conversion
- Measurement validation
- Timestamp handling
- Queue serialization
- Duplicate prevention
- Statistics and aggregation
- Staleness calculation
- MCP input validation

### Integration tests

Use a mock sensor adapter to simulate:

- Normal polling
- Sensor timeout
- Hub unavailable
- Authentication expiration
- MariaDB unavailable
- Database recovery
- Queue replay
- Process restart
- Duplicate batch insertion
- Malformed batch
- Historical backfill

Tests must not require physical Tapo hardware unless explicitly designated as hardware integration tests.

Hardware tests must not modify device configuration.

## 13. Implementation Phases

### Phase 0 — Protocol Validation

**Mandatory first milestone.**

Tasks:

1. Inspect current Go and Rust Tapo libraries.
2. Identify H100 firmware compatibility requirements.
3. Establish local authentication.
4. Discover the existing room A sensor.
5. Retrieve live temperature and humidity.
6. Determine whether readings have device timestamps.
7. Test historical retrieval if available.
8. Document authentication, connectivity, and firmware limitations.
9. Select Go-native or Go/Rust adapter architecture.

Deliverable: Minimal working connectivity proof of concept and a protocol decision report.

**Do not implement the full collector before Phase 0 succeeds.**

### Phase 1 — Collector and MariaDB

Implement:

- Protocol adapter
- Sensor discovery
- Poll scheduling
- Normalization
- MariaDB schema and migrations
- Measurement insertion
- Collector status
- Configuration and logging

Deliverable: Continuous local collection from the room A sensor.

### Phase 2 — Filesystem Spool

Implement:

- Durable batch files
- Atomic publication
- Database flush worker
- Crash recovery
- Retry behavior
- Capacity limits
- Idempotent insertion

Deliverable: Collector continues recording during a MariaDB restart and replays buffered data afterward.

### Phase 3 — Multi-Room Monitoring

Add the Room B T315 and Room C T310.

Verify stable identification, room assignment, and collection behavior.

Deliverable: Three-room historical telemetry.

### Phase 4 — MCP Server

Implement read-only MCP tools and database access.

Test locally before exposing the service externally.

Deliverable: Independent MCP service with structured environmental queries.

### Phase 5 — Remote Integration

Deploy behind authenticated HTTPS.

Validate the selected MCP transport and authentication mechanism with ChatGPT.

Deliverable: ChatGPT can retrieve sensor readings and historical statistics without control privileges.

## 14. Future Enhancements

The architecture should permit additional telemetry sources without requiring major redesign.

Potential integrations:

- Google Nest thermostat temperature
- Google Nest cooling runtime
- SCE hourly electricity consumption
- Outdoor weather data
- Additional Tapo sensors
- Long-term room comfort analysis
- Cooling efficiency comparisons

These integrations are not part of the initial implementation.

Do not implement generic plugin infrastructure solely to accommodate hypothetical future integrations.

## 15. Acceptance Criteria

The collector MVP is complete when:

- Local H100 communication works reliably.
- Existing room A sensor readings can be retrieved.
- Polling interval is configurable.
- MariaDB stores valid measurements.
- Sensor identifiers remain stable.
- Queue buffering survives service restarts.
- Buffered measurements replay without duplication.
- Temporary database failures do not terminate collection.
- Sensor failures do not terminate collection.
- No device-control operations are exposed.
- systemd services run without interactive intervention.
- Unit and integration tests pass.
- Installation and operation are documented.

MCP functionality is a later milestone and is not required for the collector MVP.

## 16. Instructions to Codex

Treat this document as the authoritative project specification.

Begin with Phase 0 only.

Before implementation:

1. Review the specification for contradictions or missing requirements.
2. Inspect current upstream Tapo library implementations.
3. Identify the simplest viable local H100 integration.
4. Determine whether native Go support is sufficient.
5. Document any necessary Rust dependency.
6. Implement a minimal, read-only connectivity test.

Do not build MariaDB schemas, the spool, the MCP server, or deployment infrastructure until the H100 communication path is validated.

Do not introduce Docker, Home Assistant, Redis, SQLite, or a message broker.

Do not expand scope without identifying the requirement that justifies the change.

After Phase 0, provide a report containing:

- Selected library and version
- H100 firmware compatibility findings
- Authentication requirements
- Discovered sensors
- Successful live measurement results
- Historical retrieval capabilities
- Known limitations
- Proposed Phase 1 implementation plan

**Primary engineering objective:** Build a small, reliable environmental telemetry service that is easy to operate, inspect, and maintain.
