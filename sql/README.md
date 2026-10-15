# Initialize MariaDB

Use these scripts to create the initial `tapoagent` database and collector account,
then a separate read-only MCP account on the deployment host. They target
MariaDB 11.8 and Unix-socket access. Run them on your own deployment host.
The SSH examples use a documentation address and generic account name; replace
both with your host and SSH account.

## Run the prepared script

`config/provision-tapoagent.sql` is the ready-to-run local copy. It supplies the
password stored in `config/database-credentials.json` and includes the SQL from
`sql/provision-tapoagent.sql`. Both files containing credentials have `0600`
permissions and are ignored by Git. Keep them private.

Copy the prepared script to the database host, preserving `0600` permissions.
Then run it there as an administrator:

```sh
sudo mariadb --protocol=socket --socket=/run/mysqld/mysqld.sock < provision-tapoagent.sql
```

Alternatively, from this project's root on the development box, use your trusted
SSH connection:

```sh
ssh -o StrictHostKeyChecking=yes operator@198.51.100.20 \
  'sudo mariadb --protocol=socket --socket=/run/mysqld/mysqld.sock' \
  < config/provision-tapoagent.sql
```

Establish a trusted host-key entry before using this command.

The script prints four table names and their permissions after completion. It
does not print the password. Keep the credentials JSON for the collector.

## Created schema and permissions

| Table | Purpose | Collector privileges |
| --- | --- | --- |
| `sensors` | Stable IDs, names, rooms, enabled flags, first/last discovery times | SELECT, INSERT, UPDATE |
| `measurements` | Celsius temperature, humidity, nullable observation time, collection time, source, resolution, battery flag, event key, batch ID | SELECT, INSERT, UPDATE |
| `collector_status` | Collection attempts, successes, failure counts, and errors | SELECT, INSERT, UPDATE |
| `schema_migrations` | Applied schema version | SELECT |

The collector account is `'tapoagent'@'localhost'`, uses password authentication,
and is intended to connect through the local Unix socket. It receives no DELETE,
schema modification, account administration, or grant privileges. Later schema
migrations use an administrator separately.

Measurement event keys are unique; foreign keys protect sensor references. Live
snapshots use `source = 'live'`; interval records use `source = 'history'` and
require an observation timestamp and resolution. Store all timestamps in UTC.
Every collector connection must select UTC for its session.

## Reusable script and execution limits

`sql/provision-tapoagent.sql` contains no password. To use it separately, supply
the session variable `@tapoagent_password` with a password of at least 16
characters in the same client session before the script. Use a protected input
file, rather than putting the password in shell arguments.

This is a one-time bootstrap. It refuses any existing `tapoagent` database or
account before creating objects. MariaDB DDL is not transactional: if execution
fails partway through, inspect the error and existing objects before deciding
how to continue. Avoid the client's `--force` option, which can continue after
the preflight refusal.

The project owner applied the prepared script on October 12, 2026. Read-only
verification confirms four InnoDB tables, schema version 1, the intended grants,
and successful password authentication as `tapoagent@localhost` through the
Unix socket. Later collector acceptance verified real telemetry inserts,
history deduplication, runtime outage buffering and idempotent restart replay
through a temporary SSH Unix-socket relay. No schema or listener changes were
made. Do not rerun the bootstrap against this initialized database.

## Provision the MCP reader

`config/provision-mcpagent.sql` is the prepared script for
`'mcpagent'@'localhost'`. Its generated password is saved separately in
`config/mcp-database-credentials.json` for the MCP service. Both files are
mode `0600` and ignored by Git. The reusable version,
`sql/provision-mcpagent.sql`, contains no password.

1. Copy `config/provision-mcpagent.sql` to the MariaDB host as
   `provision-mcpagent.sql`, preserving mode `0600`.
2. Run it there as an administrator through the existing socket:

   ```sh
   sudo mariadb --protocol=socket --socket=/run/mysqld/mysqld.sock < provision-mcpagent.sql
   ```

3. Confirm the result lists `SELECT` for each of `sensors`, `measurements`,
   `collector_status` and `schema_migrations`.

Alternatively, run the prepared file from this project's root on the development
box using the existing SSH connection:

```sh
ssh -o StrictHostKeyChecking=yes operator@198.51.100.20 \
  'sudo mariadb --protocol=socket --socket=/run/mysqld/mysqld.sock' \
  < config/provision-mcpagent.sql
```

The script grants only table-level SELECT on those four existing tables. It
does not create a database, change the schema or modify the collector account.
It refuses missing/short passwords, a missing database or required table, and
any existing account named `mcpagent`. Do not use `--force`; if execution fails
after account creation, inspect the partial grants before retrying. Account DDL
is not transactional.

To use the reusable file, supply `@mcpagent_password` with at least 16 characters
in the same client session through a protected input file. Password hashing
uses the [documented MariaDB account syntax](https://mariadb.com/docs/server/reference/sql-statements/account-management-sql-statements/create-user#identified-by-password-password_hash)
and avoids placing the supplied password in dynamically constructed SQL text.
The report excludes authentication material. Keep the credentials file for the
MCP application; see the [integration guide](../mcp/README.md) for configuration.

This account belongs only to the Tapo MCP application. Each additional
application uses a unique database username and its own protected credentials.
Its administrator supplies separate table grants; writable applications receive
only the needed INSERT, UPDATE or DELETE privileges. The Tapo MCP reader stays
read-only, while the independent collector keeps its existing ingestion writes.
See the [application file reference](../../mariadb-mcp/docs/mariadb-mcp-app-format.md)
for identity and permission declarations. JSON configuration does not grant
database privileges.

Preflight checks and prepared CREATE/GRANT syntax were verified against MariaDB
11.8 over the target's Unix socket, including quotes/backslashes in a test
password and `NO_BACKSLASH_ESCAPES`. Missing/short password, missing database,
missing tables and existing-account refusal checks passed. Collector and SELECT-only reader provisioning and socket authentication have
been verified. Run the scripts yourself to provision your own database; the
repository contains reusable SQL and no generated passwords.
