# macOS Spaces, one per task

Begun on 26 October as an experiment: does a task feel more natural as a
native macOS Space than as an AeroSpace workspace? The draw is that macOS
keeps a window on its Space by itself — nothing of ours has to remember it,
and nothing restarting can forget it — and that Mission Control and
ctrl-arrow would at last group by task, which is the whole reason the board
exists.

It was judged the better of the two, and on 12 November merged as the backend
in daily use. AeroSpace stays the other one, and `./spaces.sh off` goes back
to it. The measurements below stay here rather than in `FINDINGS.md`, because
they are about this backend alone.

## What macOS allows, measured

macOS 26.6.2 (Darwin 25.6.0), SIP enabled, 26 October 2026. The probes are
small Swift programs that call SkyLight through `dlsym`; `spaces/main.swift`
grew out of them.

| | |
|---|---|
| Read every screen's Spaces, in order, and which is showing | yes — `SLSCopyManagedDisplaySpaces`, no permission |
| Read which Space each window is on | yes — `SLSCopySpacesForWindows`, no permission |
| Read window titles | only with Screen Recording — 0 of 165 without it |
| Switch a screen to another Space | half — `SLSManagedDisplaySetCurrentSpace`, no permission and no slide, but the old desktop's windows stay drawn; not used |
| Have the Dock switch | yes — its own "Switch to Desktop N", pressed by the agent with Accessibility; 0.4–0.5s, and it slides |
| Move **another process's** window to another Space | **no** — refused by every route tried |
| Move a process's **own** window to another Space | yes, and back |
| Add a desktop | yes — Mission Control's **+**, pressed through Accessibility; 0.8s, and it shows |
| Rename a desktop | **no** — but names can be laid over Mission Control's row, by the agent, with Accessibility |

### Adding a desktop goes through Mission Control

The Dock owns the list of desktops, and offers to add one only in Mission
Control's own interface. Hammerspoon's `hs.spaces.addSpaceToScreen` drives that
interface, and the agent does the same: `CoreDockSendNotification` with
`com.apple.expose.awake` (exported from HIServices here) opens Mission Control;
the Dock's Accessibility tree then holds `mc` → `mc.display` (by
`AXDisplayID`) → `mc.spaces` → `mc.spaces.add` and `mc.spaces.list`; pressing
the first adds a desktop, and pressing the new entry in the second goes into it
and closes Mission Control in one movement. The elements exist only while
Mission Control is showing, so there is no doing it unseen.

```
19:53:29  no desktop added (8ms): slingr agent needs Accessibility
19:54:13  added a desktop in 774ms: Desktop 3 after ["", ""]
          slingr-spaces add answered with the new desktop, now showing: 877ms
```

The desktops already there had no names when read straight after Mission
Control opened. Read after Hammerspoon's 0.3s, they had them, and the new one
came last in the row:

```
19:57:18  added a desktop in 834ms: Desktop 4: ["Desktop 1", "Desktop 2", "Desktop 3"]
          became ["Desktop 1", "Desktop 2", "Desktop 3", "Desktop 4"]
```

That add came after the agent had been rebuilt and restarted: Accessibility,
like Screen Recording, survives a rebuild signed with the same identity.

### Names over Mission Control, since a desktop cannot have one

Mission Control calls every desktop "Desktop N", which is no help when a
window is dragged to the top of the screen to go to another task's desktop.
macOS has no setting for it. The tools that rename the label itself inject into
the Dock — Spaces Renamer, TotalSpaces — and need SIP off. The ones that leave
SIP alone lay their own label over the row instead, at the places the Dock's
Accessibility tree gives: DeskStag sells it, and CloseUp, nametag and
spaceballs are open source. CloseUp's `AGENTS.md` is the most careful record of
what that takes on the Dock-drawn Mission Control of macOS 26 and earlier. The
agent does the same, using its existing Accessibility, with `slingr labels`
saying which task each desktop is for. A desktop with no task keeps
Mission Control's own name.

