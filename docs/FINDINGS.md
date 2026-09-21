# Findings

Things about AeroSpace and macOS that are not in the documentation, each found
the expensive way. Every one of these shapes the design, so changing the design
without reading this will reintroduce a bug we have already paid for.

Verified against AeroSpace **0.21.3-Beta** on macOS 25.6.0 (Darwin),
September 2026.

> **Read this first.** Most of the hard-won findings below were about AeroSpace
> **0.12.0-Beta**, which this machine was pinned to via the `aerospace@0.12.0`
> cask. Upgrading to 0.21.3 removed the causes of nearly all of them. Sections
> marked **[0.12 only]** are kept because they explain why the code is shaped
> as it is — not because they still apply.
>
> **The lesson worth keeping: check the installed version against the latest
> release before designing around a limitation.** An evening went into
> engineering around behaviour that had been fixed nine releases earlier.

## What the upgrade changed, measured

| | 0.12.0 | 0.21.3 |
|---|---|---|
| Switch to a workspace of 41 windows | **120.8s** | **0.113s** |
| Move one window to another workspace | ~2-23s | **0.07s** |
| Bring 2 follower windows along | **46.3s** | **0.11s** |
| Move 42 windows in bulk | wedged the server | **2.9s total** |

Three capabilities account for it:

- **`move-node-to-workspace --window-id <id> <workspace>`** moves a window
  without focusing it. Focusing was the whole problem: it dragged the view to
  that window's workspace, and showing a workspace means restoring every window
  in it. Naming the window costs nothing. It also moves windows that cannot
  take focus at all, such as minimised ones.
- **`--focus-follows-window`** makes the view follow only when asked.
- **`aerospace subscribe`** pushes JSON-line events over a socket:
  `focus-changed`, `focused-workspace-changed`, `focused-monitor-changed`,
  `window-detected`, `mode-changed`, `binding-triggered`. No polling, and the
  current state is sent on connect.

Also available and not yet adopted here:

- **`persistent-workspaces`** (needs `config-version = 2`) keeps named
  workspaces alive while empty — most of what `known` and `seen.json` are for.
  Note the migration: under version 2 the fallback becomes an empty list rather
  than being inferred from keybindings, so the list must be written out.
- Callbacks `on-focus-changed`, `on-focused-monitor-changed`, `on-mode-changed`,
  and `run-callback` to fire them on demand.
- `aerospace eval`, shell operators (`&&`, `||`, `;`, `|`), `summon-workspace`,
  `swap`, `focus --dfs-index`.

Still absent: **sticky windows**. Issue #2 is open, and `layout sticky` is not
a thing. Emulating it by moving windows remains the only route — which is now
cheap enough not to matter.

## `move-node-to-workspace` cannot name a window **[0.12 only]**

```
USAGE: move-node-to-workspace [-h|--help] <workspace-name>
ARGUMENTS:
  <workspace-name>   Workspace name to move focused window to
```

It acts on whatever is focused **at the instant it runs**. There is no
`--window-id`. Any flow that shows a dialog between reading a window and moving
it must re-focus that window first, because showing a dialog moves focus.

This is the bug that put a Ghostty terminal into `ac-app` when a Brave window
was chosen: sling read Brave's title for the dialog, System Events took focus
for the dialog, focus fell back to Ghostty when it closed, and Ghostty is what
moved.

Pinned down by `refuses_to_move_when_the_window_cannot_be_refocused` in
`tests/flow.rs`.

## `focus --window-id` returns 0 when it fails **[0.12; moves no longer use focus]**

Exit status is no guide at all. Observed directly:

```
asked 7716  -> got 7716   (exit 0)
asked 19369 -> got 19369  (exit 0)
asked 11513 -> got 13031  (exit 0)     # a minimised window
```

A minimised window cannot be raised, so focus lands somewhere else entirely and
AeroSpace still reports success. The only reliable check is to read the focused
window id back and compare. `AeroSpace::focus` does exactly that and returns a
bool, which is why the trait exposes it that way.

Consequence for the user: a minimised window cannot be slung. Un-minimise it
first. sling declines rather than moving the wrong window.

## A workspace exists only while it holds a window — or is on screen **[see persistent-workspaces]**

`aerospace list-workspaces --all` reports a workspace if it holds windows **or**
is the active workspace on some monitor. Every monitor always has one active
workspace, so there are always at least as many as there are monitors:

```
$ aerospace list-monitors
1 | AW3423DWF
2 | Built-in Retina Display
$ aerospace list-workspaces --all
1            # empty; it is simply what monitor 2 is showing
infra
```

