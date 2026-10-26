# dot-guest

The agent that runs inside each Mac for Dot Home. It runs as a LaunchAgent in the
signed-in user's desktop session (a small supervisor that starts the current version,
see [Updates](#updates)), waits for the Dot Home server on TCP port 7466,
checks that the server knows this Mac's secret (a mutual HMAC handshake, see
`crates/dot-wire`), and then carries out its requests: screenshots, the accessibility
tree, clicks and typing, shell commands, files, apps, Safari, Shortcuts and
notifications.

It uses only tools that ship with macOS (`osascript`, `screencapture`, `sips`, `zsh`,
`open`, `mdfind`, `sdef`, `hdiutil`, ...), so it needs no private APIs. Every command it
starts runs in its own process group with a deadline; a cancelled or timed-out call
never leaves processes behind.

## Golden image (Dot Home VMs)

Inside the VM, logged in as the user the agent will run as, either run the server's
install script (`curl -fsSL https://<server>/guest/install.sh | sh`) or, with a
downloaded binary:

```sh
./dot-guest install              # copies itself into ~/Library/Application Support/Dot Guest
dg="$HOME/Library/Application Support/Dot Guest/launcher/dot-guest"
"$dg" prepare-image              # add --dry-run to only print the steps
"$dg" doctor
```

No admin password is needed and nothing goes in `/usr/local/bin`. The examples below
write `dot-guest` for that launcher path.

`prepare-image` is safe to run again. It keeps Safari as the default browser, turns on
Safari's developer features, stops sleep and the screen saver, stops automatic macOS
update restarts, and opens each Privacy pane the agent needs. A few things can only be
done by hand (automatic login, Safari's "Allow JavaScript from Apple Events"); it says
which.

VMs need no pairing. The server attaches a small FAT disk labelled `DOTCONFIG` with
`dot-guest.json`:

```json
{ "instance_id": "inst_123", "secret": "<32 random bytes, standard base64>", "port": 7466 }
```

`port` is optional. At boot the agent waits up to 60 s for that disk to mount.

## Remote Macs (pairing)

For a real Mac on your network, create a `remote` Mac in the portal, copy the pairing
string it shows once, and run on that Mac:

```sh
dot-guest pair 'dotpair:v1:<instance_id>:<secret>:<port>'
```

The format is `dotpair:v1:` followed by three `:`-separated fields: the instance id
(letters, digits, `-`, `_`, `.`), the secret as unpadded base64url, and the TCP port.
`pair` saves `~/Library/Application Support/Dot Guest/config.json` (mode 0600) and
installs the LaunchAgent. Add `--dry-run` to see what it would do.

Configuration is read from, in order: `dot-guest run --config <file>`, the `DOTCONFIG`
disk, then the paired file.

## Permissions

Grant these to `~/Library/Application Support/Dot Guest/launcher/dot-guest` (the binary
launchd runs) in System Settings > Privacy & Security:

| Permission | Why |
| --- | --- |
| Accessibility | Read the UI tree, press buttons, click and type. |
| Screen & System Audio Recording | Screenshots that include other apps' windows. |
| Full Disk Access | Read Mail, Messages, Safari data and other protected folders. |
| Automation | Control System Events, Safari and other scriptable apps. macOS asks the first time; click Allow. |

Safari also needs Develop > Allow JavaScript from Apple Events for reading pages, DOM
actions and JavaScript.

## Commands

| Command | What it does |
| --- | --- |
| `dot-guest supervise` | What the LaunchAgent starts: runs the current version's `run` and restarts or rolls it back. |
| `dot-guest run` | The daemon. Started by `supervise`; older installs start it directly. |
| `dot-guest install` | Copy this binary into the support folder (as the launcher and as the current version) and add `~/Library/LaunchAgents/dev.dot.guest.plist`. Can be rerun; restarts a running agent. |
| `dot-guest uninstall` | Remove the LaunchAgent. `--purge` also removes the support folder (installed versions and the paired configuration). |
| `dot-guest pair <string>` | Save a remote Mac's configuration and install. |
| `dot-guest doctor` | Checklist of configuration, LaunchAgent (including that it runs `launcher/dot-guest supervise`), port, permissions, Safari and tools, each with a fix. |
| `dot-guest prepare-image` | One-time golden image setup. |

`doctor` reports permissions as the running agent sees them (it writes
`~/Library/Application Support/Dot Guest/status.json`); without a running agent it
falls back to checking from Terminal and says so.

Logs: `~/Library/Logs/Dot Guest/dot-guest.log` (trimmed when over 20 MB). Logs never
contain secrets, commands' output or file contents.

## MCP servers on the Mac

The `mcp` op lets the host use MCP servers that apps on this Mac provide (`Discover`,
`ListTools`, `CallTool`; see `McpOp` in `crates/dot-wire`). Discovery looks in, in this
order, and keeps the first of any two entries for the same command and arguments or the
same address:

| Source | Where |
| --- | --- |
| Apps with their own server | `Xcode.app` (`xcrun mcpbridge`, Xcode 26.3 and later) and `Figma.app` (`http://127.0.0.1:3845/mcp`, once turned on in Figma), in `/Applications` or `~/Applications`. The table is `KNOWN_APPS` in `src/ops/mcp/discover.rs`. |
| Claude | `~/Library/Application Support/Claude/claude_desktop_config.json`, `mcpServers` |
| Claude Code | `~/.claude.json`, top-level (user) `mcpServers` |
| Cursor | `~/.cursor/mcp.json`, `mcpServers` |
| VS Code | `~/Library/Application Support/Code/User/mcp.json`, `servers` |

Missing files are normal and files that aren't valid JSON are skipped (with a warning
in the log). Apps keep fixed ids (`xcode`, `figma`). A server from a config file gets
its name plus a hash of where it was found and what it runs (`Things 3` becomes
`things_3_1a2b3c4d`). The host stores the owner's Allow or Ask by id, so an id must
never come to mean another command: renaming a server or changing its command or
arguments gives it a new id (it is then off until allowed again), and only its
environment can change under the same id.

