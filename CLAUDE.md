# CLAUDE.md

slingr: a window picker built around tasks rather than applications. Named to
match herdr, which it pairs with. A task is usually a herdr tab, and the
windows that belong with it live in one AeroSpace workspace or one macOS
desktop. There are two backends behind the `WindowManager` trait, chosen by
`backend` in `~/.config/slingr/workspaces.toml`. macOS Spaces is the one in
daily use; AeroSpace is still built and tested, and `./spaces.sh off` goes
back to it.

**Read `docs/SUMMARY.md` for the one-page version** — the model, the four
surfaces, the guards that stop the flashing, and where it stands.

**Read `docs/SPEC.md` first** — what this is for and why it is shaped as it is.
**Read `docs/FINDINGS.md` before changing anything that touches AeroSpace or
herdr, and `docs/SPACES.md` before anything that touches macOS Spaces.** They
record behaviour that is in no documentation, found by breaking things. Most
of it cost an evening. The single biggest lesson is at the top of FINDINGS.md:
check the installed version against the latest release before designing
around a limitation.

## Names

"sling" stays the verb — you sling a window, a window was slung. Only the tool
is `slingr`.

## Build

```sh
./build.sh      # cargo build --release AND swiftc the panel, Spaces helper and agent
./install.sh    # the above, plus the symlink and the herdr plugin
cargo test      # 185 tests, none of which need a window manager or a screen
```

`./spaces.sh on` builds, then points slingr, herdr's hooks and the agent at
this checkout; `./spaces.sh off` hands everything back to AeroSpace.

`plugin/herdr-plugin.toml` is generated from the `.example` by `install.sh`
and `spaces.sh`, and is git-ignored: herdr needs an absolute path, which a
repository cannot know.

`cargo build` alone does not build the Swift panel. If `slingr-panel` is missing
or stale, sling silently falls back to the AppleScript dialog — `slingr paths`
says which it will use.

## Shape

- `src/picker.rs` — the rules, with no I/O: parsing, naming, grouping, menu
  building. Tested directly.
- `src/app.rs` — the flows, written against the `WindowManager` and `Prompt`
  traits so they can be driven with no window manager and no screen.
- `src/wm.rs` — the `WindowManager` trait, and which backend answers it.
- `src/aerospace.rs`, `src/spaces.rs`, `src/herdr.rs`, `src/dialog.rs`,
  `src/panel.rs` — the impure edges. Each is thin on purpose.
- `spaces/main.swift` — `slingr-spaces`, which reads macOS Spaces through
  SkyLight and needs no permission. Switching is not its: only the Dock's own
  switch hides the desktop being left.
- `agent/main.swift` — the menu-bar agent the Spaces backend runs under
  launchd. It owns the three keys, switches desktops through the Dock — a
  made-up trackpad swipe, with no slide, or else the Dock's "Switch to
  Desktop N" — adds desktops through Mission Control, and writes task names
  over its "Desktop N" labels. It holds no state and makes no decisions.
- `src/attention.rs` — agents that want you: when to say so in a notification,
  and what a click on one does (`slingr notify`, `slingr answer`). Its flows
  are written against the `World` trait, for the same reason.
- `panel/main.swift` — presentation only. Reads rows as JSON on stdin, prints
  chosen ids. All the thinking stays in Rust. Two surfaces: `PanelView` (the
  list) and `BoardView` (the tiles), chosen by the request's `layout`. The
  field spelling between the two languages is pinned by a test in
  `src/panel.rs` — a renamed field is not a compile error in Swift, it is a
  value that silently never arrives.
- `tests/flow.rs` — whole flows against fakes. If a change is about *ordering*
  (what is called before what), assert it here; that is where the worst bug in
  this project lived.

## Conventions

- UK English in names and comments.
- Conventional Commits.
- Comments explain *why*, especially where the code looks odd — it is usually
  working around something in `docs/FINDINGS.md`. Do not remove one without
  reading that file.
- Every non-obvious constraint should be a test, not a comment alone.

## Live configuration this depends on

Either backend:

- `plugin/herdr-plugin.toml` — linked with `herdr plugin link`, runs
  `slingr goto` on `tab.focused` and `slingr notify` on
  `pane.agent_status_changed`.
- `~/.config/herdr/config.toml` — `[ui.toast] delivery = "off"`, because
  slingr says those itself, in notifications a click takes you through.
- `~/.config/slingr/workspaces.toml` — `backend`, and the tasks that are not
  herdr tabs.

AeroSpace:

- `~/.aerospace.toml` — `config-version = 2`; keybindings `ctrl-alt-cmd-s`
  (sling), `ctrl-alt-cmd-i` (jump) and `ctrl-alt-cmd-b` (board);
  `exec-on-workspace-change` runs
  `slingr follow`. The `# slingr:begin` block is written by `slingr sync` — do not
  hand-edit it, and note that a duplicate `persistent-workspaces` key makes
  AeroSpace reject the whole file silently.

macOS Spaces, all set up by `./spaces.sh on`:

- `~/Library/LaunchAgents/dev.slingr.agent.plist` — runs the agent from this
  checkout's `target/release/SlingrAgent.app`. The agent needs Accessibility,
  to switch and add desktops, and Screen Recording, for window titles. Both
  belong to its signature, and have survived rebuilds and a move to another
  checkout.
- The Dock's "Switch to Desktop N" shortcuts, ctrl-F1 to ctrl-F16, which the
  agent presses when it does not swipe.
- `switchInstantly` in the agent's preferences (`dev.slingr.agent`) — its
  menu's "Switch desktops instantly". Absent means on.
- `~/.local/state/slingr/spaces.json` — which desktop is which task's.

slingr itself keeps nothing running. Its hooks are invoked by herdr, and by
AeroSpace or the agent; a slingr daemon was tried and removed, because one
that quietly dies is indistinguishable from a broken follow list. The agent is
resident, but stands where AeroSpace stood: a key or a desktop change becomes
the same `slingr` call, with no state or decisions of its own.
