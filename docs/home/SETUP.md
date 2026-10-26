# Setting up Familiar

You need a Linux server with hardware virtualization (Intel VT-x or AMD-V, enabled in the
BIOS), Docker, and a domain name pointing at it. Plan on about 8 GB of RAM and 60 GB of
disk per running Mac, plus 20 GB for the base image.

Apple's macOS licence allows virtualization only on Apple hardware. Running macOS on
other hardware is your responsibility. A real Mac works too, as a remote Mac (step 6).

## 1. Start the container

```bash
git clone https://github.com/reallukedev/familiar && cd familiar
```

```bash
cat > deploy/home/.env <<EOF
DOT_HOME_PUBLIC_URL=https://home.example.com
DOT_HOME_ACME_EMAIL=you@example.com
KVM_GID=$(getent group kvm | cut -d: -f3)
EOF
```

```bash
docker compose -f deploy/home/compose.yaml --env-file deploy/home/.env up -d --build
```

Port 443 must reach the server from the internet for Let's Encrypt. If you already run a
reverse proxy (Caddy, Traefik, Cloudflare Tunnel), leave `DOT_HOME_ACME_EMAIL` empty,
publish `8443` only to the proxy, and set `DOT_HOME_TRUSTED_PROXIES` to the proxy's
address so sign-in limits see real client IPs.

Check the host:

```bash
docker compose -f deploy/home/compose.yaml exec dot-home dot-home doctor
```

## 2. Create the first admin

The server prints a one-time setup link:

```bash
docker compose -f deploy/home/compose.yaml logs dot-home | grep setup
```

Open it, choose a username and password, then set up two-factor authentication with any
authenticator app. Two-factor is required for everyone by default (Admin > Settings).

## 3. Put dot-guest on the server

On any Mac with Rust installed:

```bash
sh scripts/build-guest.sh
```

```bash
docker cp target/guest/dot-guest dot-home:/var/lib/dot-home/guest/dot-guest
```

## 4. Build the macOS base image

macOS Tahoe 26 is the newest version a server can run: Apple made it the last release
for Intel Macs, and OSX-KVM emulates an Intel Mac. For macOS 27 and later, add a real
Apple-silicon Mac as a remote Mac instead.

In the portal, open **Admin > macOS Image**, choose a version and press **Build Image**.
The server downloads the installer from Apple (a few minutes), then starts a blank
builder Mac. Open its screen and:

1. In the boot picker choose **macOS Base System** with the arrow keys and Return (the
   pointer only works once macOS is running), open Disk Utility, erase the
   largest QEMU disk as APFS, then quit Disk Utility and **Reinstall macOS** onto it.
   This takes 30 to 60 minutes and restarts a few times.
2. Finish Setup Assistant. Create one user, turn on **automatic login** (Users & Groups),
   skip Apple Account sign-in (each person signs in on their own Mac later).
3. Open Terminal and run:

   ```bash
   curl -fsSL https://home.example.com/guest/install.sh | sh
   ```

   It installs the agent into `~/Library/Application Support/Dot Guest` (no admin
   password) and prints its path. Then:

   ```bash
   dg="$HOME/Library/Application Support/Dot Guest/launcher/dot-guest"
   "$dg" prepare-image
   ```

   It turns off sleep and the screen saver, enables Safari's automation settings, keeps
   Safari as the default browser, installs Homebrew on Apple silicon (Homebrew no longer
   supports Intel, which every Mac on this server is, so those use download links), and
   opens each Privacy pane that needs a click, with a Finder window showing dot-guest to
   drag into each list:
   **Accessibility**, **Screen Recording** and **Full Disk Access** for
   `~/Library/Application Support/Dot Guest/launcher/dot-guest`, and
   **Automation** prompts the first time each app is scripted.
4. Run `"$dg" doctor` until every line passes.

When the builder's agent connects, the portal shows **Use as Base Image**. Press it. The
builder shuts down and its disk becomes the base every new Mac is copied from (copies
are instant and only store their own changes).

## 5. Invite people

**Admin > People > Invite** makes a single-use link (member or admin). Each person signs
up, sets up two-factor, and creates their Mac from the home page. Nobody, including
admins, can see another person's Mac, files, activity or tokens.

## 6. Connect an assistant

On a Mac's **Connect** page:

- **ChatGPT**: Settings > Connectors > Add, URL `https://home.example.com/mcp`. You'll
  sign in to Familiar and choose which Mac to connect.
