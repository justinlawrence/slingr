# slingr in one page

A condensed read of [`SPEC.md`](SPEC.md) — enough to pick the work back up
without reading all of it. The spec carries the argument behind each decision;
[`FINDINGS.md`](FINDINGS.md) carries the AeroSpace and herdr behaviour those
decisions work around, and is the one to read before changing anything that
touches either.

## What it is

Work is task-shaped, not application-shaped. A task is a herdr tab plus the
handful of windows that belong with it — a browser window, a terminal, whatever
else. macOS cannot hold that grouping: Cmd+Tab switches applications, the window
list is unordered, and Mission Control stacks same-application windows on each
other. So each task gets one AeroSpace workspace, and slingr is how a window
gets into the right one and how you get there yourself.

Finding a *known* window is already solved, by AltTab. slingr is only for
keeping them grouped.

## The model

- A task is free-form. Most are herdr tabs; some (`house`, `mail`) are not.
- One workspace per task, one Brave window per task. Brave is the user, Chrome
  is for MCP agents.
- A worktree is an attribute of a task, not the other way round.
- Names mirror herdr tabs with `-` for `/`: `t/forms` → `t-forms`. A `/` wedges
  the AeroSpace server, so the translation is not cosmetic. The prefix is the
  project — `w` work, `s` side project, `me` personal — and the menu groups on
  it.
- A new window is a new task unless it is filed against one. Nothing is
  auto-filed: naming happens when you know what the task is.
- The second screen is not in the rotation. It watches a long-running build and
  is glanced at, not switched to.

The task list is merged on each open from four sources: AeroSpace (workspaces
holding windows, with counts), herdr (`herdr tab list`, about 7ms), `seen.json`,
and `workspaces.toml`. A tab renamed in herdr is slingable the next time the
panel opens, with nothing to edit.

## The four surfaces

One panel, four tabs; ⇥ cycles them, and three have their own key.

| key | command | the question |
|---|---|---|
| `ctrl-alt-cmd-s` | `slingr` | where does *this* window go? |
| — | ⇥ to **sling many** | which windows belong together, and where? |
| `ctrl-alt-cmd-i` | `slingr jump` | take me to a task |
| `ctrl-alt-cmd-b` | `slingr board` | show me everything, and let me sort it |

**sling once** names the window being slung at the top, with the icon and title
of its active tab — a browser title is seven fragments long, and repeating it on
every row tells you nothing. `HERE` opens with whichever is true: the workspace
it is in, or `Show on all workspaces`. Typing a name nothing answers to offers
to make it; searching for a task you have not made yet and making it are the
same gesture.

**sling many** drains a workspace that has collected dozens of windows: every
window, grouped by the workspace it is in, whole groups selectable at once, then
one destination for the batch. Rows are addressed by id, never by text, because
browser windows routinely share a title.

**board** is the whole machine as tiles, one per task, windows dragged between
them. It is the one mode answered more than once: drags accumulate, nothing
moves until you confirm, and escape puts everything back. It exists because
Ctrl-↑ groups by macOS Spaces, which have nothing to do with AeroSpace
workspaces. Empty tasks still get a tile — an empty task is usually the one you
are sorting *towards*.

Every mode's front door carries the tab strip; every step after it does not.
Switching mode halfway through answering "which windows" is not something anyone
means to do.

## Windows that belong everywhere

AeroSpace has no sticky windows, so this is emulated by moving windows by id —
no focus change, no workspace change, about 55ms each. Three kinds, because
"belongs everywhere" is sometimes about a window and sometimes about an
application:

- **A named window** — the herdr terminal, one Ghostty among several. Tagged in
  `follow.json`; the id dies with the application, and `slingr following
  --prune` forgets the dead ones.
- **A followed application** — WhatsApp. Every window of it, whichever are open.
- **Global by nature** — Finder, currently the only one, hard-coded in
  `picker::GLOBAL_BY_NATURE`. It opens and closes windows all day, so a window
  id is useless; and activating it from the Dock focuses whichever window it
  has, which drags you to that window's task.

Slinging one of its windows is how you say "not this one": that window is
**grounded** and stays put. Grounding is per window and expires with it, which
is the right lifetime — "leave this Finder window in me-tax" is about the window
in front of you, not every Finder window you will ever open.

Two rules about when they move, both of them bugs first:

- **On arrival, never on a sling.** A sling does not move you, so sending the
  followers after it stripped the workspace you were still sitting in — and if
  that window was the last one there, macOS handed focus somewhere at random.
  They catch up by themselves the moment you actually go somewhere.
- **Only within their own monitor.** A window that belongs everywhere belongs
  everywhere *on its own screen*, so clicking the terminal on the second display
  cannot drag your working windows across to it.

## Both directions

A task is a herdr tab *and* a workspace, so arriving at either brings the other.
Nothing stays resident — both halves are callbacks run by something already
running:

- AeroSpace's `exec-on-workspace-change` runs `slingr follow`
- a herdr plugin's `tab.focused` hook runs `slingr goto`

Arriving is done by focusing the window you were typing in, rather than asking
for the workspace and then correcting focus — one operation, with nothing to
race the callback AeroSpace fires. Only a window that *follows* you counts as
the one you were typing in: a follower is with you by design, anything else is a
coincidence.