So `list-workspaces --all` is the wrong question to ask when the menu wants to
tick what is occupied — `1` would get a tick while holding nothing. sling asks
`list-windows --all --format '%{workspace}'` instead, which costs the same one
query and answers the question actually being asked.

Otherwise, empty workspaces are absent. So:

- A brand-new task is unreachable from a list built only from live workspaces.
  Hence `known` in `workspaces.toml`, and the name cache in
  `~/.local/state/slingr/seen.json`.
- A workspace you invent and later empty out disappears from the menu unless
  something remembers it.
- `✓` in the menu means "already holds windows", not "exists".

## Assigning workspaces to monitors is what makes empty ones exist

`workspace-to-monitor-force-assignment` materialises every workspace it names.
With 21 assignments and 3 non-empty workspaces, cycling `workspace next` had 18
blank stops, which reads as flashing — especially with key auto-repeat on a
held chord.

Removing the assignments entirely fixed it: only real tasks are now cycled
through.

The same setting is an absolute constraint elsewhere. `move-workspace-to-monitor`
is **refused** while a force-assignment exists, with
`workspace-to-monitor-force-assignment doesn't allow it`. Deleting the pin and
reloading must happen in one chained command — a backgrounded reload races and
loses.

## A `/` in a workspace name wedges the server

`t/test` hangs `reload-config` on a modal dialog that is never shown; `ttest`
is fine. Since tasks are named after herdr tabs (`t/forms`), the slash arrives
by reflex, so `sanitise_workspace` translates it to `-` rather than rejecting
the name.

## A dialog from `exec-and-forget` needs System Events to own it

AeroSpace's `exec-and-forget` has no GUI context. A bare `osascript`
`choose from list` compiles and runs, and nothing ever appears on screen. The
dialog has to be wrapped in `tell application "System Events" … activate` — an
application already running that can come to the front.

This needs an Automation grant (AeroSpace → System Events), which macOS prompts
for once. It is a narrow grant and worth allowing; it is what draws the dialog.

## `osascript` strips whitespace from what it returns

A choice comes back trimmed. The menu indents unoccupied workspaces by four
spaces to align them under the ticked ones, and that indent does not survive
the round trip:

```
shown:    "    t-mail"
returned: "t-mail"
```

Looking the returned string up in a map keyed on the displayed label therefore
found nothing for exactly the entries that matter — every empty workspace, which
is every workspace you would start a new task in. Occupied (`✓  t-mail`) and
current (`●  …`) entries worked, so the failure looked arbitrary.

It failed silently: no workspace matched, so sling concluded a heading had been
chosen and moved nothing. `Menu::workspace_for` now looks up on `trim()`, and
the map is keyed the same way, so whitespace cannot matter again. The fake
prompt in `tests/flow.rs` trims too, or the tests would pass where the real
thing fails.

## System Events keeps focus after the dialog closes

Press the sling key twice in a row and the second press reads **System Events**
as the focused window — its dialog is gone from the screen but it is still the
focused application, and AeroSpace reports it as a window in the current
workspace:

```
{"outcome":"cancelled","window":{"app":"System Events","id":"19668"}}
```

Left alone, that would file a dialog into a task while the real window stayed
put. sling now refuses any window belonging to System Events and logs
`own_dialog`. Click the window you actually want first — which is the natural
flow anyway, since sling acts on what is focused.

## Window titles break AppleScript literals

Titles carry quotes and the occasional backslash. Interpolated unescaped, they
end the string literal early, the script fails to compile, and — because the
failure is silent from the user's side — the dialog simply never appears. See
`dialog::applescript_string`.

## Showing a workspace costs ~3 seconds per window **[0.12 only]**

This is the single most important number here. Measured 20 Sep 2026 with 41
windows in one workspace:

```
infra -> t-mail   hide 41, show  1     1.8s
t-mail -> infra   hide  1, show 41   120.8s      # completed, just slowly
```

Hiding is cheap. **Showing is not** — roughly 3 seconds per window restored, and
AeroSpace serialises commands, so every other query and keypress queues behind
it. A switch back into a crowded workspace therefore looks like the window
manager has locked up and is flashing windows at random. It has not; it is
restoring them one at a time and will finish.

That appearance is what earlier notes called "the server wedging". At least some
of those sightings were this: a long restore in flight, with an innocent
read-only query stuck behind it. Killing the stuck client does not help — the
server keeps working, and the operation completes on its own.

