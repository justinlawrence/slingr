// slingr-agent — what AeroSpace did for slingr, minus managing any windows.
//
// With macOS Spaces as the backend AeroSpace is not running, and two things it
// did for slingr still need something resident: owning the three keys, and
// saying when the Space changes. This does exactly those and decides nothing —
// a key or a change becomes the same `slingr` call AeroSpace used to make.
//
// launchd starts it again if it crashes (agent/dev.slingr.agent.plist.example),
// and its menu-bar item names the task in front, since macOS cannot name a
// Space. A missing name is a missing agent: the daemon that was removed failed
// by dying quietly, and this one cannot die without it showing. For the same
// reason it writes each task's name over its desktop in Mission Control.

import AppKit
import Carbon.HIToolbox

/// The slingr binary built beside this bundle, unless told otherwise.
func slingrBinary() -> URL {
    if let explicit = ProcessInfo.processInfo.environment["SLINGR_BIN"] {
        return URL(fileURLWithPath: explicit)
    }
    return Bundle.main.bundleURL.deletingLastPathComponent().appendingPathComponent("slingr")
}

/// To the millisecond: much of what is logged is how long something took.
let stamp: ISO8601DateFormatter = {
    let format = ISO8601DateFormatter()
    format.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    return format
}()

func log(_ message: String) {
    FileHandle.standardError.write(Data("\(stamp.string(from: Date())) \(message)\n".utf8))
}

/// What slingr-spaces posts to reach the agent. Spelled the same in
/// spaces/main.swift, which a test in src/spaces.rs pins.
enum Names {
    static let addDesktop = Notification.Name("dev.slingr.agent.add-desktop")
    static let refused = Notification.Name("dev.slingr.agent.add-desktop.refused")
    static let switchDesktop = Notification.Name("dev.slingr.agent.switch-desktop")
    static let switchRefused = Notification.Name("dev.slingr.agent.switch-desktop.refused")
    static let refresh = Notification.Name("dev.slingr.agent.refresh")
}

/// The Dock's own "Switch to Desktop N", pressed the way a hand would.
///
/// A switch made through SkyLight leaves the Dock out of it, and the Dock owns
/// the rest of a switch: ShiftPlus found it fires no space-change notification
/// on macOS 15 and later and can leave the old desktop drawn. Its shortcuts are
/// symbolic hotkeys 118 to 133 — Desktop 1 to 16 — read at the moment of
/// pressing, so whatever they are set to is what is pressed.
enum DockShortcut {
    /// Nil once pressed; otherwise why not.
    static func press(desktop n: Int) -> String? {
        guard (1...16).contains(n) else { return "the Dock has shortcuts for Desktops 1 to 16 only" }
        guard AXIsProcessTrusted() else { return "slingr agent needs Accessibility" }
        let domain = "com.apple.symbolichotkeys" as CFString
        CFPreferencesAppSynchronize(domain)
        let hotkeys = CFPreferencesCopyAppValue("AppleSymbolicHotKeys" as CFString, domain) as? [String: Any]
        guard let entry = hotkeys?[String(117 + n)] as? [String: Any],
              (entry["enabled"] as? NSNumber)?.boolValue == true,
              let parameters = (entry["value"] as? [String: Any])?["parameters"] as? [NSNumber],
              parameters.count == 3
        else { return "Switch to Desktop \(n) is not on in Keyboard Shortcuts" }
        // The modifiers are stored with the same bits CGEventFlags uses.
        let key = CGKeyCode(parameters[1].uint16Value)
        let flags = CGEventFlags(rawValue: parameters[2].uint64Value)
        let source = CGEventSource(stateID: .hidSystemState)
        for down in [true, false] {
            guard let event = CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: down) else {
                return "could not make a key event"
            }
            event.flags = flags
            event.post(tap: .cghidEventTap)
        }
        return nil
    }
}

