# slingr

Throw a window at a task.

A workspace picker for [AeroSpace](https://nikitabobko.github.io/AeroSpace/).
Press one key, pick a task, and the focused window moves there.

Tasks are listed with the icons of the applications they hold, rarest first —
so a task with an editor and a spreadsheet in it reads as one before you have
read its name, rather than as another row of browsers.

## Install

```sh
./install.sh
```

That builds both halves — `cargo build --release` for the binary and `swiftc`
for the panel — links `~/.local/bin/slingr`, and writes the herdr plugin with
the absolute path it needs.

Bind it in `~/.aerospace.toml`:

```toml
ctrl-alt-cmd-s = 'exec-and-forget /Users/justin/.local/bin/slingr'
ctrl-alt-cmd-i = 'exec-and-forget /Users/justin/.local/bin/slingr jump'
ctrl-alt-cmd-b = 'exec-and-forget /Users/justin/.local/bin/slingr board'
```

`cargo build` alone does not build the panel. Without it slingr falls back to
an AppleScript dialog, and macOS then asks once to let AeroSpace control System
Events — `slingr paths` says which of the two you are getting.

## Use

| | |
|---|---|
| `slingr` | the picker |
| `slingr jump` | the picker, opened on the task list — go somewhere |
| `slingr board` | every window at once, grouped by task, drag to move them |
| `slingr sync` | teach AeroSpace every task, so empty ones persist |
| `slingr snapshot` | record where every window is |
| `slingr restore` | put them back after an AeroSpace restart |
| `slingr following` | windows that come along to every task; `--prune` forgets the dead ones |
| `slingr follow` | bring them to the workspace in front, and focus the matching herdr tab |
| `slingr goto` | go to the workspace matching herdr's focused tab, once |
| `slingr stats` | where windows actually go |
| `slingr probe` | ask AeroSpace what it sees, changing nothing |
| `slingr paths` | the files sling reads and writes |

One key, four tabs. The panel opens on the focused window; ⇥ cycles.

| tab | question |
|---|---|
| **sling once** | where does *this* window go? |
| **sling many** | which windows belong together, and where? |
| **jump to** | take me to a task |
| **board** | show me everything, and let me sort it |

**sling many** lists every window grouped by workspace, crowded one first.
Select as many as you like, then choose where they go — this is how a workspace
that has collected dozens of windows gets drained. Windows sharing a title show
their id so you can tell them apart.

**board** is the whole machine as tiles, one per task, with windows dragged
between them. Nothing moves until you confirm, so a tidy-up is one visit rather
than one visit per window; escape puts everything back before it closes. It
exists because Ctrl-↑ groups by macOS Spaces, which have nothing to do with
AeroSpace workspaces — see `docs/FINDINGS.md`.

## Following herdr

`slingr watch` polls herdr's focused tab and brings AeroSpace across to the
matching workspace — the other direction of the sling.

```sh
slingr watch              # follow along
slingr watch --dry-run    # say what it would do, change nothing
```

A task with nothing in it yet is still somewhere to go: the windows that belong
everywhere arrive with you, so a new tab lands you in a terminal rather than on
a blank screen. Creating or renaming a tab syncs it into AeroSpace first, so
its workspace is there before anything tries to reach it.

It waits for the tab to hold still before following it, and coalesces
bursts of events, so flicking through tabs does not queue a workspace switch
for each one passed — and a reported herdr bug that can emit ~29 phantom focus
events a second cannot make it thrash.

Nothing stays resident. Both halves are callbacks:

```toml
# ~/.aerospace.toml — windows that belong everywhere catch up on any
# workspace change, whatever caused it
exec-on-workspace-change = ['…/slingr', 'follow']
```

```toml
# plugin/herdr-plugin.toml — herdr runs this when the focused tab changes
[[events]]
on = "tab.focused"
command = ["…/slingr", "goto"]
```

Install the herdr half with `herdr plugin link ~/Dev/slingr/plugin`.

`slingr watch` still exists for somewhere the callbacks cannot be installed. It
subscribes rather than polls, and `--poll` is the last resort. A daemon is the
worse design though: one that quietly dies is indistinguishable from a broken
follow list, which is exactly how it failed the first time.

## Surviving a restart

AeroSpace does not remember which workspace a window belongs to. Restart it and
everything lands in whatever each monitor happens to be showing — which, with
forty-odd windows, is an evening's sorting lost.

`slingr restore` puts windows back, from two sources:

| | |
|---|---|
| the action log | every window you deliberately slung, and where you sent it |
| `layout.json` | a snapshot of where everything was, taken automatically |

The log is asked first, because it records **intent** and the snapshot records
**happenstance**. That matters most after a reboot: every window id is new, and
the automatic snapshot writes down the scattered state as soon as anything
moves — so the snapshot ends up agreeing that your windows belong in the pile
they landed in. The log does not, because you never asked for that.

Windows that follow you are skipped: they are wherever you are on purpose.

```sh
slingr restore --dry-run    # list what would move
```

## Windows that belong everywhere

Some windows are not part of any one task. The picker offers two rows for it:

| | |
|---|---|
| `Show this window on all workspaces` | one window of several — the herdr terminal among other terminals |
| `Show every Finder window on all workspaces` | the whole application, for one whose windows are interchangeable and short-lived |

Following an application also stops it dragging you around: clicking a Dock
icon activates the app, which focuses one of its windows *wherever that is*,
and AeroSpace follows focus. If its windows are always with you, there is
nowhere else to be pulled to.

AeroSpace has no sticky windows — a window is in exactly one workspace and
nothing can change that — so this is emulation. `slingr watch` subscribes to
AeroSpace's own `focused-workspace-changed` event, so followers keep up with
*any* workspace change: a keybinding, a click, a herdr tab, or sling itself.

It is cheap because moving a window by id neither focuses it nor switches
workspace, so bringing the follow list raises no further event and costs no
workspace restore. On AeroSpace 0.12 the same two windows took 46 seconds; it
is now about 100ms.

Window ids do not survive an application restart, so the list goes stale when
you quit WhatsApp. `slingr following` marks those entries `gone` and
`--prune` forgets them; re-tagging is one pick.

Draining this way is cheap. Moving a window *out* of the workspace you are
looking at hides exactly one window; switching *into* a crowded workspace
costs about three seconds per window it has to restore. See
[`docs/FINDINGS.md`](docs/FINDINGS.md).

## Where the task list comes from

Four sources, merged:

| | |
|---|---|
| AeroSpace | workspaces that currently hold windows, with their counts |
| herdr | every open tab, `t/forms` → `t-forms` |
| `seen.json` | every name ever used, so a task survives its workspace emptying |
| `workspaces.toml` | the rest — free-form tasks, and tabs not open yet |

Renaming or adding a herdr tab makes it slingable immediately, with nothing to
edit. herdr is asked on each sling and costs about 7ms; if it is not installed
or not answering, the list is simply shorter.

## Configuration

`~/.config/slingr/workspaces.toml`, seeded on first run. It holds the tasks that
are *not* herdr tabs, the group headings, and the order they appear in.

Names may only contain `[A-Za-z0-9._-]`. A `/` hangs AeroSpace, so herdr's
`t/forms` is spelled `t-forms` — typing the slash is fine, it gets translated.

## State

All under `~/.local/state/slingr/`:

| | |
|---|---|
| `follow.json` | windows that come along to every task |
| `pins.json` | tasks kept at the top of their folder |
| `layout.json` | where every window was, for `slingr restore` |
| `actions.jsonl` | one line per sling, including the ones that moved nothing |
| `watch.jsonl` | what `slingr watch` decided, with timings |
| `seen.json` | superseded by `persistent-workspaces`; read once by `slingr sync` so nothing invented before that is lost |

`slingr paths` prints where these live, and whether the panel binary is where it
is expected — a silent fallback to the AppleScript dialog usually means it is
not.

## Documentation

- [`docs/SPEC.md`](docs/SPEC.md) — what this is for and the model behind it
- [`docs/FINDINGS.md`](docs/FINDINGS.md) — AeroSpace and macOS behaviour that
  is not in any documentation, and which the design works around

## Tests

```sh
cargo test
```

`tests/flow.rs` drives the whole flow with a fake window manager, so ordering —
focus before move, which is where the real bug lived — is an assertion rather
than a hope.
