// slingr-spaces — macOS Spaces, as plain answers on stdout.
//
// The impure edge of the Spaces backend and nothing more: it reports what
// SkyLight and AppKit say, and switches when told to. Which task a Space is
// for is decided in Rust (`src/spaces.rs`); this has no idea tasks exist.
//
//   slingr-spaces state             the whole machine, as one JSON object
//   slingr-spaces switch <space>    show that Space on its screen
//   slingr-spaces focus <window>    go to the window's Space, bring it forward
//   slingr-spaces add               have the agent add a desktop to the main
//                                   screen, and answer with it
//   slingr-spaces refresh           have the agent re-read the task in front
//
// SkyLight is private, so its functions are looked up when needed rather than
// linked: a symbol that disappears in a later macOS fails one command with a
// message, instead of stopping the binary from loading at all.
//
// Measured on macOS 26.6.2 with SIP on — see docs/SPACES.md. Reading and
// switching need no permission at all. Moving another process's window to a
// different Space is refused by every route tried, so there is no command for
// it: one that could only ever fail would be a lie in the interface.

import AppKit

// MARK: - what Rust reads

struct SpaceOut: Encodable {
    let id: UInt64
    let uuid: String
    let kind: String
}

struct DisplayOut: Encodable {
    let id: String
    let main: Bool
    let current: UInt64
    let spaces: [SpaceOut]
}

struct WindowOut: Encodable {
    let id: UInt32
    let pid: Int32
    let app: String
    let bundle: String
    let title: String
    let spaces: [UInt64]
}

struct StateOut: Encodable {
    let displays: [DisplayOut]
    let windows: [WindowOut]
    let focused: UInt32?
    let active: UInt64
}

/// What `add` answers: the desktop that now exists, or why there is none. Both
/// arrive with a zero exit, because a failing exit loses what was printed.
struct AddedOut: Encodable {
    let space: SpaceOut?
    let refused: String?
}

// MARK: - SkyLight

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("slingr-spaces: \(message)\n".utf8))
    exit(1)
}

enum Sky {
    static let handle = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_NOW)

    static func fn<T>(_ name: String, as _: T.Type) -> T {
        guard let handle, let symbol = dlsym(handle, name) else {
            fail("SkyLight has no \(name) on this macOS")
        }
        return unsafeBitCast(symbol, to: T.self)
    }

    typealias MainConnection = @convention(c) () -> Int32
    typealias CopyManagedDisplaySpaces = @convention(c) (Int32) -> Unmanaged<CFArray>?
    typealias GetActiveSpace = @convention(c) (Int32) -> UInt64
    typealias CopySpacesForWindows = @convention(c) (Int32, Int32, CFArray) -> Unmanaged<CFArray>?
    typealias SetCurrentSpace = @convention(c) (Int32, CFString, UInt64) -> Void

    static let connection = fn("SLSMainConnectionID", as: MainConnection.self)()
}

/// The screen with the menu bar, spelled the way SkyLight names displays.
func mainDisplayUUID() -> String {
    guard let uuid = CGDisplayCreateUUIDFromDisplayID(CGMainDisplayID())?.takeRetainedValue() else {
        return ""
    }
    return CFUUIDCreateString(nil, uuid) as String
}

/// SkyLight's own numbering, as yabai reads it: 0 is a desktop and 4 the Space
/// a full-screen window makes for itself. Only desktops ever hold a task.
func kind(of type: Int) -> String {
    switch type {
    case 0: return "desktop"
    case 4: return "fullscreen"
    default: return "other"
    }
}