/// Adding a desktop the way a person does: Mission Control, then its +.
///
/// SkyLight leaves making desktops to the Dock, and the Dock offers it only in
/// Mission Control's own interface, so that is what this drives — through
/// Accessibility, as Hammerspoon's `hs.spaces.addSpaceToScreen` does. The
/// elements exist only while Mission Control is showing, so it shows, briefly.
enum MissionControl {
    enum Outcome {
        case added(String)
        case refused(String)
    }

    /// What Mission Control's own key sends the Dock; it toggles. Private, so
    /// looked up rather than linked.
    private static let toggleDock: (@convention(c) (CFString, Int32) -> Void)? = {
        guard let symbol = dlsym(dlopen(nil, RTLD_NOW), "CoreDockSendNotification") else { return nil }
        return unsafeBitCast(symbol, to: (@convention(c) (CFString, Int32) -> Void).self)
    }()

    private static func toggle() {
        toggleDock?("com.apple.expose.awake" as CFString, 0)
    }

    private static func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
        var value: CFTypeRef?
        return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
    }

    private static func children(_ element: AXUIElement) -> [AXUIElement] {
        attribute(element, kAXChildrenAttribute) as? [AXUIElement] ?? []
    }

    private static func identifier(_ element: AXUIElement) -> String {
        attribute(element, "AXIdentifier") as? String ?? ""
    }

    private static func name(_ element: AXUIElement) -> String {
        attribute(element, kAXTitleAttribute) as? String
            ?? attribute(element, kAXDescriptionAttribute) as? String ?? ""
    }

    private static func child(_ element: AXUIElement, _ id: String) -> AXUIElement? {
        children(element).first { identifier($0) == id }
    }

    /// Mission Control's group in the Dock: there only while it is showing.
    private static func showing(_ dock: AXUIElement) -> AXUIElement? {
        child(dock, "mc")
    }

    /// The main screen's strip of desktops.
    private static func strip(_ mc: AXUIElement) -> AXUIElement? {
        let screens = children(mc).filter { identifier($0) == "mc.display" }
        let main = screens.first { (attribute($0, "AXDisplayID") as? NSNumber)?.uint32Value == CGMainDisplayID() }
            ?? (screens.count == 1 ? screens.first : nil)
        return main.flatMap { child($0, "mc.spaces") }
    }

    private static func within<T>(_ seconds: TimeInterval, _ probe: () -> T?) -> T? {
        let deadline = Date().addingTimeInterval(seconds)
        repeat {
            if let found = probe() { return found }
            usleep(50_000)
        } while Date() < deadline
        return nil
    }

    private static func close(_ dock: AXUIElement) {
        if showing(dock) != nil { toggle() }
    }

    /// For the log when Mission Control is not shaped as expected: the next
    /// macOS will move something, and this says what.
    private static func describe(_ element: AXUIElement, depth: Int = 0) -> String {
        let role = attribute(element, kAXRoleAttribute) as? String ?? "?"
        var out = String(repeating: "  ", count: depth) + "\(role) [\(identifier(element))] \(name(element))"
        if depth < 3 {
            for each in children(element) { out += "\n" + describe(each, depth: depth + 1) }
        }
        return out
    }

    static func addDesktop() -> Outcome {
        guard AXIsProcessTrusted() else {
            // Asked now, when it is wanted, rather than at every launch.
            let prompt = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
            _ = AXIsProcessTrustedWithOptions([prompt: true] as CFDictionary)
            return .refused("slingr agent needs Accessibility")
        }
        guard toggleDock != nil else { return .refused("this macOS has no way to open Mission Control") }
        guard let pid = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.dock")
            .first?.processIdentifier
        else { return .refused("the Dock is not running") }
        let dock = AXUIElementCreateApplication(pid)

        if showing(dock) == nil { toggle() }
        guard let mc = within(2, { showing(dock) }) else { return .refused("Mission Control did not open") }
        guard let strip = within(2, { strip(mc) }),
              let add = within(2, { child(strip, "mc.spaces.add") }),
              let list = child(strip, "mc.spaces.list")
        else {
            log("Mission Control, as found:\n\(describe(mc))")
            close(dock)
            return .refused("Mission Control offered no + for the main screen")
        }
        // Hammerspoon found the elements can be there before they answer, and
        // 0.3s was its cure. Measured here: read at once, the desktops had no
        // names yet.
        usleep(300_000)
        let before = children(list)
        let named = before.map(name)
        guard AXUIElementPerformAction(add, kAXPressAction as CFString) == .success else {
            close(dock)
            return .refused("Mission Control would not add a desktop")
        }
        // Measured: the new desktop came last in the row. The name tells it
        // apart as well, in case that ever changes — but only when every
        // desktop already had a name to be told apart from.
        var after: [String] = []
        guard let new = within(2, { () -> AXUIElement? in
            let now = children(child(strip, "mc.spaces.list") ?? list)
            guard now.count > before.count else { return nil }
            after = now.map(name)
            if named.contains("") { return now.last }
            return now.first { !named.contains(name($0)) } ?? now.last
        }) else {
            close(dock)
            return .refused("no new desktop appeared in Mission Control")
        }
        let called = name(new)
        // Going in by pressing it, as a click would, makes the flash and the
        // arrival one movement instead of two.
        if AXUIElementPerformAction(new, kAXPressAction as CFString) != .success { close(dock) }
        if within(1, { showing(dock) == nil ? true : nil }) == nil { close(dock) }
        return .added("\(called): \(named) became \(after)")
    }

    /// The Dock, which draws Mission Control.
    static func dock() -> AXUIElement? {
        NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.dock").first
            .map { AXUIElementCreateApplication($0.processIdentifier) }
    }

    /// One screen's row of desktops along the top of Mission Control, in
    /// screen coordinates measured from the top.
    struct Row {
        let display: CGDirectDisplayID
        /// The row itself.
        let bar: CGRect
        /// Each desktop's, in order. Only their sizes and how far across they
        /// are can be trusted: measured here, the row filled the top 40
        /// points of the screen and its desktops sat 32 points above the top,
        /// as spaceballs found on the Dock's tree, so the row gives the height.
        let desktops: [CGRect]
    }

    /// Nil when Mission Control is not showing.
    static func rows(_ dock: AXUIElement) -> [Row]? {
        guard let mc = showing(dock) else { return nil }
        return children(mc).filter { identifier($0) == "mc.display" }.compactMap { screen in
            guard let id = (attribute(screen, "AXDisplayID") as? NSNumber)?.uint32Value,
                  let list = child(screen, "mc.spaces").flatMap({ child($0, "mc.spaces.list") })
            else { return nil }
            return Row(display: id, bar: frame(list), desktops: children(list).map(frame))
        }
    }

    private static func frame(_ element: AXUIElement) -> CGRect {
        var origin = CGPoint.zero
        var size = CGSize.zero
        if let value = attribute(element, kAXPositionAttribute), CFGetTypeID(value) == AXValueGetTypeID() {
            AXValueGetValue(value as! AXValue, .cgPoint, &origin)
        }
        if let value = attribute(element, kAXSizeAttribute), CFGetTypeID(value) == AXValueGetTypeID() {
            AXValueGetValue(value as! AXValue, .cgSize, &size)
        }
        return CGRect(origin: origin, size: size)
    }
}