- **Opening.** The Dock posts `AXExposeShowAllWindows` to an AXObserver, as
  yabai and AltTab rely on. Measured from a toggle checked against the screen:
  Mission Control's surface was up 0.10s after the toggle, the notification
  came 0.19s after that, and the names were up 0.33s later, once the row had
  held still for two reads. CloseUp found the observer goes deaf after a switch
  into a full-screen window's Space, so it is made afresh on every Space
  change. Earlier runs that logged nothing turned out to be the probe's own
  toggle (`open -b com.apple.exposelauncher`) not opening Mission Control.
- **Where.** On the main screen the row filled the top 40 points, but each
  desktop's frame sat 32 points *above* the top of the screen — as spaceballs
  found on the Dock's tree. So the row gives the height and the desktop only
  how far across. Closing, the row slides 40 points up before it goes.
- **Above it.** At screen-saver level, with `canJoinAllSpaces`, and not
  `transient`, which Mission Control hides. CloseUp found a window put up
  while Mission Control is still moving things sinks below what it draws next,
  so the names go up only once the row holds still, in fresh windows each time.
- **Closing** is read from the tree, not from `AXExposeExit`: CloseUp saw that
  posted during swipes that leave Mission Control open, so it only takes the
  names down until the row is still again.
- **Matching** is by position, so a screen whose row has a different number
  of Spaces from slingr's list gets no names at all: a wrong name is worse than
  none.
- **Opening into pictures**, when the pointer goes to the top or a window is
  dragged up, is where names went wrong. Over ten days of the agent's log,
  27 October to 6 November, names over the row of labels never went past the
  screen's edge. The row of pictures was read in many places. For every
  number of desktops the commonest was centred on the screen, to within two
  points: 15 desktops at 76 from the left and 77 from the right, 10 at 501
  and 502. But 104 of the 459 times names went up on it, the row was read
  partly past the screen's edge. 86 times names were drawn past it, some on
  the next screen. A row read like that now goes unnamed until it fits. The
  log cannot tell whether the rest of the off-centre reads were the row really
  sitting there or the Dock's report lagging behind it. So each placement now
  logs the pointer, in case the row opens around it.

Checked again on 6 November, because an overlay never lines up as well as a
name would. There is no name to set:

- The label is the Dock's own string, `DesktopNum` = `Desktop %@` in
  `Dock.app/Contents/Resources/en.lproj/Localizable.strings`, filled with a
  number. Nothing about a particular desktop goes into it.
- SkyLight does have `SLSSpaceSetName`, but a Space's name is its uuid:
  `SLSSpaceCopyName` gave the uuid for all six Spaces read, the uuid
  `com.apple.spaces` keeps and claims are made by. Setting it would change
  which Space this is, not what Mission Control calls it, so it was not tried.
- spaces-manager, tested on 26.3.1, says the label itself can only be changed
  by injecting into the Dock, which needs SIP off. The App Store's Desktop
  Space Renamer puts its names in the menu bar instead, as the agent does.

### macOS reorders desktops unless told not to

`[3, 1310, 1377]` became `[3, 1377, 1310]` within minutes of use, with nobody
dragging anything: `com.apple.dock mru-spaces` is unset, so "Automatically
rearrange Spaces based on most recent use" is on, as it is by default.
ShiftPlus hit the same. A task keeps its desktop through it — claims are by
uuid — but a `desktop-N` name moves, and so does where a task sits under
ctrl-arrow.

### Switching behind the Dock's back

`SLSManagedDisplaySetCurrentSpace` is the one write SkyLight still allows, and
it is what `switch` used first. But the Dock, not WindowServer, owns what usually
comes with a switch: ShiftPlus found the call fires no `kCGSSpaceDidChange` on
macOS 15 and later, and that the screen could stay frozen on the old desktop
until a synthetic swipe made the Dock repaint. Here the screen did change and
`activeSpaceDidChange` did fire, but within the first hour of switching the
menu bar showed two applications' menus drawn over each other. Not yet shown to be
caused by the switch.