- **Claude** (claude.ai, desktop, mobile): Settings > Connectors > Add custom connector,
  same URL.
- **Claude Code**: create a personal token on the Connect page, then

  ```bash
  claude mcp add --transport http familiar https://home.example.com/mcp --header "Authorization: Bearer dot_pat_..."
  ```

## Remote Macs (a real Mac)

Create a Mac with **Use a Mac I already have**, enter its private address (Tailscale or
LAN, port 7466), and copy the pairing code. On that Mac:

```bash
curl -fsSL https://home.example.com/guest/install.sh | sh
```

```bash
"$HOME/Library/Application Support/Dot Guest/launcher/dot-guest" pair 'dotpair:v1:...'
```

The link between server and Mac is authenticated but not encrypted, so it's only allowed
on private addresses. Use Tailscale across the internet.

## Operating it

| Task | How |
| --- | --- |
| Health | `dot-home doctor`; Admin > Overview shows the same checks |
| Logs | `docker compose logs -f dot-home` (`DOT_HOME_LOG=debug` for more) |
| Metrics | `/metrics` in Prometheus format, answered only to loopback callers (run a scraper in the same network namespace) |
| Lost password | `docker compose exec dot-home dot-home reset-password <user>` prints a link. Their encrypted Macs need their recovery key on that page; without it those Macs are erased. |
| Backup | `dot-home backup /var/lib/dot-home/backup.db`, plus `master.key`, `images/` and `instances/` (stop or hibernate Macs first for consistent disks) |
| Upgrade | `git pull && docker compose up -d --build`. Running Macs hibernate and resume, except encrypted Macs whose owner hasn't signed in since the last start (their memory can't be encrypted without the key). After any restart, people sign in once to unlock their Macs. |
| Resource limits | Admin > Settings: memory budget and Macs running at once. Idle Macs sleep after 30 minutes and hibernate after 4 hours by default (per Mac in Settings). |

Keep `master.key` safe: it encrypts two-factor secrets and Mac pairings. Without it,
everyone has to set up two-factor again and every Mac has to be re-paired.

Macs are encrypted with each person's own key, which only their password or recovery
key opens (docs/home/SECURITY.md "Encryption at rest"). Backups of `instances/` stay
encrypted; nobody, admins included, can read them without one of those.

## Updates

Familiar updates itself from this repository's GitHub releases. **Admin > Software
Update** sets the channel for the server and the default for every Mac's agent:

- **Stable**: tagged releases (`vX.Y.Z`). The default.
- **Nightly**: a build of every push to `main`. Newest changes; may break.
- **Off**: never checks.

Server updates install in a two-hour window you choose (3:00 to 5:00 AM by default),
or right away with **Update Now**. The server restarts in place in a few seconds; Macs
keep running and reconnect. A new build is on trial for its first minute: if it fails
to start twice, Familiar goes back to the previous version on its own, and **Roll Back**
does the same by hand. Each Mac's **Settings > Agent Updates** follows the server
setting, or its own channel, or Off. Agents update when their Mac has been idle for 10
minutes, and roll themselves back if the new version doesn't come up.

Updates only install if their manifest is signed by the key built into your server, and
every download is checked against the signed SHA-256. Local builds have no key, so they
never auto-update.

### Setting up releases (once, for the repository owner)

```bash
cargo run -p dot-home -- keygen
```

Add the printed private key as the Actions secret `DOT_UPDATE_SIGNING_KEY` and the
public key as the Actions variable `DOT_UPDATE_PUBLIC_KEYS`. Pushes to `main`
then publish the `nightly` release and `ghcr.io/<owner>/familiar/dot-home:nightly`;
tags like `v0.3.1` (matching `crates/dot-home/Cargo.toml`) publish a stable release and
`:latest`. Use the published image in `compose.yaml` (`image:` instead of `build:`) so
your server has the key.

Optional: the secrets `MACOS_CERT_P12`, `MACOS_CERT_PASSWORD` and `MACOS_SIGN_IDENTITY`
(a Developer ID Application certificate) sign the agent with a stable identity. Without
them the agent is ad-hoc signed; its supervisor keeps macOS privacy permissions across
updates by being the process the permissions are granted to (see `crates/dot-guest`).

What updates don't cover: QEMU, OSX-KVM firmware and the container's OS packages come
from the image, so pull a new image now and then (`docker compose pull && docker compose
up -d`); Macs hibernate and resume across that.