/// A desktop's name, drawn where Mission Control writes "Desktop N".
final class NameTag: NSView {
    private let text: NSAttributedString

    init(_ name: String, size: CGFloat) {
        text = NSAttributedString(string: name, attributes: [
            .font: NSFont.systemFont(ofSize: size, weight: .semibold),
            .foregroundColor: NSColor.white,
        ])
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { return nil }

    /// Room for the name, and a little round it.
    var fitting: CGSize {
        let size = text.size()
        return CGSize(width: ceil(size.width) + 18, height: ceil(size.height) + 6)
    }

    override func draw(_: NSRect) {
        let radius = min(bounds.height / 2, 8)
        NSColor.black.withAlphaComponent(0.82).setFill()
        NSBezierPath(roundedRect: bounds, xRadius: radius, yRadius: radius).fill()
        let size = text.size()
        text.draw(at: NSPoint(x: (bounds.width - size.width) / 2, y: (bounds.height - size.height) / 2))
    }
}

/// Task names over Mission Control's desktops, since macOS will not let a
/// desktop be renamed — and Mission Control is where a window is dragged to
/// another desktop, so where the name is wanted.
///
/// The names are the agent's own windows, laid over the row of desktops at
/// the places the Dock's Accessibility tree gives: what DeskStag sells, and
/// what CloseUp does for window controls. CloseUp's notes (oomol-lab/CloseUp,
/// AGENTS.md) are where the rest comes from, all for the Dock-drawn Mission
/// Control of macOS 26 and earlier:
///
/// - The Dock tells an AXObserver when Mission Control opens, but stops after
///   a switch into a full-screen window's Space, and a restarted Dock has a
///   new pid; so the observer is made afresh on every Space change.
/// - It also says Mission Control exited during trackpad swipes that leave it
///   open, so that only takes the names down. Whether it has really closed is
///   read from the tree.
/// - A window put up while Mission Control is still moving things sinks below
///   what it draws next, whatever its level. So the names go up only once the
///   row has held still, in fresh windows each time.
/// - Screen-saver level, and not `.transient`, which Mission Control hides.
final class DesktopNames {
    struct Labels: Decodable {
        let bars: [Bar]
    }