So switching goes through the Dock now. Its "Switch to Desktop N" shortcuts
are symbolic hotkeys 118–133 in `com.apple.symbolichotkeys`; only 1–3 were set
(ctrl-F1–F3), so `./spaces.sh shortcuts` — which `on` runs — sets 4–16 to
ctrl-F4–F16 wherever nothing is switched on, and `activateSettings -u` applies
them without logging out. The helper asks the agent for a desktop *number*;
the agent reads that desktop's shortcut and presses it. A request never names
a key, since anything can post one, and the agent's Accessibility must not
become a way to type. The helper then waits for SkyLight to show the switch:

```
switch 1310 (Desktop 3, ctrl-F3)          493ms   pressed Desktop 3
switch 1377 (Desktop 2, ctrl-F2)          379ms   pressed Desktop 2
switch 1382 (Desktop 4, ctrl-F4, new)     456ms   pressed Desktop 4
```

The number is the desktop's place among its screen's desktops, which is how
Mission Control numbers them; only the main screen's desktops are ever
switched to, and the pointer was on it throughout. SkyLight was kept as the
fallback — no agent, no Accessibility, no shortcut, or 1.5s without the switch
landing — until 6 November, below. Now there is none: a switch the Dock does
not make fails, and slingr says so in a notification.

### Switching instantly through SkyLight: tried, and taken out

The Dock's switch slides, and the slide is a flash some people cannot look
at. Reduce Motion turns it into a fade, but it changes every animation on the
machine, which rules it out for anyone testing animations on web pages. So on
6 November the agent's menu gained "Switch desktops instantly": ticked, the
helper switched through SkyLight first, which cuts straight to the desktop.

It made 15 switches in four and a half minutes, a median of 166ms a `goto`
against 609ms for the 100 the Dock made earlier that day, and the agent heard
every one. But each desktop it left that had windows of its own kept them on
screen, over the next desktop: t-pricing's Brave, t-shadcn's Brave and
t-shad3's Chrome, three of three, all drawn over me-slingr's desktop at the
end. CGWindowList called them on screen while SkyLight still put each on its
own Space, so nothing had moved. The four desktops left that had no windows
of their own showed nothing wrong.

What did not mend it, with the switch turned off again:

- `SLSHideSpaces` on one of the three Spaces, from outside the Dock: it
  returned `15630336` and the window stayed. Hiding a desktop is the Dock's.
- Restarting the Dock (`killall Dock`): all three stayed.

What hides a desktop is the Dock's own switch away from it, as every
desktop the Dock had left that day showed. So the setting and the SkyLight
fallback are both gone, and a test pins that the helper never looks
`SLSManagedDisplaySetCurrentSpace` up again. The menu bar drawn over itself on
26 October was probably the same half switch.

The agent did hear these switches. On 26 October, before the agent first
pressed a shortcut at 20:29, every switch went through SkyLight. `follow` ran
after all 17 of slingr's switches in the half hour before that. 15 logged
`catch: "Arrived"` in `watch.jsonl`, and 2 stood aside mid-switch. The agent
runs `follow` only on `activeSpaceDidChange`.

An instant switch would have to be the Dock's own. InstantSpaceSwitcher posts
the Dock a trackpad swipe with a near-zero distance and a high speed, one per
desktop moved, and reports that the Dock then switches without the slide. Not
yet tried here.

### Moving a window is refused, three ways

A throwaway panel was opened by one process and moved by another — first to
the second screen's Space, then, once a second desktop existed, to that, on
the same screen:

```
move:      window 28977 spaces [3] -> [3]  unchanged    SLSMoveWindowsToManagedSpace
compat:    window 28977 spaces [3] -> [3]  unchanged    SetCompatID + SetWindowListWorkspace
addremove: window 28977 spaces [3] -> [3]  unchanged    SLSAddWindowsToSpaces + RemoveFromSpaces
```

`SLSSetWindowListWorkspace` returns `1006`, which is `kCGErrorNotImplemented`
in `CGError.h`: yabai's workaround for macOS 14.5 and later is gone, not merely
blocked. The same call made by the window's **own** process moved it and moved
it back, so the probe was sound and the refusal is about ownership.

