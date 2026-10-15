# Set up collection and add sensors

Use this guide to configure the Tapo collector on a development computer, install
its compiled binary and private configuration on an automation host, and add
sensors while retaining existing settings and history.

Discovery is part of `tapo-telemetry setup`. The same executable also provides
`diagnose`, `check` and `collect`; there is no separate discovery script to install.

## Before you begin

Use Rust 1.88 or later on the development computer, an H100 reachable from that
computer and the automation host, and paired T310/T315 sensors. Pair new sensors
with the H100 in the Tapo app before rediscovery. Unsupported child devices are
skipped; adding them in the Tapo app doesn't make them environmental sensors.

The automation host needs MariaDB, the provisioned collector schema/account,
Python 3 for the permission helper, and the compiled collector. It needs no Rust
or Cargo. See [SQL setup](../sql/README.md) and
[initial service installation](../deploy/README.md).

Run development commands from the project root. Replace `HUB_IP` with the H100's
numeric IP address. SSH examples use `SSH_USER` and `AUTOMATION_HOST` placeholders;
replace them with your own account and automation host.

## Prepare credentials and build locally

If the two input credential files already exist, keep them. Otherwise, copy the
[account template](../config/credentials.example.json) and
[database template](../config/database-credentials.example.json):

```sh
cp -n config/credentials.example.json config/credentials.json
cp -n config/database-credentials.example.json config/database-credentials.json
chmod 600 config/credentials.json config/database-credentials.json
```

Edit only the private copies. Set `username` and `password` to your Tapo account
credentials and MariaDB collector credentials respectively. `cp -n` preserves
existing files. Template values are placeholders, not usable credentials.
Every real file or bundle under `config/` is ignored by Git; only root
`*.example.json` templates are publishable.

Build the collector on the development computer:

```sh
. "$HOME/.cargo/env"
cargo build --release --locked
```

To check hub authentication and readings before setup, run the read-only diagnostic:

```sh
target/release/tapo-telemetry diagnose \
  --hub HUB_IP \
  --credentials config/credentials.json
```

The diagnostic prints device IDs and readings but creates no collector config and
writes no database rows. A successful diagnostic doesn't automatically enable a
sensor in the poller.

## Create the first configuration

Use a destination that doesn't exist, with an existing parent directory:

```sh
target/release/tapo-telemetry setup \
  --hub HUB_IP \
  --output-dir config/deployment
```

The wizard discovers supported sensors, shows their stable IDs and available
readings, then asks which sensors to enable. New sensors start disabled: enter
`yes` for each sensor you want polled. Choose display names, rooms, live/history
intervals, the target MariaDB socket/database and an absolute target spool path.
Use `-` to clear a room. At least one supported sensor must be enabled.

Review the displayed collector/database settings and explicitly confirm saving.
The final save prompt defaults to `no`; declining or ending input saves nothing.
Setup contacts the H100, but requires no development-host MariaDB or SSH access.

The resulting bundle contains these files:

| File | Purpose |
| --- | --- |
| `collector.json` | Hub, stable sensor IDs, names/rooms, enabled flags, intervals, spool and file references. |
| `mariadb.json` | Automation-host Unix socket, database and credentials-file reference. |
| `credentials.json` | Private Tapo account credentials. |
| `database-credentials.json` | Private collector database credentials. |

Keep all four files together. Paths between them are relative. Install this bundle
and the compiled executable using the [deployment guide](../deploy/README.md).
Keep `/var/lib/tapo-telemetry/spool/` separate from configuration and preserve it
when replacing a bundle.

## Configuration examples

Use these templates to understand the file format or prepare a configuration
manually. For actual devices, the setup wizard supplies stable sensor IDs and
models from discovery.

| Template | Example |
| --- | --- |
| [collector.example.json](../config/collector.example.json) | One enabled T315, 60-second live polls and 1800-second history synchronization. |
| [collector.multi-sensor.example.json](../config/collector.multi-sensor.example.json) | Two enabled sensors using T315 and T310 models, plus a disabled spare. |
| [mariadb.example.json](../config/mariadb.example.json) | Local Unix socket and separate database credentials. |
| [credentials.example.json](../config/credentials.example.json) | Tapo account placeholders. |
| [database-credentials.example.json](../config/database-credentials.example.json) | Collector database account and password placeholder. |