    struct Bar: Decodable {
        let display: String
        let main: Bool
        let names: [String?]
    }

    /// A name, and where it goes.
    struct Spot: Equatable {
        let name: String
        let centre: CGPoint
        /// Mission Control's own "Desktop N", which a name covers while the
        /// row is collapsed. Nothing once it has opened into pictures: a name
        /// sits on the picture.
        let cover: CGSize
    }

    private static let opened = ["AXExposeShowAllWindows", "AXExposeShowFrontWindows", "AXExposeShowDesktop"]
    private static let exited = "AXExposeExit"

    /// `slingr labels`, run.
    private let labels: () -> String

    // The main thread's.
    private var observer: AXObserver?
    private var watched: pid_t = 0
    private var tags: [NSWindow] = []

    // `queue`'s. The Dock is read off the main thread because it is asked
    // just as it is busiest, drawing Mission Control, and the keys must keep
    // working meanwhile.
    private let queue = DispatchQueue(label: "dev.slingr.agent.names")
    private var timer: DispatchSourceTimer?
    private var dock: AXUIElement?
    private var because = ""
    private var began = Date()
    private var ticks = 0
    private var bars: [Bar] = []
    private var seen = false
    private var misses = 0
    private var last: [Spot] = []
    private var still = 0
    private var shown: [Spot] = []
    private var shows = 0
    private var told = false
    /// The rows as last read, for the log.
    private var layout = ""

    init(labels: @escaping () -> String) {
        self.labels = labels
    }

