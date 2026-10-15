# Tapo Environmental Telemetry System

Rust tools for reading temperature and humidity from Tapo H100 sensors and
storing telemetry in MariaDB. The executable includes a read-only diagnostic,
an interactive setup wizard, target database validation, and a collector with
durable disk buffering.

## Workflow guide

See [Set up collection and add sensors](docs/workflow.md) for initial credentials,
build/setup, rerunning discovery, merge rules, private bundle review, transfer,
validation, MCP verification and rollback. The wizard's `--config` option merges
existing settings with newly discovered devices by stable ID.

The [configuration examples](docs/workflow.md#configuration-examples) include
single-sensor and multi-sensor collectors, Unix-socket MariaDB settings and
separate credential templates, with commands to copy a complete private bundle.

## Configure sensors on the development box

After preparing the separate Tapo and database credential files, run:

```sh
cargo run --locked -- setup --hub 192.0.2.10 --output-dir config/deployment
```

The addresses in this guide are documentation placeholders. Pass your hub's IP
with `--hub`; initial setup has no built-in hub address.
Select sensors, assign names and rooms, and review the settings before saving.
Setup writes a new directory containing `collector.json`, `mariadb.json`,
`credentials.json`, and `database-credentials.json`. Move these four files
together to `/etc/tapo-telemetry/` on the automation box. File references remain
relative; credentials retain owner-only permissions. Setup needs the hub but
does not need MariaDB or SSH access.

The output directory must not exist; its parent must exist. To add sensors later,
use `--config config/deployment/collector.json --output-dir config/deployment-next`.
Existing sensor settings are preserved; newly discovered sensors default to
disabled until selected. Rediscovery reuses the hub address from `--config`;
use `--hub IP` to override it. Both directory paths with and without trailing
separators work. Existing destinations and symlink directories are refused.

## Run collection on the automation box

With the compiled executable and configuration installed, run:

```sh
tapo-telemetry check
tapo-telemetry collect --once
tapo-telemetry collect
```

The default configuration is `/etc/tapo-telemetry/collector.json`; use
`--config FILE` to select another file. `check` validates configuration,
protected credentials, socket authentication and schema without collecting or
inserting measurements. `collect --once` reads one live/history cycle and commits
it, then exits. The unattended collector uses separate configurable live and
history schedules and reloads configuration after a restart.

Live polling defaults to 60 seconds, history synchronization to 1800 seconds.
The collector stores Celsius temperatures, NULL live observation timestamps,
and completed history intervals at 900-second resolution. Its bounded spool
retains synchronized batches until database commit and replays them without
duplicate measurements. Runtime database outages continue buffering; startup
requires MariaDB for validation. Capacity exhaustion stops collection explicitly.

Follow [the deployment guide](deploy/README.md) for manual installation,
permissions, systemd service setup and recovery limits. Build binaries on this
box; the automation box needs no Rust toolchain.

On the automation box, run `sudo bash install.sh` using the transferred
[directory preparation script](deploy/install.sh). It creates the service account
and directories and repairs permissions on existing configuration and spool
files. Stop an existing collector first. You can repeat the script without
replacing configuration or pending batches.

## Run the data access test

Development requirements: Rust 1.88 or later, network access to the H100, and the credentials
for the TP-Link/Tapo account associated with the hub.

1. Load the Rust environment:

   ```sh
   . "$HOME/.cargo/env"
   ```

2. Create `config/credentials.json` if it does not already exist:

   ```json
   {
     "username": "your-account-email",
     "password": "your-account-password"
   }
   ```

   Edit the file locally and restrict its permissions with
   `chmod 600 config/credentials.json`. The file is ignored by Git. Avoid putting
   credentials in command arguments or sharing the file.

3. Run from the project root:

   ```sh
   cargo run --locked -- --hub 192.0.2.10 --credentials config/credentials.json
   ```

   `--hub IP` is required. The credentials path defaults to
   `config/credentials.json`. Use `--help` for usage.

The command prints JSON. Exit code `0` means an online environmental sensor was
read and its history request succeeded. Exit code `1` means configuration or
communication failed, no online environmental reading was obtained, or a
sensor/history read partially failed. Inspect `status`, `stage`, and sensor
results for details. The command stops on authentication rejection and does not
retry credentials. Requests have a 10-second timeout; the complete diagnostic
has a 60-second limit.

## Interpret readings

Temperature values retain the unit reported by the sensor. Humidity is a
percentage. Battery information is a low-battery flag, not a remaining-charge
percentage. `report_interval_seconds` is device metadata, not proof that a
measurement was taken at the time of collection.

`current.collected_at` records when this computer received the live response.
`current.observed_at` is `null` because that response has no measurement
timestamp. The live reading can therefore be a cached hub value.

History contains the library's interpretation of the hub's last 24 hours in
15-minute intervals. `hub_retrieved_at` comes from the hub clock; record timestamps
represent interval starts assigned by the library. The latest interval may be
incomplete. The upstream library describes values as averages; aggregation and
clock accuracy have not been independently verified.

## Hardware compatibility

Authenticated live and history retrieval has been tested with H100 hardware 1.0,
firmware 1.7.10, and a T315 sensor. Non-environmental child devices are skipped;
this diagnostic performs no device-control operations.

## Development

```sh
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
python3 -m unittest discover -s deploy -p 'test_*.py'
```

The Tapo library is pinned to `0.11.1`; `Cargo.lock` fixes the resolved dependency
versions. [Initialize MariaDB](sql/README.md) describes the database bootstrap.
Verification covers authenticated H100 reads, schema version 1, restricted
Unix-socket collector access, MariaDB writes, history deduplication, outage
buffering, graceful shutdown and idempotent restart replay. Compiled-binary
systemd deployment and unattended collection have also been verified.

The independent [MariaDB MCP project](../mariadb-mcp/README.md) owns the generic
runtime and [application file reference](../mariadb-mcp/docs/mariadb-mcp-app-format.md).
Tapo owns collection, the telemetry schema, reader provisioning and four read-only
JSON query definitions. ChatGPT OAuth linking and telemetry reads are confirmed;
see [Tapo MCP integration](mcp/README.md). Production configuration and identifiers
remain in ignored local files.

## Deployment requirement

Build application binaries on the development box. Deploy compiled executables,
required configuration, and service units to your automation host; do not install
Rust, rustup, Cargo, or build dependencies there or copy application source for
compilation. Verify executable compatibility with the target environment before enabling the collector. The [deployment guide](deploy/README.md)
and [systemd unit](deploy/tapo-telemetry.service) describe the verified installation.
The executable is installed at `/usr/local/bin/tapo-telemetry`; the enabled service
is `tapo-telemetry.service`.

The setup wizard runs on the development box. Move its generated
configuration and separate protected credential files to the automation box,
where the collector uses MariaDB's local Unix socket. Setup must be able to save
configuration without database access from the development box. Target filesystem
and database validation happen on the automation box.

The configuration set can be moved together into `/etc/tapo-telemetry/`:

| File | Contents |
| --- | --- |
| `collector.json` | Hub, sensors, polling intervals, spool settings, and references to the other settings files |
| `mariadb.json` | Unix socket, database name, and reference to database credentials |
| `credentials.json` | Tapo username and password |
| `database-credentials.json` | MariaDB username and password |

File references are relative to the file containing them, so moving the set
requires no path edits. Passwords stay out of connection settings. Credential
files must remain readable only by their owner, with ownership allowing the
collector service account to read them.
