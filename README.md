# galaxy-buds-control

Control a Samsung Galaxy Buds2 (`SM-R177`) from Linux: ANC and ambient sound,
equalizer, touch-and-hold actions per earbud, touchpad lock, game mode,
sidetone, Find My Earbuds — everything the Galaxy Wearable app changes, over the
buds' own SPP channel.

Three parts:

| Part | What it is |
|---|---|
| `budsd` | Daemon. Owns the single RFCOMM/SPP link, keeps the state, serves clients on `$XDG_RUNTIME_DIR/galaxybuds/ipc.sock` and rewrites `$XDG_RUNTIME_DIR/galaxybuds/state.json` on every change. |
| `buds` | CLI and TUI. `buds` with no arguments opens the TUI; `buds <command>` is for scripts and keybinds. |
| `home.buds` | Omarchy bar widget in `../omarchy-plugins/home.buds`, watching the state file and calling `buds`. See its `UI-PLAN.md`. |

## Install

```bash
./install.sh
```

That builds release binaries into `~/.local/bin`, installs and enables the
`galaxybuds` user service, links the bar widget and enables it.

## Use

```bash
buds                      # TUI
buds status               # one-line summary
buds status --json        # machine readable
buds anc anc              # off | anc | ambient | cycle
buds ambient 2            # 0..2
buds eq soft              # normal | bass-boost | soft | dynamic | clear | treble-boost
buds touch left volume    # voice-assistant | noise-control | volume | spotify | other
buds hold-anc off anc     # which modes the long press cycles through
buds touchpad-lock on
buds game-mode on
buds sidetone on
buds one-earbud-anc on    # keep noise control working with one bud in
buds find start           # beep; buds find stop to silence
buds connect              # connect through BlueZ, audio profiles included
buds disconnect
buds watch                # JSON state line on every change
```

A Hyprland binding, for example in `~/.config/hypr/bindings.conf`:

```
bindd = SUPER, N, Cycle buds noise control, exec, ~/.local/bin/buds anc cycle
bindd = SUPER SHIFT, N, Open the buds panel, exec, omarchy-shell home.buds toggle
```

## How it works

The buds speak a framed message protocol over RFCOMM:

```
0xFD | int16 LE header | msgId | payload | CRC16-CCITT LE | 0xDD
       header = 1 + len(payload) + 2;  bit 0x1000 = response, 0x2000 = fragment
```

Message ids and payload layouts are mirrored from
[GalaxyBudsClient](https://github.com/timschneeb/GalaxyBudsClient); framing and
CRC come from the [`galaxy_buds_rs`](https://github.com/JojiiOfficial/GalaxyBuds-rs)
crate.

Two things are not obvious:

- **The status channel is not the one SDP points at.** The buds publish several
  RFCOMM records; the standard SPP record (`0x1101`) answers version and manager
  messages only. The real control channel is 20 on this firmware. BlueZ's
  attribute query returns these records without protocol descriptors, so the
  channel is found by probing and cached in `~/.config/galaxybuds/config.json`.
- **The buds do not echo a change back to the session that made it.** Settings
  written here are applied on the device (verified by reconnecting and reading
  the state fresh), but no update frame follows, so the daemon mirrors each
  successful write into its own state the way the phone app does.
- **Opening the RFCOMM socket pages the buds.** A daemon that just retries
  therefore creates the link it is meant to be watching: BlueZ reports the buds
  as connected, nothing negotiates an audio profile behind it, and the user gets
  a widget that says "connected" over silence. `budsd` gates on
  `org.bluez.Device1.Connected` (polled every 1.5 s over the system bus) and
  attaches only once the system itself holds the device, so `connected` in the
  state file means the same thing it means everywhere else. `buds connect` asks
  BlueZ for the connection rather than opening a socket, which is what brings
  A2DP up with it.

The state file carries a `heartbeat` field, rewritten every 60 seconds whether
or not anything changed: `updated-at` is the last state change, so on its own it
cannot tell "daemon idle, buds away" from "daemon dead".

Settings live on the buds, so a change made here also applies when they are
connected to a phone.

Only one process may hold the SPP link: stop `galaxybuds.service` before running
GalaxyBudsClient, and vice versa.

## Development

```bash
cargo run --bin budsd            # daemon in the foreground
BUDS_DEBUG=1 cargo run --bin budsd   # log every frame in and out
cargo run --example dump         # raw frames with the status payload annotated
cargo run --example probe        # probe every RFCOMM channel
cargo test
```