func displays() -> [DisplayOut] {
    let copy = Sky.fn("SLSCopyManagedDisplaySpaces", as: Sky.CopyManagedDisplaySpaces.self)
    let raw = copy(Sky.connection)?.takeRetainedValue() as? [[String: Any]] ?? []
    let main = mainDisplayUUID()
    return raw.map { display in
        let id = display["Display Identifier"] as? String ?? ""
        let current = ((display["Current Space"] as? [String: Any])?["id64"] as? NSNumber)?.uint64Value ?? 0
        let spaces = (display["Spaces"] as? [[String: Any]] ?? []).map { space in
            SpaceOut(
                id: (space["id64"] as? NSNumber)?.uint64Value ?? 0,
                uuid: space["uuid"] as? String ?? "",
                kind: kind(of: (space["type"] as? NSNumber)?.intValue ?? 0)
            )
        }
        // `com.apple.spaces` calls the main screen "Main" rather than by its
        // uuid, so either spelling counts.
        return DisplayOut(id: id, main: id == main || id == "Main", current: current, spaces: spaces)
    }
}

func activeSpace() -> UInt64 {
    Sky.fn("SLSGetActiveSpace", as: Sky.GetActiveSpace.self)(Sky.connection)
}

func spaces(of window: UInt32) -> [UInt64] {
    let copy = Sky.fn("SLSCopySpacesForWindows", as: Sky.CopySpacesForWindows.self)
    // 0x7: every kind of Space the window could be on, current or not.
    let ids = [NSNumber(value: Int32(bitPattern: window))] as CFArray
    return (copy(Sky.connection, 0x7, ids)?.takeRetainedValue() as? [NSNumber])?.map(\.uint64Value) ?? []
}

// MARK: - windows