Command servers are started directly, never through a shell, in the home folder, in
their own process group. A bare command name (`npx`, `uvx`) is looked up in the login
shell's PATH, read once when the agent starts (or `/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin`
if that fails), or in the config's own `PATH`. The config's environment is added, minus
names that make programs load extra code (`ZDOTDIR`, `BASH_ENV`, `ENV`, `NODE_OPTIONS`,
`DYLD_*`, `LD_*`, `PYTHONSTARTUP`, `PERL5OPT`, `RUBYOPT`, `JAVA_TOOL_OPTIONS`,
`_JAVA_OPTIONS`), which are also removed from the agent's own environment.

One session per server is kept and reused. A request that runs out of time is
cancelled (`notifications/cancelled`) and its late answer skipped; a server that misses
two answers in a row, exits, or whose config changes is stopped, as is one whose call
the host abandoned mid-request. Sessions unused for 10 minutes are closed. Config files
are parsed again only when their size or modification time changes; a file caught
half-written keeps its last good parse.

The rules:

- `Discover` only reads files and checks that apps exist. It never starts anything.
- The host only names a server by its id. Every `ListTools` and `CallTool` runs
  discovery again and looks the id up, so what is started or connected to always comes
  from this Mac's files, never from the host. An id that is gone is `NOT_FOUND`.
- Each server has a `fingerprint`: SHA-256 of its source, command, arguments and
  environment (names and values), or of its URL. The host stores it with the owner's
  approval and treats the server as off once it changes. Ids never include the
  environment and never come to mean a different command.
- Environment values from config files (often API keys) never leave the Mac. The
  `detail` the host sees is the command and its arguments, or the address, with
  anything that looks like a credential shown as `•••`: values after options named like
  key, token, secret, password, auth or bearer, `NAME=value` for such names,
  `Authorization:` and `Bearer` values, and arguments shaped like keys (`sk-`, `ghp_`,
  `github_pat_`, `xox`, `AKIA`, long hex or base64), and URL passwords and secret
  query values. A server's stderr and HTTP error pages quoted in errors get the same
  treatment, plus any of the config's environment values, and are cut to 300
  characters. Logs name only the file and the kind of parse error, never its text.
- HTTP servers are used only at plain `http://` on `127.0.0.1`, `localhost` or `[::1]`.
  Remote addresses (they aren't on the Mac) and SSE-only servers are left out.

## Updates

The host streams a new binary over the authenticated link (`update` op: Begin, Chunk,
Commit, Abort; see `crates/dot-wire`). The agent offers `update` in its capabilities
only when it runs under the supervisor.

```text
~/Library/Application Support/Dot Guest/
  launcher/dot-guest            what launchd runs; replaced only by `dot-guest install`
  versions/<version>/dot-guest  the newest 3 are kept, plus current and previous
  current  -> versions/<version>
  previous -> versions/<version>
  update/<version>.partial      a transfer in progress
  trial.json                    {version, sha256, started_at, attempts} for a new version
  bad.json                      versions that were rolled back
```

Commit checks the size, the SHA-256 and that `<new binary> --version` prints the
version, moves it into `versions/`, repoints `previous` and then `current` (each a new
symlink renamed over the old one), writes `trial.json`, replies, and exits 500 ms later
with status 75. The supervisor starts `current` right away.

A new version is on trial until it is healthy: after its first successful handshake
with the host, or after its listener has been up for 2 minutes without a crash,
whichever comes first. It then deletes `trial.json`. The time rule means a server outage
after an update never rolls back a good agent; only a crash loop does:

- Each exit while on trial counts as a failed start and it is restarted. On the third
  failed start it is rolled back.
- Rolling back records the version and its SHA-256 in `bad.json`, deletes `trial.json`,
  points `current` at `previous` (or removes `current`, so the launcher's own version
  runs) and logs it. The host can't install that exact binary again; a new build (a
  different SHA-256, even with the same version) can.

Other exits restart with a backoff of 1, 2, 5, 10, then 30 s (reset after a minute of
running). SIGTERM to the supervisor is passed on to the agent.

If an agent that has this code is started directly by an older LaunchAgent (no
supervisor), Commit still installs the files but replies `restarting: false` with a
hint and keeps running: launchd would only restart the same old binary. Running
`dot-guest install` once switches the LaunchAgent to the launcher.

### Why a separate launcher

macOS ties Accessibility, Screen Recording and Full Disk Access to the code signature of
the *responsible process*. An ad-hoc signed build gets a new identity every time, so if
launchd started each updated binary directly, every update would silently lose those
permissions. launchd runs the launcher instead, and the launcher starts the current
version as a child. A child process is attributed to its parent for privacy checks, the
same way a command run in Terminal uses Terminal's permissions, so the grants stay with
the launcher, which only a manual reinstall replaces.

This has not been confirmed on a real Mac or VM yet: grant the permissions, push an
update, and check that `dot-guest doctor` and a screenshot still work. Signing every
build with the same Developer ID in CI would make the question moot.

