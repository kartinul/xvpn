# xvpn

`xvpn` is a user-space VPN manager for multiple `vless://` profiles. It runs
xray for the proxy and sing-box for system routing, with a user supervisor that
reconciles the selected mode every few seconds.

## What the binary does

The released `xvpn` binary:

- detects macOS or Linux at compile time and selects the native network
  integration;
- stores configuration in `~/.config/xvpn` and runtime state/logs in
  `~/.local/state/xvpn`;
- creates and repairs its xray/sing-box configuration;
- manages profiles, selective apps/sites, and `on`/`default`/`off` modes;
- runs the supervisor directly with `xvpn supervise`;
- installs the per-user supervisor agent with `xvpn agent install`;
- installs a narrow, root-owned sudo setup for macOS TUN routing.

No `make` command is required at runtime. `install.sh` is only a convenience
installer for building from source, installing third-party dependencies, and
loading the user agent.

## Required proxy engines

The binary is not a proxy engine itself. It launches these two external
executables:

- `xray`
- `sing-box`

Install both using your operating system's package manager or trusted upstream
release packages, and make sure both are in `PATH`. xvpn checks for them before
starting the supervisor and reports the missing executable explicitly.

This separation is intentional: silently downloading executable code from the
internet would make a public standalone binary harder to audit and would
create an unsafe update path.

## Install from a release

1. Download the binary matching your OS and CPU architecture.
2. Make it executable and put it in a directory on `PATH`, for example:

   ```sh
   install -m 755 xvpn ~/.local/bin/xvpn
   ```

3. Install `xray` and `sing-box` as described above.
4. Install the user supervisor:

   ```sh
   xvpn repair
   xvpn agent install
   ```

The agent is user-scoped; it does not run xvpn as a root daemon. On macOS it
uses a LaunchAgent. On Linux it uses a user systemd unit.

## Quick start

Import a profile:

```sh
xvpn import 'vless://...'
```

Select a mode:

```sh
xvpn default       # selective routing
xvpn on            # all traffic (macOS: needs `xvpn agent install` once)
xvpn off           # stop routing
xvpn 1             # select profile 1; enables full routing if currently off
```

Configure selective routing:

```sh
xvpn apps set Telegram
xvpn sites set netflix.com
```

Inspect state and logs:

```sh
xvpn
xvpn logs
xvpn check
```

Restart the user supervisor after rebuilding:

```sh
xvpn restart
```

Run the interactive end-to-end diagnostic:

```sh
xvpn diagnose
```

The interactive screen shows only numbered friendly task names by default.
Use Up/Down to move, Space to toggle checks, and Enter to run the selected
checks. Each task shows a colored pending/running/pass/fail state and a
determinate progress bar; running tasks use a spinner-like running state and
completed tasks show a tick. Press Left/Right on the focused task to open or
close its detail pane. The pane contains a bounded live-output box with the
commands and output from the running check; output stays hidden from the
collapsed list. After all checks finish the title bar switches to
"results shown" and the progress bar turns green or red depending on whether
every selected check passed. Press `q` or Escape before running to quit.

The default selections cover direct connectivity and both all-profile checks.
Site, routing-mode, and Firefox checks are opt-in because they change routing
or configuration. Every check cleans up and ends with routing off.

Add `--yes` (or `-y`) to bypass the interactive menu entirely and run the
default checks non-interactively, printing results to stdout. This makes
`xvpn diagnose --yes` fully agent-readable.

Each check can also run without the menu:

```sh
xvpn diagnose direct
xvpn diagnose sites
xvpn diagnose profiles
xvpn diagnose profiles --yes
xvpn diagnose profile-commands
xvpn diagnose modes
xvpn diagnose firefox
xvpn diagnose all
```

The diagnostic log is written to `~/.local/state/xvpn/diagnose.log`. Network
The profiles check first shows the observed `ifconfig.me` IP for direct mode
and every profile, then asks for one confirmation before running automated
checks. Use `--yes` (or `-y`) to skip that confirmation. The legacy probes
remain `curl -4 ip.me` and `curl -4 ifconfig.me`; if a proxy returns an IPv6
address, diagnostics additionally query an IPv4-only fallback so the result
can be compared as IPv4.
The profile check switches through all profiles without turning routing off
between profiles, then performs one final `xvpn off` cleanup after the last
profile. When running a manual check through a profile, the original
bare-host behavior is preserved, so `xvpn 2 curl -4 ifconfig.me` may expose
the proxy's actual IPv6 egress while the diagnostic fallback reports an IPv4
comparison value. The Firefox check opens `https://ifconfig.me` for a manual
check and does not use curl.

## Privilege behavior

xray and Linux gateway/TProxy routing run as the current user. macOS TUN
routing needs root for sing-box. A LaunchAgent has no terminal, so it cannot
reuse a terminal's sudo ticket. `xvpn agent install` (asks for sudo once) sets up:

- `/usr/local/libexec/xvpn/{xvpn,sing-box}` — root-owned copies of both binaries;
- `/etc/sudoers.d/xvpn` — NOPASSWD only for `xvpn root-run on|default` and
  `pkill` of the exact root sing-box command line.

`root-run` builds the TUN config from code; in default mode it carries over only
plain `domain_suffix` / `process_path_regex` rules from `modes/default.json`.
Nothing the user can edit is run as root. Re-run `xvpn agent install` after
upgrading sing-box or xvpn; `xvpn on` refuses with a clear error while the root
setup is missing or from an older xvpn.

## Building from source

`make` compiles the release binary and copies it to `~/.local/bin/xvpn`:

```sh
make
```

The build also clears the previous xvpn diagnostic, supervisor, xray, and
sing-box logs. To clear those logs without rebuilding, run `make clean-logs`.

The build artifact is also written to `target/release/xvpn`. Make sure
`~/.local/bin` is on your shell's `PATH`; a Makefile cannot change the `PATH`
of the shell that invoked it. The command does not install dependencies,
modify the user agent, or change VPN state. Runtime setup is handled by xvpn
itself.

For a build without installing the binary:

```sh
cargo build --release
./target/release/xvpn --help
```

Useful checks:

```sh
cargo test
cargo clippy -- -D warnings
```

The project currently targets macOS and Linux. Other operating systems are
rejected explicitly rather than falling back to an incorrect network setup.
