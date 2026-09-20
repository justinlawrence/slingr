# What sling is for

## The problem

Work is task-shaped, not application-shaped. A task is a herdr tab plus the
handful of windows that belong with it: a Brave window, sometimes a Chrome
window where an MCP agent is driving, a terminal, whatever else.

macOS offers no way to hold that grouping. Cmd+Tab switches applications, the
window list is unordered, and Mission Control stacks same-application windows
on top of each other. With 40-odd windows open the cost of finding the right
one is high enough to break the task.

Two pieces answer this, and they are separate:

- **Finding a known window** — solved, by AltTab. `⌃⌥⇧Tab`, type a few
  letters, Enter. It searches across both browsers and all profiles, which is
  strictly better than Chrome's own tab search (`Cmd+Shift+A`), which is
  per-profile.
- **Keeping windows grouped by task** — AeroSpace workspaces, one per task.
  This is what sling is for.

## The model

- A **task** is free-form. Most are herdr tabs; some are not (`house`, `mail`).
- A **worktree is an attribute of a task**, not the other way round.
- One **workspace per task**, one **Brave window per task**.
- Brave is the user; Chrome is for MCP agents.
- A window belongs to exactly one task. AeroSpace has no sticky windows — the
  string does not appear anywhere in its binary — so this is a constraint
  rather than a decision.
- Except for the few that belong to none of them: herdr, WhatsApp. Those are
  tagged to follow, and get dragged to whichever task you sling to next.
- A new window is a new task unless it is filed against an existing one.
- The XDR is not part of the rotation. It watches `tytoctl up` and nothing else.

Workspace names mirror herdr tabs with `-` where herdr uses `/`: `t/forms`
becomes `t-forms`. The prefix is the project — `t` tyto, `ac` art corner, `me`
personal — and the menu groups on it.

Since a task is usually a herdr tab, the tabs are read from herdr rather than
kept in step by hand: `herdr tab list` returns JSON, the labels are sanitised,
and they join the list. A tab renamed in herdr is slingable the next time the
dialog opens. Tabs herdr has not named — a bare number — describe no task and
are left out.

That leaves `workspaces.toml` holding only what herdr cannot supply: free-form
tasks like `house` and `mail`, and tabs planned but not yet open.

## What sling does

One key (`ctrl-alt-cmd-s`, bound to layer 5 on the Voyager) opens a panel. It
reads the focused window, offers the task list, and moves the window to
whichever task is chosen.

```
Slinger ◈
[icon] Brave Browser   Inbox (578) — Arty Corner Mail
[ sling once ] sling many                    ＋ new workspace
❯ type to filter tasks
HERE
  ✓  ac-mail                              3 windows
  ✓  Show on all workspaces
TYTO
  ★  t-pair                               1 window
     t-forms                              empty
```

The window being slung is named once, at the top, with its application's icon —
a browser title is seven fragments long and repeating it on every row tells you
nothing. `HERE` opens with whichever answer is true: the workspace the window
is in, or `Show on all workspaces` if it is set. The two are mutually
exclusive, because a window shown everywhere is not in any one place.

Tasks are grouped by project. Pinned tasks rise within their folder rather than
forming a group of their own — pinning is about the tab, not the project it
belongs to. `empty` distinguishes a known task with nothing in it yet from one
that already holds windows, because "file this with its mates" and "start
something new" are different intentions.

`＋ new workspace` sits on the tab line rather than in the list: it is an
action, not a destination. Naming happens when you know what the task is, not
when the window opens, which is why new windows are never auto-filed.

## Draining

A workspace that has collected dozens of windows is a transitional state, not a
load to design for. `sling many` is how you get out of it: select a batch, send
it to one task, repeat.

Two modes, two tabs, one key. The fast path stays a single question about the
focused window; the batch path is one tab away. Both tabs are always shown and
only which is active changes — a row that means "switch" was tried first and
was never honest about being a mode.

The window list covers every workspace, grouped by the one each window is in,
and a whole group can be taken at once: the heading carries none/some/all. So
the folders are in the answer rather than being a question of their own. A
drill-down was tried and removed — it made taking most of a folder, or windows
from two folders, impossible.

Rows are addressed by id, never by their text. Browser windows routinely share
a title, and where one does collide the window id is shown beside it.

Every step after a mode's front door carries no tabs. Switching mode halfway
through answering "which windows" is not something anyone means to do.

## Two screens

The second display is not part of the task rotation: it watches `tytoctl` and
is glanced at, not switched between. Nothing declares this — it falls out of
one rule.

A window that belongs everywhere belongs everywhere **on its own screen**.
Followers move only when the workspace change is on the monitor they are
already on, so clicking the terminal on the second display cannot drag the
windows you were working beside on the first one across to it.

That also means naming the second screen's workspace would be harmless but
pointless: it would join the sling list, the cycle and the tab list, inviting
you to sling things there, when the whole point is that it is an instrument
panel rather than a task.

## Why it is built this way

The flow is written against two traits, `WindowManager` and `Prompt`, so it can
be driven without a window manager or a screen. That is not ceremony — it is
what lets a bug about *ordering* be pinned down, and what made replacing the
entire front end a question of adding one implementation.

`picker.rs` holds the rules with no I/O in them — parsing, naming, grouping,
menu construction — and is tested directly.

