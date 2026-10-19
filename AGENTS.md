# Adding `xvpn diagnose` tests

`xvpn diagnose` has two interfaces:

- `xvpn diagnose` opens the checkbox selector.
- `xvpn diagnose <testname>` runs one named test without interaction.
- `xvpn diagnose all` runs every test in execution order.

## Add a test

1. Add one entry to `DIAGNOSTIC_TESTS` in `src/cli/commands/diagnose.rs`.
2. Give it a stable kebab-case name, a short checkbox label, and a safe
   default.
3. Add the name to `run_diagnostic_test`.
4. Document the exact commands in the plan/README with separate **Start**,
   **Test**, and **Cleanup / end** sections.
5. Register cleanup before changing routing or configuration. Profile tests must
   run all profiles consecutively and defer their single `xvpn off` cleanup
   until every profile has been tested. Other tests must attempt `xvpn off` on
   success, failure, timeout, and Ctrl-C.
6. Enumerate every saved profile in configured order; never test only the
   active profile.
7. Use only `curl -4 ip.me` and `curl -4 ifconfig.me` for automated network
   checks. App checks are manual and must not add curl assertions.
8. Snapshot existing app/site entries before using `Firefox` or `ip.me`, then
   restore the original state rather than blindly removing an entry.
9. Log the exact command/action, output, status, duration, and cleanup result
   to `~/.local/state/xvpn/diagnose.log`.

## Validation

Run a single test before running the full suite:

```sh
xvpn diagnose direct
xvpn diagnose sites
xvpn diagnose profiles
xvpn diagnose profile-commands
xvpn diagnose modes
xvpn diagnose firefox
xvpn diagnose all
```

Then run:

```sh
cargo fmt --check
cargo test
cargo check
```

Never call `xvpn diagnose` from inside a diagnostic test. Routing tests always
clean up with `xvpn off`.