To copy a complete multi-sensor bundle into a fresh private directory, run from
the project root:

```sh
mkdir -m 700 config/example-bundle
```

After directory creation succeeds, copy and rename the templates:

```sh
install -m 600 config/collector.multi-sensor.example.json config/example-bundle/collector.json
install -m 600 config/mariadb.example.json config/example-bundle/mariadb.json
install -m 600 config/credentials.example.json config/example-bundle/credentials.json
install -m 600 config/database-credentials.example.json config/example-bundle/database-credentials.json
```

For one sensor, use `collector.example.json` as the first copy's source instead.
If `config/example-bundle` exists, choose another unused directory rather than
overwriting an existing configuration. Keep the runtime filenames shown above:
the JSON references use those names, without `.example` or `.multi-sensor`.

Replace the documentation-only hub address `192.0.2.10`, synthetic sensor IDs,
names/rooms and credential placeholders. Set each model to the discovered model.
Review the target database/socket and spool paths. `enabled: false` stops
polling that entry, and `room: null` leaves it unassigned. At least one sensor
must remain enabled. Verify actual readings before unattended collection;
the examples illustrate accepted configuration, not hardware acceptance.

Live/history intervals apply to all enabled sensors and accept 1 through 86400
seconds. The spool cap must be greater than zero. These example credentials and
IDs don't access a real device. After adapting the bundle, follow the
[deployment guide](../deploy/README.md) and run target validation before starting
collection. Your copied bundle stays ignored by Git; keep real values out of
the publishable `*.example.json` templates.

## Rediscover and merge new sensors

1. Pair the additional sensors with the same H100 in the Tapo app.
2. Use the local bundle that matches the running deployment. If configuration
   was edited on the automation host, obtain an administrator-provided private
   copy of all four installed files before continuing.
3. Load the existing configuration and write the merged result to a fresh directory:

   ```sh
   target/release/tapo-telemetry setup \
     --config config/deployment/collector.json \
     --output-dir config/deployment-next
   ```

   `--config` performs the merge; no manual concatenation of JSON is needed.
   The wizard reuses the saved hub and both credential files. If that H100's IP
   changed, add `--hub HUB_IP` to override its saved address.

4. Press Enter to retain existing sensor settings and global settings. For each
   appended sensor, choose `yes` to enable it and enter its display name and room.
5. Review the result and confirm saving. The original bundle remains unchanged.

The merge uses stable device IDs, with these outcomes:

| Discovery result | Merged configuration |
| --- | --- |
| Existing ID with the same model | Keeps its display name, room and enabled flag as prompt defaults. |
| Existing ID missing from discovery | Keeps the entry and its settings; absence doesn't delete it. |
| New ID | Appends a sensor using its discovered name/model, no room and `enabled: false` until you choose otherwise. |
| Existing ID reported with a different model | Stops for configuration review; it doesn't silently replace the model. |

Existing sensor order is retained, with new sensors appended. Poll/history
intervals, database connection, spool settings and credential values are reused
unless explicitly changed through the wizard or credential input overrides.
Names aren't used as merge keys: two devices with the same name still have
different IDs. Repeated discovery of an existing ID doesn't append a duplicate.

For example, an existing enabled `sensor-a` keeps its chosen name and room when
the hub reports a different display name. A discovered `sensor-b` is appended
and needs explicit enablement. A temporarily absent `sensor-c` remains configured.
These IDs are synthetic examples; use the IDs supplied by actual discovery.

Review the private collector settings without printing credential-file contents:

```sh
python3 -m json.tool config/deployment-next/collector.json
diff -u config/deployment/collector.json config/deployment-next/collector.json
```

`diff` exits with 1 when changes are present; that is expected for added sensors.
You can also review `mariadb.json` if connection settings changed. Keep these
outputs local because they contain your device IDs and room labels.

For another round, use the latest reviewed/deployed bundle as `--config` and
another unused output directory, such as `config/deployment-third`. Relative and
absolute directory paths with single or repeated trailing separators are accepted.
Existing destinations, even empty ones, and symlink paths are refused. Choose a
fresh directory rather than deleting or overwriting the previous working bundle.

