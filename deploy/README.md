# Deploy the collector

Build and run setup on the development box. Install the compiled executable and
the four generated configuration files on your automation host. The target needs
no Rust toolchain. Provision database schema version 1 before installation.
The hub address below is a documentation placeholder; replace it with your own.

## Prepare on the development box

```sh
. "$HOME/.cargo/env"
cargo build --release --locked
target/release/tapo-telemetry setup --hub 192.0.2.10 --output-dir config/deployment
```

Use your hub's IP for `--hub`; initial setup has no built-in address.
Setup reads the existing `config/credentials.json` and
`config/database-credentials.json`. It contacts the H100, prompts for sensor
selection, names, rooms and polling settings, and saves only after confirmation.
New sensors default to disabled. It does not connect to MariaDB or use SSH.

The output directory must not exist; its parent must exist. Both `DIR` and
`DIR/` work. The wizard refuses existing destinations and symlink directories.
To rediscover sensors while retaining existing settings, write a new bundle:

```sh
target/release/tapo-telemetry setup \
  --config config/deployment/collector.json \
  --output-dir config/deployment-next
```

Previously configured sensors remain in the configuration even when absent from
discovery. Rediscovery uses the saved hub address; add `--hub IP` to override it.
The wizard copies credentials without changing their values.

For the detailed merge rules and the update/backup/rollback procedure, see
[Rediscover and merge new sensors](../docs/workflow.md#rediscover-and-merge-new-sensors)
and [Install the reviewed update](../docs/workflow.md#install-the-reviewed-update).

## Transfer files

Use an encrypted transfer method to move these files to the automation box:

- `target/release/tapo-telemetry`
- `deploy/tapo-telemetry.service`
- `deploy/install.sh`
- `config/deployment/collector.json`
- `config/deployment/mariadb.json`
- `config/deployment/credentials.json`
- `config/deployment/database-credentials.json`

Do not copy source code, Cargo, rustup, or the protected bootstrap SQL.
The configuration files reference each other by relative filename, so moving
them together requires no edits. The socket and spool paths describe the target.

## Install on the automation box

Stop an existing collector before changing its files or repairing permissions:

```sh
sudo systemctl stop tapo-telemetry.service
```

On the automation box, run the transferred directory preparation script:

```sh
sudo bash install.sh
```

The script creates the Linux `tapoagent` account if missing, separate from the
MariaDB account. It creates `/etc/tapo-telemetry/`, `/var/lib/tapo-telemetry/`
and `/var/lib/tapo-telemetry/spool/` with ownership `tapoagent:tapoagent` and mode
0700. Existing bundle files and spool files receive the same ownership and mode
0600; nested spool directories receive mode 0700. It preserves file contents,
including pending batches. It rejects symlinks, unexpected file types and a
privileged service account before repairing permissions. It takes no directory
arguments and uses the standard deployment paths.

You can run it again after copying configuration files or to repair permissions.
It installs no binary, toolchain or schema and does not control the service.
Keep a protected backup of the previous bundle when replacing configuration.

From the directory holding the transferred executable, service unit and four
JSON files, install them:

```sh
sudo install -o root -g root -m 755 tapo-telemetry /usr/local/bin/tapo-telemetry
sudo install -o root -g root -m 644 tapo-telemetry.service /etc/systemd/system/tapo-telemetry.service
sudo install -o tapoagent -g tapoagent -m 600 \
  collector.json mariadb.json credentials.json database-credentials.json \
  /etc/tapo-telemetry/
```

If you used another method to copy the JSON files, run `sudo bash install.sh`
again before validation. Keep the collector stopped until validation passes.

Verify executable compatibility and socket access, then collect one cycle:

```sh
/usr/local/bin/tapo-telemetry --help
sudo -u tapoagent /usr/local/bin/tapo-telemetry check
sudo -u tapoagent /usr/local/bin/tapo-telemetry collect --once
```

`check` reads credentials and checks database authentication, schema version and
required tables/columns. It creates no spool files and inserts no measurements.
`collect --once` also verifies/creates the private spool, checks the hub path,
reads live values and completed history intervals, and writes a durable batch
before committing it to MariaDB. Any partial read or persistence failure exits
with code 1; pending batches remain available for replay. Hub authentication
rejection exits with code 78.

Enable unattended collection after these checks pass:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now tapo-telemetry.service
sudo systemctl status tapo-telemetry.service
sudo journalctl -u tapo-telemetry.service --since today
```

The service reads configuration at startup. Transfer revised files and restart
it after adding sensors or changing intervals. Authentication rejection stops
the service without automatic retries; correct credentials and restart it.

## Recovery and limits

Live polling defaults to 60 seconds, history synchronization to 1800 seconds.
Both are independent schedules; reads are sequential, and missed ticks are
skipped rather than caught up in a burst. An offline or missing sensor records a
failure without stopping reads from other configured sensors.

The default spool is `/var/lib/tapo-telemetry/spool`, limited to 100,000,000 bytes.
Each batch is synchronized to disk before insertion. A failed transaction leaves
the batch pending; replay uses stable keys to prevent duplicate inserts. Live
events remain distinct even when values are unchanged. History keys identify
the hub, sensor and completed 15-minute interval; newer retrievals may update
that interval. Live observation timestamps remain NULL, and temperatures are
stored in Celsius.

At startup, MariaDB must be reachable for schema validation. During a running
collector's database outage, collection continues into the spool. Exhausting
the cap stops collection explicitly, preserving pending batches; it does not
silently evict them. A process lock excludes concurrent collectors using the
same spool. Corrupt published batches stop replay and remain on disk for review.

The durability guarantee begins once a batch has been published and synchronized.
Readings still in memory during an interrupted poll cycle are outside that
guarantee. Incomplete `.tmp` files from abrupt termination are not replayed;
they still count toward capacity. Review them while the collector is stopped.
Never remove published `.json` batches unless their database commit is verified.

The supplied service unit uses systemd's state directory and restricts writes to
it. If you choose a spool outside `/var/lib/tapo-telemetry/`, adjust the service's
write permissions for that location before starting it.

Verification covers H100 reads, MariaDB inserts over a Unix socket, runtime
outage buffering, graceful shutdown, idempotent restart replay and unattended
systemd collection with protected configuration. Build only on the development
box. Sustained operation, repeated history scheduling and recovery after a target
reboot or power loss require validation in your environment.
