# CLAUDE.md

slingr: a window picker for AeroSpace, built around tasks rather than
applications. Named to match herdr, which it pairs with. A task is usually a herdr tab, and the windows that belong with
it live in one AeroSpace workspace.

**Read `docs/SPEC.md` first** — what this is for and why it is shaped as it is.
**Read `docs/FINDINGS.md` before changing anything that touches AeroSpace or
herdr.** It records behaviour that is in neither project's documentation, found
by breaking things. Most of it cost an evening. The single biggest lesson is at
the top: check the installed version against the latest release before
designing around a limitation.

## Names

"sling" stays the verb — you sling a window, a window was slung. Only the tool
is `slingr`.

## Build

```sh
./build.sh      # cargo build --release AND swiftc the panel
./install.sh    # the above, plus the symlink and the herdr plugin
cargo test      # 121 tests, none of which need AeroSpace or a screen
```

`plugin/herdr-plugin.toml` is generated from the `.example` by `install.sh`
and is git-ignored: herdr needs an absolute path, which a repository cannot
know.

`cargo build` alone does not build the Swift panel. If `slingr-panel` is missing
or stale, sling silently falls back to the AppleScript dialog — `slingr paths`
says which it will use.

## Shape

- `src/picker.rs` — the rules, with no I/O: parsing, naming, grouping, menu
  building. Tested directly.
- `src/app.rs` — the flows, written against the `WindowManager` and `Prompt`
  traits so they can be driven with no window manager and no screen.
- `src/aerospace.rs`, `src/herdr.rs`, `src/dialog.rs`, `src/panel.rs` — the
  impure edges. Each is thin on purpose.
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

- `~/.aerospace.toml` — `config-version = 2`; keybindings `ctrl-alt-cmd-s`
  (sling), `ctrl-alt-cmd-i` (jump) and `ctrl-alt-cmd-b` (board);
  `exec-on-workspace-change` runs
  `slingr follow`. The `# slingr:begin` block is written by `slingr sync` — do not
  hand-edit it, and note that a duplicate `persistent-workspaces` key makes
  AeroSpace reject the whole file silently.
- `plugin/herdr-plugin.toml` — linked with `herdr plugin link`, runs
  `slingr goto` on `tab.focused`.
- `~/.config/slingr/workspaces.toml` — the tasks that are not herdr tabs.

Nothing runs resident. Both hooks are invoked by AeroSpace and herdr
themselves; a daemon was tried and removed, because one that quietly dies is
indistinguishable from a broken follow list.