func windows() -> [WindowOut] {
    let info = CGWindowListCopyWindowInfo([.optionAll, .excludeDesktopElements], kCGNullWindowID)
        as? [[String: Any]] ?? []
    let me = ProcessInfo.processInfo.processIdentifier
    var owners: [Int32: NSRunningApplication?] = [:]

    return info.compactMap { w in
        guard (w[kCGWindowLayer as String] as? Int) == 0,
              let id = (w[kCGWindowNumber as String] as? NSNumber)?.uint32Value,
              let pid = (w[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value,
              pid != me
        else { return nil }

        if owners[pid] == nil { owners[pid] = NSRunningApplication(processIdentifier: pid) }
        // Agents, helpers and slingr's own panel draw windows too. Only an
        // application with a place in the Dock owns what a person would call
        // a window.
        guard let owner = owners[pid] ?? nil, owner.activationPolicy == .regular else { return nil }

        let bounds = w[kCGWindowBounds as String] as? [String: CGFloat] ?? [:]
        guard (bounds["Width"] ?? 0) >= 40, (bounds["Height"] ?? 0) >= 40 else { return nil }

        // Most layer-0 windows are on no Space at all — 140 of 165 on the
        // machine this was written on — and there is nowhere to go to see one.
        let on = spaces(of: id)
        guard !on.isEmpty else { return nil }

        return WindowOut(
            id: id,
            pid: pid,
            app: owner.localizedName ?? (w[kCGWindowOwnerName as String] as? String ?? ""),
            bundle: owner.bundleIdentifier ?? "",
            // Empty unless whoever started this has Screen Recording.
            title: w[kCGWindowName as String] as? String ?? "",
            spaces: on
        )
    }
}

/// The window with focus, as near as can be told without Accessibility: the
/// frontmost of the frontmost application's windows. On-screen windows are
/// listed front to back.
func focusedWindow() -> UInt32? {
    guard let front = NSWorkspace.shared.frontmostApplication?.processIdentifier else { return nil }
    let showing = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
        as? [[String: Any]] ?? []
    return showing
        .first {
            ($0[kCGWindowLayer as String] as? Int) == 0
                && ($0[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == front
        }
        .flatMap { ($0[kCGWindowNumber as String] as? NSNumber)?.uint32Value }
}

// MARK: - acting

/// Show `space` on whichever screen it belongs to, and wait until SkyLight
/// says it is showing.
func switchTo(_ space: UInt64) -> Bool {
    guard let display = displays().first(where: { $0.spaces.contains { $0.id == space } }) else {
        fail("no screen has a Space \(space)")
    }
    if display.current == space { return true }
    if switchThroughDock(space, on: display) { return true }

    // Behind the Dock's back: the one write SkyLight still allows, and all of
    // a switch the Dock does not hear about. Kept for when the agent cannot
    // press anything — not running, no Accessibility, no shortcut.
    let set = Sky.fn("SLSManagedDisplaySetCurrentSpace", as: Sky.SetCurrentSpace.self)
    set(Sky.connection, display.id as CFString, space)

    // A second is generous; the switch confirms itself well inside one.
    for _ in 0..<20 {
        if displays().first(where: { $0.id == display.id })?.current == space { return true }
        usleep(50_000)
    }
    return false
}

/// Raise one particular window. Needs Accessibility; without it the
/// application decides which of its windows comes forward.
func raise(_ window: UInt32, of pid: Int32) {
    typealias GetWindow = @convention(c) (AXUIElement, UnsafeMutablePointer<UInt32>) -> AXError
    guard AXIsProcessTrusted(), let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "_AXUIElementGetWindow")
    else { return }
    let windowOf = unsafeBitCast(symbol, to: GetWindow.self)

    let app = AXUIElementCreateApplication(pid)
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &value) == .success,
          let list = value as? [AXUIElement]
    else { return }
    for element in list {
        var id: UInt32 = 0
        if windowOf(element, &id) == .success, id == window {
            AXUIElementPerformAction(element, kAXRaiseAction as CFString)
            return
        }
    }
}

func focus(_ id: UInt32) -> Bool {
    guard let window = windows().first(where: { $0.id == id }) else { fail("no window \(id)") }
    if !window.spaces.contains(activeSpace()), let space = window.spaces.first, !switchTo(space) {
        return false
    }
    raise(id, of: window.pid)
    guard let app = NSRunningApplication(processIdentifier: window.pid) else { return false }
    return app.activate()
}

// MARK: - asking the agent

/// Spelled the same in agent/main.swift, which a test in src/spaces.rs pins.
enum Agent {
    static let bundle = "dev.slingr.agent"
    static let addDesktop = Notification.Name("dev.slingr.agent.add-desktop")
    static let refused = Notification.Name("dev.slingr.agent.add-desktop.refused")
    static let switchDesktop = Notification.Name("dev.slingr.agent.switch-desktop")
    static let switchRefused = Notification.Name("dev.slingr.agent.switch-desktop.refused")
    static let refresh = Notification.Name("dev.slingr.agent.refresh")

    static var running: Bool {
        !NSRunningApplication.runningApplications(withBundleIdentifier: bundle).isEmpty
    }

    static func tell(_ name: Notification.Name) {
        // The agent is never the active application, and an inactive one has
        // its distributed notifications held back unless they say otherwise.
        DistributedNotificationCenter.default()
            .postNotificationName(name, object: nil, userInfo: nil, deliverImmediately: true)
    }
}

/// Wait out one slice of the run loop, which is where an answer from the
/// agent arrives. It returns at once if it has nothing to wait on, so the rest
/// of the slice is slept.
///
/// Not `pause`: libc has one taking no arguments, and calling a `pause` with
/// none calls libc's instead — a wait for a signal that never comes. It hung
/// the first build of this.
func slice(_ seconds: TimeInterval = 0.05) {
    let until = Date().addingTimeInterval(seconds)
    RunLoop.current.run(until: until)
    if until.timeIntervalSinceNow > 0 { usleep(useconds_t(until.timeIntervalSinceNow * 1_000_000)) }
}

/// The number Mission Control gives a desktop, which is what "Switch to
/// Desktop N" counts: desktops only, full-screen windows' Spaces skipped.
func desktopNumber(of space: UInt64, on display: DisplayOut) -> Int? {
    display.spaces.filter { $0.kind == "desktop" }.firstIndex { $0.id == space }.map { $0 + 1 }
}

/// Have the agent press the Dock's own shortcut for the desktop, so the Dock
/// makes the switch with everything a switch involves. Waits for the switch
/// itself rather than a reply; a refusal arrives at once.
func switchThroughDock(_ space: UInt64, on display: DisplayOut) -> Bool {
    guard Agent.running, let n = desktopNumber(of: space, on: display) else { return false }
    var refused = false
    let centre = DistributedNotificationCenter.default()
    let listening = centre.addObserver(forName: Agent.switchRefused, object: nil, queue: nil) { _ in refused = true }
    defer { centre.removeObserver(listening) }
    centre.postNotificationName(Agent.switchDesktop, object: String(n), userInfo: nil, deliverImmediately: true)

    // The Dock animates; half a second is usual. Past this, the press did not
    // land, and SkyLight is asked instead.
    let deadline = Date().addingTimeInterval(1.5)
    while Date() < deadline {
        slice()
        if refused { return false }
        if displays().first(where: { $0.id == display.id })?.current == space { return true }
    }
    return false
}

func mainScreen() -> DisplayOut {
    guard let main = displays().first(where: { $0.main }) else { fail("no main screen reported") }
    return main
}

/// Only the Dock can make a desktop that Mission Control keeps, and only
/// through its own interface, which takes Accessibility to drive. The agent
/// holds that, so this asks it to and watches SkyLight for the result: the
/// new desktop itself is the answer, not anything the agent says.
func addDesktop() -> AddedOut {
    if !Agent.running {
        return AddedOut(space: nil, refused: "slingr agent is not running")
    }
    let before = mainScreen()
    let had = Set(before.spaces.map(\.id))

    var refusal: String?
    let centre = DistributedNotificationCenter.default()
    let listening = centre.addObserver(forName: Agent.refused, object: nil, queue: nil) { note in
        refusal = note.object as? String ?? "the agent could not add one"
    }
    defer { centre.removeObserver(listening) }
    Agent.tell(Agent.addDesktop)

    // Mission Control has to open before there is a button to press.
    let deadline = Date().addingTimeInterval(8)
    while Date() < deadline {
        slice()
        if let refusal { return AddedOut(space: nil, refused: refusal) }
        let now = displays().first { $0.id == before.id }
        guard let new = now?.spaces.first(where: { $0.kind == "desktop" && !had.contains($0.id) }) else { continue }
        // The agent goes into the new desktop through Mission Control, which
        // animates. A switch made under that animation would race it, so wait
        // for it to land — briefly; the caller switches anyway if it has not.
        let landing = Date().addingTimeInterval(1.5)
        while Date() < landing, displays().first(where: { $0.id == before.id })?.current != new.id {
            usleep(50_000)
        }
        return AddedOut(space: new, refused: nil)
    }
    return AddedOut(space: nil, refused: "no new desktop appeared")
}

// MARK: - main

func emit<T: Encodable>(_ value: T) {
    guard let data = try? JSONEncoder().encode(value) else { fail("could not encode the answer") }
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

let args = Array(CommandLine.arguments.dropFirst())
switch args.first {
case "state":
    emit(StateOut(displays: displays(), windows: windows(), focused: focusedWindow(), active: activeSpace()))
case "switch":
    guard let space = args.dropFirst().first.flatMap(UInt64.init) else { fail("switch <space-id>") }
    exit(switchTo(space) ? 0 : 1)
case "focus":
    guard let window = args.dropFirst().first.flatMap(UInt32.init) else { fail("focus <window-id>") }
    exit(focus(window) ? 0 : 1)
case "add":
    emit(addDesktop())
case "refresh":
    Agent.tell(Agent.refresh)
default:
    fail("usage: slingr-spaces state | switch <space-id> | focus <window-id> | add | refresh")
}