The practical rule: **keep workspaces small.** A handful of windows each is the
design; forty in one is a transitional state to be drained, not a load to be
tuned for. Draining does not require switching workspace — moving a window out
of the visible workspace hides exactly one window and is fast — so sort with
sling while staying put, and do not leave a crowded workspace until it is not
crowded.

Because of all this, every call in `aerospace.rs` is bounded by a 5s timeout.
That is deliberately far shorter than a big restore: sling should fall back to
remembered names and let you carry on, not sit for two minutes. The log line
carries `aerospace_unavailable` when it happens.

sling does **not** kill its own timed-out clients. It would not stop the work,
and a stuck client exits once the server answers. `pkill -f 'bin/aerospace'`
clears any that accumulate.

Separately, and still unexplained: three earlier lockups were attributed to
scripts firing `focus --window-id` + `move-node-to-workspace` in a loop. Given
the above, those may have been slowness rather than deadlock — but that has not
been re-tested, so bulk-scripting window assignment stays off the table.

## Unexplained: the first run of a freshly built binary times out

`slingr probe` reports `no answer within 5s` for both queries when it is the
first thing run after `cargo build` or `cargo test` actually recompiled. Seen
five times. Every time:

- no stuck `aerospace` clients
- AeroSpace at flat CPU, demonstrably idle
- an immediate retry works

So it is not the server. Two guesses were made and both were wrong: that it was
something about `cargo test` specifically (it happens after `cargo build` too),
and that AeroSpace was busy (it is not). The cause is still unknown.

Not worth a retry in the code: it only happens to a binary built moments
earlier, and the key binding runs one built long before. Recorded so the next
person does not spend the evening on it — retry and carry on.

## herdr's bundled schema under-reports what can be subscribed to

`herdr api schema --json` describes `SubscriptionEventKind` as exactly three
values:

```
["pane.output_matched", "pane.agent_status_changed", "pane.scroll_changed"]
```

That is wrong, or at least incomplete. The server accepts far more. The
documentation for the same version (v0.9.1) lists workspace, tab, pane, layout
and worktree events, and the running server confirms it:

```
$ echo '{"id":"s","method":"events.subscribe",
         "params":{"subscriptions":[{"type":"tab.focused"}]}}' | socket
{"id":"s","result":{"type":"subscription_started"}}
{"data":{"tab_id":"w7:tA","type":"tab_focused","workspace_id":"w7"},"event":"tab_focused"}
```

Do not trust the bundled schema's enum for what is subscribable. Ask the
server — it answers `subscription_started` or an error, which settles it in one
round trip.

Two spellings are involved and they differ: you subscribe to `tab.focused`
and receive `"event":"tab_focused"`.

## Subscribing to herdr

Newline-delimited JSON over a Unix socket at `~/.config/herdr/herdr.sock`
(overridden by `HERDR_SOCKET_PATH`, or `HERDR_SESSION` naming
`~/.config/herdr/sessions/<name>/herdr.sock`). No handshake — send the request
straight away. The first line acknowledges; every line after is a pushed event.
Events from before the subscription was accepted are not replayed.

A `tab_focused` event carries `tab_id` and `workspace_id` but **not** the
label. Since tabs can be renamed, a cached id-to-label mapping goes stale;
`herdr tab list` costs about 7ms, so resolving the label per event is both
cheaper to write and always correct.

## herdr can emit phantom focus events in bursts

Reported against herdr: on an idle session with several agent panes producing
output, `events.subscribe` can deliver roughly nine
`pane`/`tab`/`workspace.focused` triples per second — about 29 events/s — with
the "focused" tab appearing to cycle through every tab that has output, while
nobody is navigating anywhere.

This session does not show it (12 seconds subscribed to all three focus events
with no interaction produced no events at all), but the failure mode is severe
enough to design against: one workspace switch per event is one full window
restore per event.

`slingr watch` is built so that a storm cannot hurt it:

- Events are read on a separate thread and coalesced. A burst collapses to a
  single action, and the watcher cannot fall behind.
- The event's `tab_id` is never trusted as the thing to act on. The live
  focused tab is read afterwards, so a discarded or phantom event says nothing
  new, and `decide` answers "already there".

Worth re-checking after a herdr upgrade, with a subscription and a stopwatch.

## Both halves can be callbacks — no daemon needed

Verified on this machine rather than inferred:

```
exec-on-workspace-change = ['/bin/bash', '-c', 'probe.sh']
  → 07:41:10 ws=ac-app prev=t-mail
  → 07:41:11 ws=t-mail prev=ac-app
```