    /// Listen for Mission Control opening, afresh.
    func arm() {
        guard AXIsProcessTrusted(),
              let pid = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.dock")
                  .first?.processIdentifier
        else { return }
        if let old = observer {
            CFRunLoopRemoveSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(old), .defaultMode)
            observer = nil
        }
        var made: AXObserver?
        let callback: AXObserverCallback = { _, _, notification, context in
            guard let context else { return }
            Unmanaged<DesktopNames>.fromOpaque(context).takeUnretainedValue().noticed(notification as String)
        }
        guard AXObserverCreate(pid, callback, &made) == .success, let made else {
            log("could not watch the Dock for Mission Control")
            return
        }
        let dock = AXUIElementCreateApplication(pid)
        let context = Unmanaged.passUnretained(self).toOpaque()
        let refused = (Self.opened + [Self.exited]).filter {
            AXObserverAddNotification(made, dock, $0 as CFString, context) != .success
        }
        CFRunLoopAddSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(made), .defaultMode)
        observer = made
        if pid != watched || !refused.isEmpty {
            log("watching the Dock (pid \(pid)) for Mission Control" + (refused.isEmpty ? "" : "; it refused \(refused)"))
        }
        watched = pid
    }

    /// Mission Control redraws itself on a switch, over anything already up.
    func spaceChanged() {
        arm()
        queue.async { [self] in unshow() }
    }

    private func noticed(_ notification: String) {
        queue.async { [self] in
            if notification == Self.exited {
                unshow()
            } else {
                begin(because: notification)
            }
        }
    }

    private func begin(because notification: String) {
        guard timer == nil else { return }
        because = notification
        began = Date()
        log("Mission Control opened (\(notification))")
        dock = MissionControl.dock()
        ticks = 0
        bars = read()
        seen = false
        misses = 0
        last = []
        still = 0
        shown = []
        shows = 0
        told = false
        layout = ""
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now(), repeating: .milliseconds(100))
        timer.setEventHandler { [weak self] in self?.tick() }
        timer.resume()
        self.timer = timer
    }

    private func read() -> [Bar] {
        (try? JSONDecoder().decode(Labels.self, from: Data(labels().utf8)))?.bars ?? []
    }

    private func tick() {
        ticks += 1
        // Again each second, for a desktop dragged to a new place in the row:
        // names are matched to desktops by position.
        if ticks % 10 == 0 { bars = read() }
        guard let dock, let found = MissionControl.rows(dock) else {
            // Not showing: not yet, or no longer.
            if seen { misses += 1 }
            if seen ? misses >= 2 : Date().timeIntervalSince(began) > 2 { end() }
            return
        }
        seen = true
        misses = 0
        let spots = place(found)
        if spots != last {
            last = spots
            unshow()
            return
        }
        still += 1
        if still >= 2, shown != spots {
            shown = spots
            shows += 1
            show(spots)
        }
    }

    private func place(_ rows: [MissionControl.Row]) -> [Spot] {
        var spots: [Spot] = []
        var described: [String] = []
        for row in rows {
            let uuid = CGDisplayCreateUUIDFromDisplayID(row.display)
                .map { CFUUIDCreateString(nil, $0.takeRetainedValue()) as String } ?? ""
            guard let bar = bars.first(where: { $0.display == uuid || ($0.main && row.display == CGMainDisplayID()) })
            else { continue }
            // Matched by position, so a row of another length is not the row
            // slingr described, and a wrong name is worse than none.
            guard bar.names.count == row.desktops.count else {
                if !told {
                    told = true
                    log("Mission Control shows \(row.desktops.count) Spaces on a screen where slingr knows \(bar.names.count); not naming them")
                }
                continue
            }
            described.append("row \(Self.text(row.bar)), first desktop \(Self.text(row.desktops.first ?? .zero))")
            // Measured: closing, the row slides up off the screen before it
            // goes, and a name there would be drawn where nothing is.
            if !row.bar.isEmpty, row.bar.maxY <= CGDisplayBounds(row.display).minY { continue }
            let middle = row.bar.isEmpty ? nil : row.bar.midY
            for (name, frame) in zip(bar.names, row.desktops) where !frame.isEmpty {
                guard let name else { continue }
                // A "Desktop N" label is a line of text, which a name covers;
                // anything taller is a picture of the desktop, which it sits on.
                let label = frame.height < 40
                spots.append(Spot(
                    name: name,
                    centre: CGPoint(x: frame.midX, y: middle ?? frame.midY),
                    cover: label ? frame.size : .zero
                ))
            }
        }
        layout = described.joined(separator: "; ")
        return spots
    }

    private static func text(_ rect: CGRect) -> String {
        "\(Int(rect.minX)),\(Int(rect.minY)) \(Int(rect.width))x\(Int(rect.height))"
    }

    private func show(_ spots: [Spot]) {
        let ms = Int(Date().timeIntervalSince(began) * 1000)
        let places = spots.map { "\($0.name) at \(Int($0.centre.x)),\(Int($0.centre.y))" }
        log("names over Mission Control, \(ms)ms after \(because): \(layout); \(places.joined(separator: ", "))")
        DispatchQueue.main.async { [self] in
            retire()
            // The Dock measures from the top of the main screen, a window
            // from the bottom.
            let top = CGDisplayBounds(CGMainDisplayID()).height
            tags = spots.map { tag($0, top: top) }
        }
    }

    private func unshow() {
        still = 0
        guard !shown.isEmpty else { return }
        shown = []
        DispatchQueue.main.async { [self] in retire() }
    }

    private func end() {
        timer?.cancel()
        timer = nil
        let ms = Int(Date().timeIntervalSince(began) * 1000)
        if shows > 0 {
            log("Mission Control closed, \(ms)ms after \(because)")
        } else if !seen {
            log("no names: Mission Control never appeared in the Dock's tree in \(ms)ms")
        } else if bars.isEmpty {
            log("no names: slingr labels gave none")
        } else if last.isEmpty {
            log("no names: no desktop showing is a task's (\(layout))")
        } else {
            log("no names: the row never held still in \(ms)ms (\(layout))")
        }
        unshow()
    }

    private func tag(_ spot: Spot, top: CGFloat) -> NSWindow {
        let view = NameTag(spot.name, size: spot.cover == .zero ? 15 : 12)
        var size = view.fitting
        size.width = max(size.width, spot.cover.width)
        size.height = max(size.height, spot.cover.height)
        let frame = NSRect(
            x: spot.centre.x - size.width / 2,
            y: top - spot.centre.y - size.height / 2,
            width: size.width, height: size.height
        )
        let window = NSPanel(
            contentRect: frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false
        )
        window.isReleasedWhenClosed = false
        window.level = .screenSaver
        window.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .ignoresCycle]
        window.ignoresMouseEvents = true
        window.isOpaque = false
        window.backgroundColor = .clear
        window.hasShadow = false
        window.hidesOnDeactivate = false
        window.contentView = view
        window.orderFrontRegardless()
        return window
    }

    private func retire() {
        for window in tags { window.orderOut(nil) }
        tags = []
    }
}

