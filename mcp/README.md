# Tapo MCP integration

These files expose collector schema version 1 through the independent MariaDB
MCP service. The collector owns schema and ingestion; the generic service loads
the JSON definitions after restart.

Copy the definition and connection into protected service configuration:

```text
mcp/apps.d/tapo.json → /etc/mariadb-mcp/apps.d/tapo.json
mcp/connections/tapo.json → /etc/mariadb-mcp/connections/tapo.json
config/mcp-database-credentials.json → /etc/mariadb-mcp/credentials/tapo.json
```

The credential source is a local protected file. Provision its account manually
using the reader script in [SQL setup](../sql/README.md). The reader needs SELECT
on four declared tables; its account and exact grants are verified at startup. The collector retains its separate ingestion account.

The connection uses /run/mysqld/mysqld.sock and database tapoagent. Directories
need mode 0700, files mode 0600, and service-account ownership. Use the independent
[permission helper](../../mariadb-mcp/deploy/README.md) after copying, then check.

The app exposes four read-only tools under the tapo:read scope:

| Tool | Result |
| --- | --- |
| tapo__list_sensors | Stored metadata, latest live presence and collection age/freshness. |
| tapo__get_current_conditions | Latest live values, missing-reading state and unknown physical observation freshness. |
| tapo__get_sensor_history | Separate live/history records in a half-open range; known sensors with no data return a presence row. |
| tapo__get_collection_health | Database-visible collector status and healthy/degraded/unknown state. |

The optional sensor ID defaults to all sensors for current conditions and health.
Empty selections return not_found; an empty sensor list returns zero rows.
History requires sensor ID and timezone-bearing start/end, defaults to history
and 500 rows, caps at 1000 rows and 31 days, and reports truncation. Row counts
include no-data presence rows; use reading_present to count physical readings.

Temperature uses Celsius, humidity uses percent and timestamps use UTC. Live
observation time remains NULL: recent collection does not prove a recent physical
observation. The stale_seconds constant defaults to 180 independently of the
collector interval. Future collection timestamps produce clock_anomaly; latest
ties use the larger measurement ID. Metadata reflects persisted collector state.

[Fixture SQL](fixtures/schema.sql) creates only the named isolated test database
and reader. Run it only in the private development server, immediately before
the time-sensitive [acceptance harness](../../mariadb-mcp/docs/private-acceptance.md).
The four tools pass fixture and deployment acceptance. ChatGPT OAuth linking
and telemetry reads are confirmed. Configure the public HTTPS URL, exact client
callback, listener and protected credentials in the independent MCP service;
these deployment-specific values are kept outside the published files.