yabai now prefers `SLSBridgedMoveWindowsToManagedSpaceOperation`. The class
exists on this machine; the C function yabai runs it through,
`SLSPerformAsynchronousBridgedWindowManagementOperation`, does not, and nothing
else exported from SkyLight performs one. ShiftPlus report the same refusal on
15 and 26 ([their write-up](https://shiftplus.app/blog/macos-spaces-without-disabling-sip/)).

**So nothing of ours can put a window on a Space.** A window gets onto one by
being opened there — a new window opens on the Space in front — or by being
dragged in Mission Control.

### Switching works, and macOS notices

```
  305ms before:                display current=3    active=3
  328ms notification: activeSpaceDidChange
 1313ms after switching:       display current=1310 active=1310
 1323ms notification: activeSpaceDidChange
 2133ms after switching back:  display current=3    active=3
```

Seen on screen too: the main display went to the empty desktop and came back,
and nothing looked wrong afterwards. ShiftPlus report that the notification
does *not* fire on 15 and later; here it did, at least in the process that
switched. Not yet checked: whether Mission Control and ctrl-arrow stay in step
over many switches made this way.

### A window on every desktop is on all of them

Ghostty and WhatsApp, assigned to All Desktops from the Dock, are reported on
every desktop of their screen — `[3, 1310]` — and on the second screen's one
desktop for the Ghostty windows there. That is what lets `free_space` ignore
them when it looks for an empty desktop.

## How it works

`backend = "spaces"` in `~/.config/slingr/workspaces.toml`, or
`SLINGR_BACKEND=spaces` for one command.

- **One Space per task.** A task's first visit — from herdr, from jump —
  claims the first empty, unclaimed desktop on the main screen, and keeps it.
  The claim lives in `~/.local/state/slingr/spaces.json`, by the Space's uuid,
  since macOS has no names for Spaces.
- **No empty desktop, so one is made.** slingr asks the agent through the
  helper (`slingr-spaces add`, a distributed notification), and the agent adds
  one through Mission Control, above. The helper watches SkyLight for the new
  desktop rather than trusting a reply, and answers with it, or with the
  agent's reason for refusing — which becomes the notification.
- **Unclaimed desktops are places**, named the way Mission Control numbers
  them: `desktop-2`, `screen-2-desktop-1`. They can be jumped to, and appear on
  the board.
- **`slingr bind NAME`** gives the desktop in front to a task — how the one
  everything already sits in becomes a task, rather than waiting for a task to
  claim an empty one. Nothing about the Space changes, so slingr tells the
  agent (`slingr-spaces refresh`); measured, `slingr bind home` renamed the
  menu bar at once. A task that is not a herdr tab, such as `home`, is still
  one jump lists, and herdr stays where it is on the way there.
- **`slingr spaces`** lists every desktop and the task it is for.
- **Mission Control says the task**, over its own "Desktop N", above.

What AeroSpace did for slingr, and what does it now:

| | AeroSpace | Spaces |
|---|---|---|
| a workspace per task | `persistent-workspaces` | a desktop, claimed on first visit, made if none is free |
| herdr tab → task | herdr hook runs `slingr goto` | the same; the Dock switches |
| task → herdr tab | `exec-on-workspace-change` | the agent, on `activeSpaceDidChange` |
| ctrl-alt-cmd-s/i/b | AeroSpace keybindings | the agent |
| windows that belong everywhere | moved by slingr on arrival | Dock → Options → All Desktops |
| sling, sling many, board drags | moved by id | **not offered** |
| restore after a restart | the action log | not needed: macOS keeps them |

The sling key opens on jump, since there is nothing to sling with. The board
draws every task and its windows and goes where you click, with no drags.

### The agent

`agent/main.swift`, built into `target/release/SlingrAgent.app`. It is the one
resident thing, and it stands where AeroSpace stood rather than being a slingr
daemon: no state, no decisions — a key or a Space change becomes the same
`slingr` call AeroSpace used to make. Its menu-bar item names the task in
front, since macOS cannot, so a missing name is a missing agent. launchd
starts it again after a crash; quitting it from the menu is left alone.

It is an app bundle so there is something to grant Screen Recording to: the
helper inherits it, and titles appear. `build.sh` signs it with an Apple
Development identity when there is one, because a permission granted to an
ad-hoc signature is lost on the next build.

Allowing Screen Recording quit the agent launchd had started — cleanly, so
launchd left it stopped — and three seconds later the app was open again,
outside launchd's job (`XPC_SERVICE_NAME=application.dev.slingr.agent.…`
rather than `dev.slingr.agent`): logging nowhere, and with nothing to start it
again after a crash. So a copy launchd did not start runs `launchctl kickstart`
on the job and exits. Checked both ways by opening the app by hand: with the
job stopped, launchd's copy started and the opened one left; with it running,
the one agent stayed the only one. Each start logs whether Screen Recording is
allowed, so that can be read rather than guessed at.

## Trying it

```sh
./spaces.sh on     # build, stop AeroSpace, point slingr/herdr/the keys here
./spaces.sh off    # all of it back to the main checkout and AeroSpace
```

Then, once:

- Turn off "Automatically rearrange Spaces based on most recent use", so
  desktops stay where they are put.
- Allow Screen Recording for "slingr agent" when asked.
- Assign anything that should be everywhere to All Desktops from its Dock
  menu. Ghostty and WhatsApp already are.
- Worth trying: System Settings → Desktop & Dock → "When switching to an
  application, switch to a Space with open windows for the application". It is
  off here (`AppleSpacesSwitchOnActivate = 0`), which suited AeroSpace; with
  Spaces it is how cmd-tab would take you to a task, and the agent would bring
  herdr's tab along.
- Allow Accessibility for "slingr agent" — asked the first time a task needs
  a desktop made, or from "Allow Accessibility…" in its menu. Without it, a
  task with no empty desktop gets a notification instead. It also lets a click
  on the board raise the window clicked, rather than whichever of that
  application's windows it prefers.

## Not known yet

- Whether a Space's uuid survives a reboot. Claims are kept by uuid on the
  strength of macOS keeping it in `com.apple.spaces`; the numeric id is the
  fallback, and whether *it* survives is not known either.
- Whether applications put their windows back on the right desktops after a
  restart — Brave and Ghostty restore their own windows.
- Whether the helper, run by slingr run by the agent, really is given the
  agent's Screen Recording — that is, whether the board shows titles.
- Where Mission Control puts a new desktop when a full-screen window's Space is
  at the end of the row.
- What becomes of a closed tab's desktop: nothing yet. It stays claimed, and
  Mission Control is reported to stop at sixteen desktops a screen.
- Whether a row of pictures read off centre, but within its screen, is
  where the pictures really are. Names still go up on one. The pointer is now
  logged with each placement, to show whether the row opens around it.
- Whether dropping a window on a desktop redraws Mission Control over the
  names, as CloseUp found its re-tiles do.
- Whether "Switch to Desktop N" acts on the screen under the pointer rather
  than the main screen. The pointer was on the main screen for every
  measurement; if not, the switch fails after 1.5s, and slingr says so.

## What the next iterations could be

- **Switching instantly, through the Dock**: InstantSpaceSwitcher's swipe,
  posted by the agent in place of the shortcut. The Dock would make the
  switch, so its desktop is hidden as it should be. Untested here: whether
  a jump of several desktops, one swipe each, shows the ones in between.

- **Sling by dragging for you**: a synthetic mouse-down on the title bar, a
  desktop switch, a mouse-up. Needs Accessibility, flips Spaces visibly, and
  must find a part of a browser's title bar that is not a tab. Untested.
- **Name the desktop in the panel** instead of `slingr bind`.
- **yabai with SIP partly off**, the only route found that moves windows
  properly — reported broken on 26.1 and 26.2 for some.
