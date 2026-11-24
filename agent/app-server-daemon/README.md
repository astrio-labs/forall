# forall-app-server-daemon

> `forall-app-server-daemon` is experimental and its lifecycle contract may
> change while the daemon flow is still being developed.

`forall-app-server-daemon` backs the machine-readable `forall app-server`
lifecycle commands used by remote clients such as IDE extensions and automation tools.
It manages a detached app-server process on the local machine, typically for
instances launched over SSH or other headless environments.

## Running the daemon

Run `forall app-server daemon start` to launch the app-server from the same
executable as the CLI. Use `restart` after rebuilding it, or use the bootstrap
command to monitor the executable for a locally installed replacement.
The monitor compares executable identities and refreshes a running app-server
when the installed file changes.

## Lifecycle semantics

`start` is idempotent and returns after app-server is ready to answer the normal
JSON-RPC initialize handshake on the Unix control socket.

`restart` stops any managed daemon and starts it again.

`stop` sends a graceful termination request first, then sends a second
termination signal after the grace window if the process is still alive.

All mutating lifecycle commands are serialized per `FORALL_HOME`, so a concurrent
`start`, `restart`, `stop`, or `bootstrap` does not race another in-flight
lifecycle operation.

## State

The daemon stores its local state under `FORALL_HOME/app-server-daemon/`:

- `settings.json` for persisted launch settings
- `app-server.pid` for the app-server process record
- `app-server-updater.pid` for the pid-backed standalone updater loop
- `daemon.lock` for daemon-wide lifecycle serialization