AeroSpace runs the command itself and hands it
`AEROSPACE_FOCUSED_WORKSPACE` / `AEROSPACE_PREV_WORKSPACE`.

herdr will too, through a plugin. The manifest is `herdr-plugin.toml` and
`herdr plugin link <dir>` installs it. The required fields are not all
documented — `id`, `name`, `version` and `min_herdr_version` are each rejected
by name if missing, which is at least a legible way to find out:

```toml
[[events]]
on = "tab.focused"
command = ["…/sling", "goto"]
```

`tab.focused` is accepted even though the plugin docs only ever show
`worktree.created`. Confirmed firing.

So `slingr watch` is no longer the way this runs. A resident watcher was worse
on the merits: it died during a debugging session and stayed dead, and a
watcher that has quietly stopped looks exactly like a broken follow list.

The one thing a daemon did better is coalescing. A hook spawns a process per
event, and herdr has been reported to emit focus events in bursts, so `goto`
takes a lock and a second one exits immediately rather than piling up.

## AeroSpace has no sticky windows## herdr can emit phantom focus events in bursts

Reported against herdr: on an idle session with several agent panes producing
output, `events.subscribe` can deliver roughly nine
`pane`/`tab`/`workspace.focused` triples per second — about 29 events/s — with
the "focused" tab appearing to cycle through every tab that has output, while
nobody is navigating anywhere.

This session does not show it (12 seconds subscribed to all three focus events
with no interaction produced no events at all), but the failure mode is severe
enough to design against: one workspace switch per event is one full window
restore per event.

`slingr watch` is built so that a storm cannot hurt it:

- Events are read on a separate thread and coalesced. A burst collapses to a
  single action, and the watcher cannot fall behind.
- The event's `tab_id` is never trusted as the thing to act on. The live
  focused tab is read afterwards, so a discarded or phantom event says nothing
  new, and `decide` answers "already there".

Worth re-checking after a herdr upgrade, with a subscription and a stopwatch.

## Both halves can be callbacks — no daemon needed

Verified on this machine rather than inferred:

```
exec-on-workspace-change = ['/bin/bash', '-c', 'probe.sh']
  → 07:41:10 ws=ac-app prev=t-mail
  → 07:41:11 ws=t-mail prev=ac-app
```

AeroSpace runs the command itself and hands it
`AEROSPACE_FOCUSED_WORKSPACE` / `AEROSPACE_PREV_WORKSPACE`.

herdr will too, through a plugin. The manifest is `herdr-plugin.toml` and
`herdr plugin link <dir>` installs it. The required fields are not all
documented — `id`, `name`, `version` and `min_herdr_version` are each rejected
by name if missing, which is at least a legible way to find out:

```toml
[[events]]
on = "tab.focused"
command = ["…/sling", "goto"]
```

`tab.focused` is accepted even though the plugin docs only ever show
`worktree.created`. Confirmed firing.

So `slingr watch` is no longer the way this runs. A resident watcher was worse
on the merits: it died during a debugging session and stayed dead, and a
watcher that has quietly stopped looks exactly like a broken follow list.

The one thing a daemon did better is coalescing. A hook spawns a process per
event, and herdr has been reported to emit focus events in bursts, so `goto`
takes a lock and a second one exits immediately rather than piling up.

## AeroSpace has no sticky windows

Confirmed at line 185 of its own `default-config.toml`:

```
#s = ['layout sticky tiling', 'mode main'] # sticky is not yet supported
```

So one window cannot appear in two tasks. A window belongs to exactly one.

## AeroSpace does not manage panels

Opening Raycast changes nothing about what AeroSpace can see:

```
windows known to AeroSpace: before=43 after=43
Raycast panel: not listed
```

A floating `NSPanel` is not a managed window. It therefore belongs to no
workspace, is never hidden, never moved, and costs no restore — it simply
appears over whatever is in front.

This decides the shape of any richer picker. A panel is free of the window
management this whole project is about; a local web page is a browser window,
which lives in a workspace, gets hidden on every task switch, and would have to
be added to the follow list like the terminal. The look is independent of the
choice: a panel can be styled any way at all.

It would also remove the `OwnDialog` guard. The current `choose from list`
dialog is a System Events window that AeroSpace *does* manage — it has been
seen as `20086|infra|System Events|Sling window` — which is why pressing the
sling key twice running used to offer to sling the dialog itself.

## AeroSpace workspaces are not macOS Spaces

They are AeroSpace's own concept. Mission Control, Stage Manager and the
desktop switcher know nothing about them.
