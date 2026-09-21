# sling

Throw a window at a task.

A workspace picker for [AeroSpace](https://nikitabobko.github.io/AeroSpace/).
Press one key, pick a task, and the focused window moves there.

```
＋  new workspace…
──────  tyto  ──────
✓  t-forms
   t-pair
──────  art corner  ──────
●  ac-app  (this window is here)
```

`✓` already holds windows · `●` where this window is now · unmarked, a known
task with nothing in it yet.

## Install

```sh
cargo build --release
ln -sf ~/Dev/sling/target/release/sling ~/.local/bin/sling
```

Bind it in `~/.aerospace.toml`:

```toml
ctrl-alt-cmd-s = 'exec-and-forget /Users/justin/.local/bin/sling'
ctrl-alt-cmd-i = 'exec-and-forget /Users/justin/.local/bin/sling jump'
```

macOS will ask once to let AeroSpace control System Events. That grant is what
draws the dialog; without it nothing appears.

## Use

| | |
|---|---|
| `sling` | the picker |
| `sling jump` | the picker, opened on the task list — go somewhere |
| `sling sync` | teach AeroSpace every task, so empty ones persist |
| `sling snapshot` | record where every window is |
| `sling restore` | put them back after an AeroSpace restart |
| `sling following` | windows that come along to every task |
| `sling follow` | bring them to the workspace in front, now |
| `sling stats` | where windows actually go |
| `sling probe` | ask AeroSpace what it sees, changing nothing |
| `sling paths` | the files sling reads and writes |

One key, two modes. The dialog opens on the focused window, and its first line
switches to selecting several:

```
⇄  several windows…          <- switches the list
＋  new workspace…
──────  tyto  ──────
✓  t-forms
```

Pick `⇄` and the list becomes every window, grouped by workspace with the
crowded one first. Select as many as you like with shift- or cmd-click, then
choose where they go; its own first line switches back. This is how a workspace
that has collected dozens of windows gets drained. Windows sharing a title show
their id so you can tell them apart.

`choose from list` is a flat, single-column AppleScript control with no tab
support, so the mode row is a line you pick rather than a tab you click.

## Following herdr

`sling watch` polls herdr's focused tab and brings AeroSpace across to the
matching workspace — the other direction of the sling.

```sh
sling watch              # follow along
sling watch --dry-run    # say what it would do, change nothing
```

It switches only when the target workspace already holds windows. Showing an
empty workspace blanks the screen, and getting back costs a full restore of
wherever you came from. So a herdr tab starts pulling AeroSpace across as soon
as you have slung something into its task, and is quietly ignored until then:

```
holding: that task holds no windows yet (t-pair)
```

It also waits for the tab to hold still before following it, and coalesces
bursts of events, so flicking through tabs does not queue a workspace switch
for each one passed — and a reported herdr bug that can emit ~29 phantom focus
events a second cannot make it thrash.

Nothing stays resident. Both halves are callbacks:

```toml
# ~/.aerospace.toml — windows that belong everywhere catch up on any
# workspace change, whatever caused it
exec-on-workspace-change = ['…/sling', 'follow']
```

```toml
# plugin/herdr-plugin.toml — herdr runs this when the focused tab changes
[[events]]
on = "tab.focused"
command = ["…/sling", "goto"]
```

Install the herdr half with `herdr plugin link ~/Dev/sling/plugin`.

`sling watch` still exists for somewhere the callbacks cannot be installed. It
subscribes rather than polls, and `--poll` is the last resort. A daemon is the
worse design though: one that quietly dies is indistinguishable from a broken
follow list, which is exactly how it failed the first time.

## Surviving a restart

AeroSpace does not remember which workspace a window belongs to. Restart it and
everything lands in whatever each monitor happens to be showing — which, with
forty-odd windows, is an evening's sorting lost.

`sling snapshot` writes the mapping to `~/.local/state/sling/layout.json`, and
`sling restore` puts it back. The watcher snapshots automatically on every
workspace change, so there is normally a current one without thinking about it.

Restore matches by window id first, then by application and title, so a window
whose application has restarted since the snapshot still finds its way home.
Windows that no longer exist are skipped.

```sh
sling restore --dry-run    # list what would move
```

## Windows that belong everywhere

Some windows — herdr, WhatsApp — are not part of any one task. Pick
`∞  all workspaces` and that window joins the follow list: it stays where it
is, and comes along to whichever task you sling something to next. Picking it
again takes it off.

AeroSpace has no sticky windows — a window is in exactly one workspace and
nothing can change that — so this is emulation. `sling watch` subscribes to
AeroSpace's own `focused-workspace-changed` event, so followers keep up with
*any* workspace change: a keybinding, a click, a herdr tab, or sling itself.

It is cheap because moving a window by id neither focuses it nor switches
workspace, so bringing the follow list raises no further event and costs no
workspace restore. On AeroSpace 0.12 the same two windows took 46 seconds; it
is now about 100ms.

Window ids do not survive an application restart, so the list goes stale when
you quit WhatsApp. `sling following` shows what is on it; re-tagging is one
pick.

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

`~/.config/sling/workspaces.toml`, seeded on first run. It holds the tasks that
are *not* herdr tabs, the group headings, and the order they appear in.

Names may only contain `[A-Za-z0-9._-]`. A `/` hangs AeroSpace, so herdr's
`t/forms` is spelled `t-forms` — typing the slash is fine, it gets translated.

## State

- `~/.local/state/sling/seen.json` — every workspace name ever seen, so a task
  survives its workspace emptying out
- `~/.local/state/sling/actions.jsonl` — one line per sling, including the ones
  that moved nothing

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