## Install the reviewed update

A configuration update doesn't require rebuilding the binary, changing the
schema, or resetting the database. Transfer the four new files and the collector
permission helper into a fresh private staging directory on the automation host.
For example, create a directory in the SSH account's home:

```sh
ssh SSH_USER@AUTOMATION_HOST 'mkdir -m 700 tapo-update'
```

If `tapo-update` already exists, choose a different unused staging name. After
creation succeeds, transfer the files from the development computer:

```sh
scp deploy/install.sh \
  config/deployment-next/collector.json \
  config/deployment-next/mariadb.json \
  config/deployment-next/credentials.json \
  config/deployment-next/database-credentials.json \
  SSH_USER@AUTOMATION_HOST:tapo-update/
```

Run the remaining commands on the automation host. Stop collection, then create a
unique protected backup of the installed configuration:

```sh
sudo systemctl stop tapo-telemetry.service
TAPO_CONFIG_BACKUP=$(sudo mktemp -d /etc/tapo-telemetry-backup.XXXXXX)
sudo cp -a /etc/tapo-telemetry/. "$TAPO_CONFIG_BACKUP/"
printf 'Configuration backup: %s\n' "$TAPO_CONFIG_BACKUP"
```

Record that backup path. From the staging directory, install the reviewed files
and repair service ownership/permissions:

```sh
cd ~/tapo-update
sudo install -o tapoagent -g tapoagent -m 600 \
  collector.json mariadb.json credentials.json database-credentials.json \
  /etc/tapo-telemetry/
sudo bash install.sh
sudo -u tapoagent /usr/local/bin/tapo-telemetry check
```

`check` validates configuration, protected credentials, database socket access and
schema. It doesn't authenticate to the H100, collect measurements or write rows.
If it fails, correct the configuration or restore the backup before proceeding.

With the unattended service still stopped, verify a real hub/collection cycle:

```sh
sudo -u tapoagent /usr/local/bin/tapo-telemetry collect --once
```

This step writes telemetry and can replay pending spool batches. If it reports a
partial sensor or persistence failure, inspect the error before starting the
unattended collector. Don't run a second collector against the same spool.

After validation and the cycle succeed, start the service and inspect status:

```sh
sudo systemctl start tapo-telemetry.service
sudo systemctl status tapo-telemetry.service
sudo journalctl -u tapo-telemetry.service --since '10 minutes ago'
```

Remove the transferred credential copies from the private staging directory after
successful installation; retain the protected configuration backup for rollback.
Your database history and pending spool batches remain in place. Keep the
confirmed bundle on the development computer for the next rediscovery run.

## Verify through MCP and recover if needed

After the first successful commit, the Tapo MCP tools read new sensor rows from
MariaDB. Adding a sensor requires no MCP app edit or MCP restart when using the
standard Tapo definition. Ask your connected client to list sensors, show current
conditions and check collection health. Recent collection doesn't prove a new
physical measurement; live observation time remains unknown.

To stop polling a sensor, disable it during setup and deploy that reviewed bundle.
Existing history isn't deleted. Disabled sensors aren't polled or included in new
collector batches, so their existing database metadata/status may remain visible
to MCP rather than immediately reflecting the disabled config flag.

If the revised configuration fails, keep collection stopped and restore the four
files from the recorded backup path. Use `TAPO_CONFIG_BACKUP` in the same shell,
or set it to the previously recorded backup path:

```sh
sudo systemctl stop tapo-telemetry.service
sudo install -o tapoagent -g tapoagent -m 600 \
  "$TAPO_CONFIG_BACKUP/collector.json" \
  "$TAPO_CONFIG_BACKUP/mariadb.json" \
  "$TAPO_CONFIG_BACKUP/credentials.json" \
  "$TAPO_CONFIG_BACKUP/database-credentials.json" \
  /etc/tapo-telemetry/
sudo -u tapoagent /usr/local/bin/tapo-telemetry check
```

After the restored configuration passes `check`, start the service. Restoring
configuration doesn't erase readings already written by the verification cycle.
Don't recreate the schema, delete the database, clear the spool or reset MCP
OAuth state as part of adding sensors or restoring collector configuration.
