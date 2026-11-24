# App Server Test Client
Quickstart for running and hitting `forall app-server`.

## Quickstart

Run from `<reporoot>/agent`.

```bash
# 1) Build debug forall binary
cargo build -p forall-cli --bin forall

# 2) Start websocket app-server in background
cargo run -p forall-app-server-test-client -- \
  --forall-bin ./target/debug/forall \
  serve --listen ws://127.0.0.1:4222 --kill

# 3) Call app-server (defaults to ws://127.0.0.1:4222)
cargo run -p forall-app-server-test-client -- model-list
```

`send-message` and `send-message-v2` handle `request_user_input` server requests interactively.
When Forall asks a question, choose a numbered option (or `o` for a free-form answer when offered)
and the client will send the response and continue streaming the same turn.

## Removing a remote plugin

To uninstall a plugin from the active account, run the following command.

```bash
cargo run -p forall-app-server-test-client -- \
  --forall-bin ./target/debug/forall \
  plugin-remote-uninstall \
  --remote-plugin-id <REMOTE_PLUGIN_ID> \
  --confirm-account-mutation
```

## Watching Raw Inbound Traffic

Initialize a connection, then print every inbound JSON-RPC message until you stop it with
`Ctrl+C`:

```bash
cargo run -p forall-app-server-test-client -- watch
```

## Testing Thread Rejoin Behavior

Build and start an app server using commands above. The app-server log is written to `/tmp/forall-app-server-test-client/app-server.log`

### 1) Get a thread id

Create at least one thread, then list threads:

```bash
cargo run -p forall-app-server-test-client -- send-message-v2 "seed thread for rejoin test"
cargo run -p forall-app-server-test-client -- thread-list --limit 5
```

Copy a thread id from the `thread-list` output.

### 2) Rejoin while a turn is in progress (two terminals)

Terminal A:

```bash
cargo run --bin forall-app-server-test-client -- \
  resume-message-v2 <THREAD_ID> "respond with thorough docs on the rust core"
```

Terminal B (while Terminal A is still streaming):

```bash
cargo run --bin forall-app-server-test-client -- thread-resume <THREAD_ID>
```