final class Agent: NSObject, NSApplicationDelegate {
    private let slingr = slingrBinary()
    private var item: NSStatusItem!
    private var hotKeys: [EventHotKeyRef] = []
    private var actions: [UInt32: [String]] = [:]
    /// Children still running, held so they are not released mid-flight.
    private var running: Set<Process> = []
    private var names: DesktopNames!

    /// One `follow` at a time, and one more if the Space moved again while it
    /// ran — so the last change gets the last word, as `goto` does for taps.
    private let following = DispatchQueue(label: "dev.slingr.agent.follow")
    private var inFlight = false
    private var again = false

    func applicationDidFinishLaunching(_: Notification) {
        item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        item.button?.title = "◈"
        item.menu = menu()

        bind(kVK_ANSI_S, id: 1, [])
        bind(kVK_ANSI_I, id: 2, ["jump"])
        bind(kVK_ANSI_B, id: 3, ["board"])
        installHotKeyHandler()

        NSWorkspace.shared.notificationCenter.addObserver(
            self, selector: #selector(spaceChanged),
            name: NSWorkspace.activeSpaceDidChangeNotification, object: nil
        )
        let distributed = DistributedNotificationCenter.default()
        distributed.addObserver(self, selector: #selector(addDesktop), name: Names.addDesktop, object: nil)
        distributed.addObserver(self, selector: #selector(switchDesktop(_:)), name: Names.switchDesktop, object: nil)
        distributed.addObserver(self, selector: #selector(refreshRequested), name: Names.refresh, object: nil)

        // Titles on the board need Screen Recording, and asking is the only
        // way to be listed in System Settings at all. macOS asks once.
        if !CGPreflightScreenCaptureAccess() {
            CGRequestScreenCaptureAccess()
        }
        names = DesktopNames { [weak self] in self?.runAndWait(["labels"]) ?? "" }
        names.arm()
        refresh()
        let recording = CGPreflightScreenCaptureAccess() ? "allowed" : "not allowed"
        log("started; slingr is \(slingr.path); screen recording \(recording)")
    }

    private func menu() -> NSMenu {
        let menu = NSMenu()
        menu.addItem(withTitle: "Jump to a task   ⌃⌥⌘I", action: #selector(jump), keyEquivalent: "").target = self
        menu.addItem(withTitle: "Board   ⌃⌥⌘B", action: #selector(board), keyEquivalent: "").target = self
        menu.addItem(.separator())
        // Accessibility is what adding a desktop for a task takes. It also
        // lets going to a window raise that window, rather than bringing its
        // application forward to pick whichever of its windows it likes.
        menu.addItem(withTitle: "Allow Accessibility…", action: #selector(askAccessibility), keyEquivalent: "")
            .target = self
        menu.addItem(.separator())
        menu.addItem(withTitle: "Quit slingr agent", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "")
        return menu
    }

    // MARK: keys

    private func bind(_ keyCode: Int, id: UInt32, _ args: [String]) {
        var ref: EventHotKeyRef?
        let status = RegisterEventHotKey(
            UInt32(keyCode), UInt32(controlKey | optionKey | cmdKey),
            EventHotKeyID(signature: OSType(0x736C_6E67), id: id), // 'slng'
            GetApplicationEventTarget(), 0, &ref
        )
        guard status == noErr, let ref else {
            // AeroSpace still running is the usual reason.
            log("ctrl-alt-cmd key \(id) could not be registered (\(status))")
            return
        }
        hotKeys.append(ref)
        actions[id] = args
    }

    private func installHotKeyHandler() {
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        InstallEventHandler(GetApplicationEventTarget(), { _, event, context in
            var id = EventHotKeyID()
            GetEventParameter(
                event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID),
                nil, MemoryLayout<EventHotKeyID>.size, nil, &id
            )
            let agent = Unmanaged<Agent>.fromOpaque(context!).takeUnretainedValue()
            agent.pressed(id.id)
            return noErr
        }, 1, &spec, Unmanaged.passUnretained(self).toOpaque(), nil)
    }

    private func pressed(_ id: UInt32) {
        guard let args = actions[id] else { return }
        launch(args)
    }

    @objc private func jump() { launch(["jump"]) }
    @objc private func board() { launch(["board"]) }

    @objc private func askAccessibility() {
        let prompt = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
        _ = AXIsProcessTrustedWithOptions([prompt: true] as CFDictionary)
    }

    /// A key opens the panel, which waits for an answer for as long as you
    /// take. Nothing waits on it here.
    private func launch(_ args: [String]) {
        let process = Process()
        process.executableURL = slingr
        process.arguments = args
        process.terminationHandler = { [weak self] done in
            DispatchQueue.main.async { self?.running.remove(done) }
        }
        do {
            try process.run()
            running.insert(process)
        } catch {
            log("could not run \(slingr.path) \(args): \(error)")
        }
    }

    // MARK: desktops

    /// One at a time: two at once would each open and close Mission Control
    /// under the other.
    private let adding = DispatchQueue(label: "dev.slingr.agent.add")

    /// Off the main thread: it takes as long as Mission Control does, and the
    /// keys must keep working meanwhile.
    @objc private func addDesktop() {
        adding.async {
            let began = Date()
            let outcome = MissionControl.addDesktop()
            let ms = Int(Date().timeIntervalSince(began) * 1000)
            switch outcome {
            case .added(let what):
                log("added a desktop in \(ms)ms: \(what)")
            case .refused(let why):
                log("no desktop added (\(ms)ms): \(why)")
                // So slingr can say why at once, rather than wait out its
                // patience for a desktop that is not coming.
                DistributedNotificationCenter.default()
                    .postNotificationName(Names.refused, object: why, userInfo: nil, deliverImmediately: true)
            }
        }
    }

    /// slingr asking for Desktop N. A request names a desktop, never a key:
    /// anything can post one, and the agent's Accessibility must not become a
    /// way for anything to type. So the only keys ever pressed are the Dock's
    /// own shortcut for that desktop.
    @objc private func switchDesktop(_ note: Notification) {
        guard let n = (note.object as? String).flatMap(Int.init) else { return }
        if let why = DockShortcut.press(desktop: n) {
            log("did not press Desktop \(n): \(why)")
            DistributedNotificationCenter.default()
                .postNotificationName(Names.switchRefused, object: why, userInfo: nil, deliverImmediately: true)
        } else {
            log("pressed Desktop \(n)")
        }
    }

    /// slingr changed which task a desktop is for — `bind`, or a claim — and
    /// nothing about the Space changed to say so.
    @objc private func refreshRequested() {
        refresh()
    }

    // MARK: Spaces

    @objc private func spaceChanged() {
        names.spaceChanged()
        if inFlight {
            again = true
            return
        }
        inFlight = true
        again = false
        following.async { [self] in
            _ = runAndWait(["follow"])
            let here = runAndWait(["here"])
            DispatchQueue.main.async { [self] in
                show(here)
                inFlight = false
                if again { spaceChanged() }
            }
        }
    }

    private func refresh() {
        following.async { [self] in
            let here = runAndWait(["here"])
            DispatchQueue.main.async { [self] in show(here) }
        }
    }

    private func show(_ task: String) {
        let name = task.trimmingCharacters(in: .whitespacesAndNewlines)
        item.button?.title = name.isEmpty ? "◈" : "◈ \(name)"
    }

    private func runAndWait(_ args: [String]) -> String {
        let process = Process()
        process.executableURL = slingr
        process.arguments = args
        let out = Pipe()
        process.standardOutput = out
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
        } catch {
            log("could not run \(slingr.path) \(args): \(error)")
            return ""
        }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return String(decoding: data, as: UTF8.self)
    }
}

/// launchd's name for the agent: the plist's Label, and what spaces.sh boots.
let label = "dev.slingr.agent"

/// A copy launchd did not start hands over to launchd's job and steps aside.
///
/// Measured, on allowing Screen Recording: the agent launchd started quit
/// cleanly — so launchd left it stopped — and three seconds later the app was
/// open again outside the job, logging nowhere, with nothing to start it again
/// if it crashed: the daemon that quietly dies, all over again. Opening the
/// app by hand would do the same. kickstart starts the job if it is stopped
/// and leaves it be if it is running; with no job installed, this copy carries
/// on as the agent.
func handedToLaunchd() -> Bool {
    // launchd names its job's process by Label; LaunchServices names one it
    // opened `application.<bundle id>.<numbers>`.
    if ProcessInfo.processInfo.environment["XPC_SERVICE_NAME"] == label { return false }
    let kick = Process()
    kick.executableURL = URL(fileURLWithPath: "/bin/launchctl")
    kick.arguments = ["kickstart", "gui/\(getuid())/\(label)"]
    kick.standardOutput = FileHandle.nullDevice
    kick.standardError = FileHandle.nullDevice
    guard (try? kick.run()) != nil else { return false }
    kick.waitUntilExit()
    return kick.terminationStatus == 0
}

// Before anything is registered, so the keys are free for launchd's copy.
if handedToLaunchd() { exit(0) }

let app = NSApplication.shared
let agent = Agent()
app.delegate = agent
app.setActivationPolicy(.accessory)
app.run()
