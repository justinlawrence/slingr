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

One key (`ctrl-alt-cmd-s`, bound to layer 5 on the Voyager). It reads the
focused window, offers the task list grouped by project, and moves the window
to whichever is chosen.

The list distinguishes three states, because "file this with its mates" and
"start a new task" are different intentions:

```
＋  new workspace…
──────  tyto  ──────
✓  t-forms                        holds windows already
   t-pair                         a known task, nothing in it yet
──────  art corner  ──────
●  ac-app  (this window is here)  where this window is now
```

`＋ new workspace…` prompts for a name, translates it to something AeroSpace
accepts, and moves the window there — AeroSpace creates the workspace on
demand. Naming happens when you know what the task is, not when the window
opens, which is why new windows are not auto-filed.

## Draining

A workspace that has collected dozens of windows is a transitional state, not a
load to design for: restoring it costs about three seconds per window and
blocks everything else meanwhile. Draining is how you get out of it — select a
batch, send it to one task, repeat.

It is the same key and the same dialog as a single sling, with a mode row on
the first line. Two modes rather than two commands: the fast path stays one
dialog on the focused window, and the batch path is one pick away. A separate
invocation was tried first and rejected — the decision to move one window or
several belongs inside the dialog, not before it.

The window list covers every workspace rather than only the focused one, so a
task can be gathered from anywhere. Rows are addressed by number, not by
their text: browser windows routinely share a title, and a chosen line comes
back from the dialog trimmed, so matching on the label is both ambiguous and
fragile. Where a title does collide, the window id is shown alongside it.

A window that will not take focus — minimised, or closed since the list was
drawn — is skipped and reported, never silently left behind and never allowed
to strand the rest of the batch.

## Why it is built this way

The flow is written against two traits, `WindowManager` and `Prompt`, so it
can be driven without AeroSpace or a screen. That is not ceremony: the bug
that moved the wrong window is a property of *ordering* — focus must happen
before the move — and ordering is only observable if the calls can be
recorded. See `tests/flow.rs`.

`picker.rs` holds the rules with no I/O in them (parsing, naming, grouping,
menu construction) and is tested directly.

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

`∞  all workspaces` is not a destination but a standing instruction. The window
stays where it is and joins a follow list; the next time anything is slung, the
list comes too, and focus is handed back to the window you were moving.

This is emulated stickiness and the cheapest form of it — poor man's
persistence. It follows on a sling, not on a workspace switch, because
following every switch means moving windows on every switch, and moving a
window *into* the visible workspace is the expensive direction. The hook for
that exists (`exec-on-workspace-change` with `AEROSPACE_FOCUSED_WORKSPACE`, and
`sling follow` is written for it), but it is re-entrant by construction —
moving a window changes focus, which changes workspace, which fires the hook —
so it needs a guard and a measurement first, not just a config line.

Window ids are not stable across an application restart. Accepted: the list is
cheap to rebuild, `sling following` shows it, and re-tagging is one pick. When
the tooling improves this becomes a tidy-up rather than a design problem.

## The other direction

Slinging files a window under a task. `sling watch` closes the loop: change
herdr tab, and AeroSpace follows to the matching workspace.

herdr pushes a `tab.focused` event over its session socket, so the watcher
subscribes rather than polls. (The bundled API schema claims only three pane
events are subscribable; that is wrong — see `docs/FINDINGS.md`.) A polling
fallback is kept for when the socket is not available.

The rule that matters is when *not* to switch. AeroSpace shows a workspace by
restoring its windows, so switching to an empty one blanks the screen and the
way back costs a full restore. The watcher therefore only follows a tab whose
task already holds windows. That is deliberately conservative and it resolves
itself: a task starts pulling as soon as something has been slung into it.

Whether this is the intuitive rule is not settled. It is the safe one, which
is the right place to start — the failure it prevents is loud and slow, and
the failure it causes is nothing happening.

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

- **Pinned destinations.** The dialog lists tasks by project prefix, which is
  stable but not ordered by use. The action log already records where windows
  actually go, so a pinned section at the top is a small change to
  `build_menu` — no new UI needed. A richer picker (pinning by drag, a kanban
  layout) needs a real panel, since `choose from list` is a flat list.
- **Routing rules** — Chrome to `agents`, the `justin@artycorner.uk` profile to
  `ac-*`. Written once, caused a window-flashing loop, removed. Re-add one at a
  time, specific first, with no fall-through.
- **A columned picker.** `choose from list` is a fixed single-column AppleScript
  control, so a kanban layout needs a SwiftUI panel, a Raycast extension or a
  local web page. Deferred until the grouped list has been lived with.
- **Claude artifact routing.** Artifacts open into whatever window is in front,
  losing their provenance. Intended mechanism: a per-herdr-pane `BROWSER`
  variable pointing at a wrapper that opens the tab in that task's window.
  Needs confirming that Claude Code honours `BROWSER`.