A daemon was written first and removed. One that quietly dies is
indistinguishable from a broken follow list, which is how it failed. `slingr
watch` survives for anywhere the callbacks cannot be installed.

## The guards that stop the flashing

Read this before touching `goto` or `follow`. Both halves are idempotent, which
is not enough on its own — two idempotent halves still oscillate.

- **A lock each.** `only_one("goto")` and `only_one("follow")`. Without the
  second, `exec-on-workspace-change` spawned as many runs as AeroSpace fired
  changes, and two of them moved the same seven windows in a race.
- **Three states, not two.** `store::Settling` is `Ours`, `InFlight` or
  `Settled`. The old guard asked only "did we just go *here*", and AeroSpace
  reports the workspace it is *on* when the hook fires — which during a switch
  can still be the one being left. It failed open on exactly the wrong beat, and
  the bounce that followed was slingr's own.
- **One decision function.** `watch::catch(another_is_running, settling)` →
  `Busy`, `MidSwitch`, `Arrived` or `Chase`. Only `Chase` brings followers or
  moves herdr's tab. Each value is a bug that happened, and each is tested by
  name in `watch.rs`'s `chain_tests`.
- **One more look.** Two taps 400ms apart left the two halves on different tabs,
  because the lock turned the second away and nothing looked again — the
  *earlier* intent won. `watch::look_again` makes `goto` check once more before
  releasing the lock, so the last tap gets the last word.

Cost, measured in `watch.jsonl`: about 900ms per switch before this work, about
660ms after. The hooks fire once each; what remains is the window moves
themselves, roughly 55ms apiece. **The only lever left on flashing is how many
windows follow you** — which is what grounding is for.

## Surviving a restart

AeroSpace does not remember which workspace a window belongs to; restart it and
an evening's sorting is gone. `slingr restore` asks the action log *before* the
snapshot, because the log records intent and the snapshot records happenstance —
and after a reboot every window id is new, so the automatic snapshot otherwise
writes down the pile they landed in and agrees with it. Matching is by
application and title. Followers are skipped: they are wherever you are on
purpose. Measured on a real reboot: 43 windows in one pile, 29 put back.

## Where the code is

| | |
|---|---|
| `src/picker.rs` | the rules, no I/O: parsing, naming, grouping, menu building |
| `src/app.rs` | the flows, against the `WindowManager` and `Prompt` traits |
| `src/watch.rs` | what a workspace change asks of the hook (`Catch`, `look_again`) |
| `src/store.rs` | follow list, pins, echo (`Settling`), snapshots, action log |
| `src/aerospace.rs`, `src/herdr.rs`, `src/dialog.rs`, `src/panel.rs` | the impure edges, thin on purpose |
| `src/main.rs` | the CLI, and the two hook entry points `goto_now` / `follow_now` |
| `panel/main.swift` | presentation only; rows in as JSON, chosen ids out |
| `tests/flow.rs` | whole flows against fakes — assert *ordering* here |

The field spelling between Rust and Swift is pinned by a test in `src/panel.rs`:
a renamed field is not a compile error in Swift, it is a value that silently
never arrives.

State lives under `~/.local/state/slingr/` — `follow.json`, `pins.json`,
`layout.json`, `actions.jsonl`, `watch.jsonl`, `seen.json`. `slingr paths`
prints them, and says whether the panel binary is where it is expected; a silent
fallback to the AppleScript dialog usually means it is not.

## Build and test

```sh
./build.sh      # cargo build --release AND swiftc the panel
./install.sh    # the above, plus the symlink and the herdr plugin
cargo test      # 130 tests, none of which need AeroSpace or a screen
```

`cargo build` alone does not build the panel.

## Where it stands

Last commit `4e099e2` (24 Sep 2026), **one ahead of `origin/main`** — the
flashing fix is local and unpushed. 130 tests green (69 lib, 61 flow).

Verified live: four tab switches and a double-tap, every `follow` reporting
`Arrived` with nothing moved, both guards firing when they should, and both
halves agreeing at the end.

Open, in rough order of interest:

- The board's drag-and-drop is covered by fakes but has never been exercised by
  eye. Confirming a tidy-up of a dozen windows is the obvious next thing to do.
- Whether grounding a Finder window reads as natural in use, and whether
  `GLOBAL_BY_NATURE` should become a list in `workspaces.toml` that a person can
  add to. What it must not become is a rule that guesses.
- herdr's agent list stopped putting agents needing attention at the top. slingr
  never calls anything that reorders agents; the old bounce did produce about 15
  spurious `focus_tab` calls, and whether herdr's priority sort weights recency
  was **not** established. Worth re-observing now the spurious focusing has
  stopped.

## Not built yet

- **Live state** — the list is a snapshot taken before the panel opens; it could
  subscribe to `focused-workspace-changed` and `window-detected`.
- **Feedback instead of vanishing** — pinning and `Show on all workspaces` close
  the panel; a panel could show the new state, which a dialog never could.
- **Window previews** — `CGWindowListCreateImage`, most useful on the board.
- **Drag to reorder pins** — pinned tasks are still ordered by when they were
  pinned.
- **An inline rename field**, so `＋ new workspace` stops bouncing out to an
  AppleScript text dialog — the last of the old UI still in the flow.
- **Routing rules** — Chrome to `agents`, a work profile to `ac-*`. Written
  once, caused a window-flashing loop, removed. Re-add one at a time, specific
  first, with no fall-through.