The panel is a separate process that reads rows on stdin and prints chosen ids.
All the thinking stays in Rust, under test; the Swift is presentation only. It
is an `NSPanel` because AeroSpace does not manage panels: it belongs to no
workspace, is never hidden or moved, and cannot be slung by accident. It is
also non-activating, so AeroSpace still reports the window being slung as
focused while the panel is up.

Its colours come from the terminal's, not from a palette of its own. Omarchy
plugins are theme-aware by convention, and there is no system theme to read on
macOS — but there is the terminal this all revolves around.

The AppleScript dialog remains as an automatic fallback when the panel is not
built. It can only take flat strings, which is why rows are rendered as well as
structured.

## The action log

Every sling appends a line to `~/.local/state/sling/actions.jsonl`, including
the ones that moved nothing:

```json
{"at":"2026-09-20T04:12:00Z","outcome":"moved","from":"infra",
 "window":{"id":"11513","app":"Brave Browser","title":"Arty Corner Mail"},
 "to":"ac-app","created":false}
```

The point is to find out whether the taxonomy is right, which is not knowable
in advance. `sling stats` summarises it. The questions it should answer:

- Which workspaces actually get used, and which were invented and never
  revisited?
- How often is a window re-slung? That means it was filed wrongly, or the task
  boundary is in the wrong place.
- How often is `cancelled` chosen? That means the list did not offer what was
  wanted.
- How often is a task created rather than reused? A high rate early is
  expected; a high rate later means the taxonomy is too fine.

## Windows that belong everywhere

Some windows are not part of any one task: herdr, WhatsApp. `Show on all
workspaces` is a standing instruction rather than a destination, so it reads as
an ordinary row that carries a tick instead of an action that announces itself.
The window stays where it is and joins a follow list.

AeroSpace has no sticky windows — the string does not appear anywhere in its
binary, and issue #2 has been open since the beginning — so this is emulation.
It is cheap because a window is moved by naming it: no focus change, no
workspace change, no restore. Bringing two followers takes about 100ms, and
raises no event of its own, so the hook that triggers it cannot feed itself.

On AeroSpace 0.12 the same two windows took 46 seconds and the design had to
avoid doing it at all. That constraint is gone, and with it the rule that
followers could only be fetched from the workspace on screen.

What remains is the screen rule, which is about intent rather than cost: a
follower moves only within the monitor it is on. See *Two screens*.

Window ids do not survive the application restarting. Accepted: `sling
following` shows the list and re-tagging is one pick.

## The other direction

Slinging files a window under a task. The reverse closes the loop: change herdr
tab, and AeroSpace follows to the matching workspace.

Nothing stays resident. Both halves are callbacks, run by something already
running:

- AeroSpace's `exec-on-workspace-change` runs `sling follow`, so windows that
  belong everywhere catch up on any workspace change, whatever caused it.
- A herdr plugin's `tab.focused` hook runs `sling goto`.

A resident watcher was written first and removed. It was worse on the merits:
it died during a debugging session and stayed dead, and a watcher that has
quietly stopped is indistinguishable from a broken follow list. `sling watch`
survives for anywhere the callbacks cannot be installed.

The one thing a daemon did better is coalescing, so `goto` takes a lock — a
hook spawns a process per event, and herdr has been reported to emit focus
events in bursts.

The rule that matters is when *not* to switch: only into a workspace that
already holds windows. Switching to an empty one blanks the screen. That is
deliberately conservative and resolves itself, since a task starts pulling as
soon as something has been slung into it.

## Surviving a restart

AeroSpace does not remember which workspace a window belongs to. Restart it and
everything lands in whatever each monitor happens to be showing — an evening's
sorting lost, which is exactly what happened during the upgrade.

`sling snapshot` writes the mapping and `sling restore` replays it, matching by
window id first and then by application and title, so a window whose
application has restarted still finds its way home. The snapshot is taken
automatically on every workspace change.

## Configuration that AeroSpace now carries itself

`config-version = 2` unlocks `persistent-workspaces`, which keeps named
workspaces alive while empty. That is most of what `known` and `seen.json` were
for, and it is generated from all three task sources at once.

It comes with a catch worth remembering: `workspace next|prev` walks *all*
workspaces including empty ones, so making twenty-nine of them persistent would
put twenty-odd blank stops into the cycle — exactly the flashing this project
started with. The bindings therefore cycle the non-empty ones only:

```toml
ctrl-alt-cmd-n = 'list-workspaces --monitor focused --empty no | workspace --stdin next'
```

## Not built yet

- **Live state.** The list is a snapshot taken before the panel opens. It could
  subscribe to AeroSpace's `focused-workspace-changed` and `window-detected`
  and update while open.
- **Feedback instead of vanishing.** Pinning and `Show on all workspaces` close
  the panel; a panel can show the new state instead, which a dialog never
  could.
- **Window previews.** `CGWindowListCreateImage` would let you recognise a
  window rather than parse its title.
- **Drag** — to reorder pins, or a window onto a task. The original metaphor,
  finally literal.
- **An inline rename field**, so `＋ new workspace` stops bouncing out to an
  AppleScript text dialog — the last piece of the old UI still in the flow.
- **Routing rules** — Chrome to `agents`, the `justin@artycorner.uk` profile to
  `ac-*`. Written once, caused a window-flashing loop, removed. Re-add one at a
  time, specific first, with no fall-through.
- **`persistent-workspaces` could replace `known` and `seen.json`** now that
  AeroSpace keeps empty workspaces alive itself.
