// slingr-panel — the picker, as a floating panel.
//
// Presentation only. Every decision belongs to the Rust side: this reads a
// list on stdin, shows it, and prints what was chosen. That keeps the logic
// under test and the window code dumb.
//
// It is an NSPanel rather than a window on purpose. AeroSpace does not manage
// panels, so this belongs to no workspace, is never hidden, never moved, and
// cannot be slung by accident — unlike the System Events dialog it replaces.

import AppKit
import SwiftUI

// MARK: - what Rust sends

struct Item: Decodable, Identifiable, Hashable {
    let id: String
    let label: String
    var section: String?
    var count: Int?
    /// "here" marks where the window is now; "action" is a row that does
    /// something rather than naming a task.
    var marker: String?
    var pinned: Bool?
    var detail: String?
    var active: Bool?
    var bundle: String?
    /// Which applications a task holds, as bundle ids. Rust chooses them and
    /// their order — see `occupancy_from` — so this only has to draw them.
    var stack: [String]?
}

/// Application icons, looked up once each. The icon says which application a
/// window belongs to far faster than its name does, and leaves the row free to
/// say which *window* it is.
@MainActor
enum Icons {
    private static var cache: [String: NSImage?] = [:]

    static func forBundle(_ id: String?) -> NSImage? {
        guard let id, !id.isEmpty else { return nil }
        if let hit = cache[id] { return hit }
        let icon = NSRunningApplication.runningApplications(withBundleIdentifier: id)
            .first?.icon
            ?? NSWorkspace.shared.urlForApplication(withBundleIdentifier: id)
                .map { NSWorkspace.shared.icon(forFile: $0.path) }
        cache[id] = icon
        return icon
    }
}

struct Palette: Decodable {
    var background = "#11111b"
    var foreground = "#cdd6f4"
    var accent = "#89b4fa"
    var ok = "#a6e3a1"
    var warn = "#fab387"
    var dim = "#6c7086"
}

struct Request: Decodable {
    let title: String
    var subtitle: String?
    var multi: Bool?
    var placeholder: String?
    var theme: Palette?
    var pinnedFolders: [String]?
    /// Which surface to draw. "board" is a different shape of answer — several
    /// windows sent to several tasks — not a differently-styled list.
    var layout: String?
    let items: [Item]
}

// MARK: - palette
//
// Taken from the terminal rather than invented, so the panel belongs to the
// same world as the windows it appears over. Flat and high contrast, not the
// usual macOS translucency: this has to read at a glance over tiled windows.

extension Color {
    init(hex: String, fallback: Color = .gray) {
        var s = hex.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("#") { s.removeFirst() }
        guard s.count == 6, let v = UInt32(s, radix: 16) else { self = fallback; return }
        self.init(
            .sRGB,
            red: Double((v >> 16) & 0xff) / 255,
            green: Double((v >> 8) & 0xff) / 255,
            blue: Double(v & 0xff) / 255,
            opacity: 1
        )
    }
}

struct Ink {
    let base: Color, raised: Color, selected: Color, edge: Color
    let text: Color, dim: Color, accent: Color, here: Color, pin: Color
    /// Kept apart from the accent: a warning should not look like a choice.
    var warn: Color { pin }

    init(_ p: Palette?) {
        let p = p ?? Palette()
        base = Color(hex: p.background)
        text = Color(hex: p.foreground)
        dim = Color(hex: p.dim)
        accent = Color(hex: p.accent)
        here = Color(hex: p.ok)
        pin = Color(hex: p.warn)
        // Washes of the foreground over the background, rather than colours of
        // their own, so every theme keeps its own character.
        raised = Color(hex: p.foreground).opacity(0.05)
        selected = Color(hex: p.foreground).opacity(0.11)
        edge = Color(hex: p.foreground).opacity(0.14)
    }
}

/// Window-behind vibrancy, so the panel sits in the light of the desktop it
/// is over rather than painting a flat rectangle on top of it.
///
/// The palette stays the terminal's — this only decides how much of what is
/// underneath comes through. The flat original was chosen so the panel would
/// read at a glance over tiled windows, and that still governs: the theme
/// background is laid over the blur at nearly full strength, so the blur reads
/// as depth at the edges without ever costing contrast in the rows.
struct Vibrancy: NSViewRepresentable {
    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = .hudWindow
        view.blendingMode = .behindWindow
        view.state = .active
        return view
    }

    func updateNSView(_: NSVisualEffectView, context _: Context) {}
}

/// Set once from the request before any view exists, then only read.
nonisolated(unsafe) var ink = Ink(nil)

/// Every size in one place, scaled together. Bumping one font and not the
/// rest is how a panel stops looking like itself.
struct Type {
    let scale: Double
    /// The system face, at the system's own sizes. The panel used to be set in
    /// the terminal's monospace throughout, which made it look like output;
    /// task names are names, and read faster in the face macOS sets names in.
    private func sf(_ size: Double, _ weight: Font.Weight = .regular) -> Font {
        .system(size: size * scale, weight: weight)
    }
    var body: Font { sf(13) }
    var small: Font { sf(11) }
    /// The wordmark stays monospaced. Everything slingr does it does for a
    /// terminal, and the one place that lineage belongs is its own name.
    var title: Font { .system(size: 16 * scale, weight: .bold, design: .monospaced) }
    var tab: Font { sf(13, .medium) }
    var rowName: Font { sf(14, .medium) }
    /// Tabular, so counts in a column line up on their digits rather than
    /// drifting with the width of a 1.
    var bigNumber: Font { .system(size: 19 * scale, weight: .semibold).monospacedDigit() }
    /// Keys stay monospaced: ⌘p is a thing you press, not a word you read.
    var key: Font { .system(size: 11 * scale, weight: .medium, design: .monospaced) }
}

/// The reader's preference, not the program's state, so it lives where macOS
/// keeps preferences rather than in sling's own files.
enum Zoom {
    static let key = "fontScale"
    static let steps: [Double] = [0.8, 0.9, 1.0, 1.15, 1.3, 1.5, 1.75]

    static func load() -> Double {
        let stored = UserDefaults.standard.double(forKey: key)
        return steps.contains(stored) ? stored : 1.0
    }

    static func save(_ value: Double) {
        UserDefaults.standard.set(value, forKey: key)
    }

    static func stepped(from current: Double, by delta: Int) -> Double {
        let at = steps.firstIndex(of: current) ?? steps.firstIndex(of: 1.0)!
        return steps[max(0, min(steps.count - 1, at + delta))]
    }
}

// MARK: - state

@MainActor
final class Picker: ObservableObject {
    @Published var query = ""
    @Published var cursor = 0
    @Published var chosen: Set<String> = []
    @Published var scale = Zoom.load()

    /// Told to the controller so the window can be refitted around the text.
    var onResize: (() -> Void)?

    func zoom(_ delta: Int) {
        let next = delta == 0 ? 1.0 : Zoom.stepped(from: scale, by: delta)
        guard next != scale else { return }
        scale = next
        Zoom.save(next)
        onResize?()
    }

    let request: Request
    var multi: Bool { request.multi ?? false }
    var isBoard: Bool { request.layout == "board" }

    /// The task showing now, so the board can ring its tile. Carried in the
    /// subtitle, which the board has no other use for.
    var here: String { request.subtitle ?? "" }

    /// Windows dragged somewhere, not yet slung.
    ///
    /// Kept here rather than answered one at a time because tidying is several
    /// moves at once: answering each drop would close the panel, and sorting
    /// forty windows would become forty keypresses.
    @Published var pending: [String: String] = [:]
    /// Set by the controller; the view has no business exiting the process.
    var onConfirm: (([String]) -> Void)?

    init(_ request: Request) { self.request = request }

    /// The modes, drawn as tabs rather than listed as rows.
    var tabs: [Item] { request.items.filter { $0.marker == "tab" } }

    /// Rows that survive the query. Tabs and the new-workspace action are
    /// drawn in the header, so they never appear among them.
    var visible: [Item] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        return request.items.filter { item in
            guard item.marker != "tab", item.marker != "subject" else { return false }
            guard item.id != newAction?.id else { return false }
            guard !q.isEmpty else { return true }
            if item.marker == "action" { return false }
            return matches(q, item.label.lowercased())
                || matches(q, (item.detail ?? "").lowercased())
        }
    }

    /// What the typed text would be called as a task.
    ///
    /// The rule is AeroSpace's: a "/" hangs it on a modal dialog, so herdr's
    /// "w/forms" becomes "w-forms". Mirrored here only to show the name before
    /// it is made — `picker::sanitise_workspace` on the Rust side decides it,
    /// and a test there pins the rule.
    var typedTaskName: String {
        var out = ""
        var prevDash = false
        for ch in query.trimmingCharacters(in: .whitespaces) {
            if ch.isASCII && (ch.isLetter || ch.isNumber || ch == "." || ch == "_" || ch == "-") {
                out.append(ch)
                prevDash = ch == "-"
            } else if !prevDash {
                out.append("-")
                prevDash = true
            }
        }
        while out.hasPrefix("-") { out.removeFirst() }
        while out.hasSuffix("-") { out.removeLast() }
        return out
    }

    /// Offer to make it only when nothing already answers to that name.
    var canMakeTyped: Bool {
        let name = typedTaskName
        guard !name.isEmpty else { return false }
        return !request.items.contains { $0.marker == nil && $0.id == name }
    }

    /// The next tab round, for the keyboard shortcut. Cycling rather than
    /// flipping, because there are more than two of them now and ⇥ landing on
    /// the same one every time is not a cycle.
    var otherTab: Item? {
        let all = tabs
        guard !all.isEmpty else { return nil }
        let showing = all.firstIndex { $0.active == true } ?? -1
        return all[(showing + 1) % all.count]
    }

    /// The row that creates a workspace, lifted out of the list and onto the
    /// tab line where it reads as an action rather than a destination.
    var newAction: Item? { request.items.first { $0.id.hasPrefix("＋") || $0.label.hasPrefix("＋") } }

    /// What is being slung, shown at the top rather than squeezed into a
    /// corner: it is the subject of the whole question.
    var subject: Item? { request.items.first { $0.marker == "subject" } }

    var pinnedFolders: Set<String> { Set(request.pinnedFolders ?? []) }

    /// Subsequence match, so "tgw" finds "t-gant-workload".
    private func matches(_ needle: String, _ haystack: String) -> Bool {
        if haystack.contains(needle) { return true }
        var i = needle.startIndex
        for ch in haystack where ch == needle[i] {
            i = needle.index(after: i)
            if i == needle.endIndex { return true }
        }
        return false
    }

    func move(_ delta: Int) {
        let rows = visible
        guard !rows.isEmpty else { return }
        cursor = max(0, min(rows.count - 1, cursor + delta))
    }

    /// Take or drop a whole folder at once. Selecting twenty windows one at a
    /// time is not sorting, it is data entry.
    func toggleFolder(_ name: String) {
        guard multi else { return }
        let ids = visible.filter { $0.section == name && $0.marker != "action" }.map(\.id)
        guard !ids.isEmpty else { return }
        if ids.allSatisfy(chosen.contains) {
            ids.forEach { chosen.remove($0) }
        } else {
            ids.forEach { chosen.insert($0) }
        }
    }

    func folderState(_ name: String) -> (all: Bool, some: Bool) {
        let ids = visible.filter { $0.section == name && $0.marker != "action" }.map(\.id)
        guard !ids.isEmpty else { return (false, false) }
        let taken = ids.filter(chosen.contains).count
        return (taken == ids.count, taken > 0)
    }

    func toggle() {
        guard multi, let item = visible[safe: cursor], item.marker != "action" else { return }
        if chosen.contains(item.id) { chosen.remove(item.id) } else { chosen.insert(item.id) }
        move(1)
    }

    /// The heading to draw above the row at `index`, if it starts a group.
    func headingBefore(_ index: Int) -> String? {
        let rows = visible
        guard let section = rows[safe: index]?.section else { return nil }
        if index == 0 { return section }
        return rows[safe: index - 1]?.section == section ? nil : section
    }

    /// A click picks, the way a click should. In multi mode it ticks instead,
    /// since confirming there means confirming the whole set.
    func click(_ index: Int) {
        cursor = index
        if multi {
            toggle()
        } else {
            onConfirm?(confirm())
        }
    }

    /// Where a window started, before anything was dragged.
    private func home(of windowId: String) -> String {
        request.items.first { $0.id == windowId }?.section ?? ""
    }

    /// The tiles, in the order Rust listed them — the task you are on first,
    /// then the rest — with each window shown wherever it has been dragged to.
    var boardGroups: [(task: String, windows: [Item])] {
        var order: [String] = []
        var seen = Set<String>()
        for item in request.items {
            guard let section = item.section,
                  item.marker == nil || item.marker == "space" else { continue }
            if seen.insert(section).inserted { order.append(section) }
        }

        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        var byTask: [String: [Item]] = [:]
        for item in request.items where item.marker == nil {
            guard let section = item.section else { continue }
            let task = pending[item.id] ?? section
            guard q.isEmpty
                || matches(q, item.label.lowercased())
                || matches(q, task.lowercased()) else { continue }
            byTask[task, default: []].append(item)
        }

        let groups = order.map { (task: $0, windows: byTask[$0] ?? []) }
        // With nothing typed every task keeps its tile, empty ones included:
        // an empty tile is where you tidy *to*. Once filtering, a tile that
        // matched nothing is noise.
        guard !q.isEmpty else { return groups }
        return groups.filter { !$0.windows.isEmpty || matches(q, $0.task.lowercased()) }
    }

    /// A window let go over a task. Dropping it back where it came from is how
    /// you undo, rather than a move to the place it already is.
    func drop(_ windowId: String, on task: String) {
        guard request.items.contains(where: { $0.id == windowId && $0.marker == nil }) else { return }
        if home(of: windowId) == task {
            pending.removeValue(forKey: windowId)
        } else {
            pending[windowId] = task
        }
    }

    var pendingCount: Int { pending.count }

    /// One line per window that has actually been moved somewhere else.
    func boardAnswer() -> [String] {
        pending.map { "__sling__:\($0.key):\($0.value)" }.sorted()
    }

    /// Going somewhere is only offered while nothing is waiting to move —
    /// otherwise one click would throw away a tidy-up half done.
    func goTo(_ id: String) {
        guard pending.isEmpty else { return }
        onConfirm?([id])
    }

    func confirm() -> [String] {
        // The board answers with everything dragged, in one go.
        if isBoard { return pending.isEmpty ? [] : boardAnswer() }
        // Typed a name nothing answers to and pressed return: that is a task
        // you meant to make, not a search that failed.
        if visible.isEmpty && canMakeTyped {
            return ["__new__:" + query]
        }
        if multi, !chosen.isEmpty {
            // Keep the order they were listed in, not the order they were ticked.
            return request.items.map(\.id).filter { chosen.contains($0) }
        }
        guard let item = visible[safe: cursor] else { return [] }
        return [item.id]
    }
}

extension Array {
    subscript(safe index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}

// MARK: - view

struct PanelView: View {
    @ObservedObject var picker: Picker

    private var type: Type { Type(scale: picker.scale) }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            list
            footer
        }
        .background {
            ZStack {
                Vibrancy()
                ink.base.opacity(0.86)
            }
        }
        // `.continuous` is the macOS squircle rather than a plain arc — the
        // difference is small and it is most of what makes a rounded corner
        // look like the system drew it.
        .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 13, style: .continuous)
                .stroke(ink.edge, lineWidth: 1)
        )
    }

    // A sentence where a label would be, and a title with room to breathe.
    private var header: some View {
        VStack(spacing: 12) {
            HStack(spacing: 8) {
                Text("slingr").font(type.title).tracking(5).foregroundStyle(ink.text)
                Text("◈").font(type.rowName).foregroundStyle(ink.accent)
                Spacer()
            }
            if let subject = picker.subject {
                HStack(spacing: 9) {
                    if let icon = Icons.forBundle(subject.bundle) {
                        Image(nsImage: icon)
                            .resizable()
                            .frame(width: 20 * picker.scale, height: 20 * picker.scale)
                    }
                    Text(subject.label).font(type.rowName).foregroundStyle(ink.text)
                    if let title = subject.detail, !title.isEmpty {
                        Text(title)
                            .font(type.body)
                            .foregroundStyle(ink.dim)
                            .lineLimit(1)
                            .truncationMode(.tail)
                    }
                    Spacer(minLength: 0)
                }
            }
            if let note = picker.request.subtitle, !note.isEmpty {
                HStack {
                    Text(note).font(type.small).foregroundStyle(ink.warn)
                    Spacer()
                }
            }
            tabStrip

            HStack(spacing: 8) {
                Text("❯").font(type.body).foregroundStyle(ink.accent)
                ZStack(alignment: .leading) {
                    if picker.query.isEmpty {
                        Text(picker.request.placeholder ?? "type to filter")
                            .font(type.body).foregroundStyle(ink.dim.opacity(0.7))
                    }
                    Text(picker.query).font(type.body).foregroundStyle(ink.text)
                }
                Spacer(minLength: 8)
                // A name nothing answers to is a task you have not made yet.
                if picker.canMakeTyped {
                    HStack(spacing: 6) {
                        Text("＋").foregroundStyle(ink.accent)
                        Text(picker.typedTaskName).foregroundStyle(ink.text)
                        if picker.visible.isEmpty {
                            Text("⏎").foregroundStyle(ink.dim)
                        }
                    }
                    .font(type.small)
                    .padding(.horizontal, 9)
                    .padding(.vertical, 5)
                    .overlay(RoundedRectangle(cornerRadius: 6).stroke(ink.accent.opacity(0.5), lineWidth: 1))
                    .contentShape(Rectangle())
                    .onTapGesture { picker.onConfirm?(["__new__:" + picker.query]) }
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 9)
            .background(ink.raised)
            .clipShape(RoundedRectangle(cornerRadius: 7))
        }
        .frame(maxWidth: .infinity)
        .padding(.horizontal, 16)
        .padding(.top, 18)
        .padding(.bottom, 14)
    }

    /// Two tabs, because there are exactly two modes and a row pretending to
    /// be a mode was never honest about it.
    private var tabStrip: some View {
        HStack(spacing: 4) {
            ForEach(picker.tabs) { tab in
                let on = tab.active == true
                Text(tab.label)
                    .font(type.tab)
                    .tracking(1.5)
                    .foregroundStyle(on ? ink.text : ink.dim)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 7)
                    .background(on ? ink.selected : Color.clear)
                    .clipShape(RoundedRectangle(cornerRadius: 7))
                    .overlay(alignment: .bottom) {
                        Rectangle()
                            .fill(on ? ink.accent : .clear)
                            .frame(height: 2)
                            .padding(.horizontal, 12)
                    }
                    .contentShape(Rectangle())
                    .onTapGesture { if !on { picker.onConfirm?([tab.id]) } }
            }
            Spacer()
            if let make = picker.newAction {
                Text(make.label)
                    .font(type.tab)
                    .foregroundStyle(ink.accent)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 7)
                    .overlay(RoundedRectangle(cornerRadius: 7).stroke(ink.edge, lineWidth: 1))
                    .contentShape(Rectangle())
                    .onTapGesture { picker.onConfirm?([make.id]) }
            }
        }
    }

    private var list: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 5) {
                    ForEach(Array(picker.visible.enumerated()), id: \.element.id) { index, item in
                        if let heading = picker.headingBefore(index) {
                            sectionHeading(heading)
                        }
                        card(item, focused: index == picker.cursor)
                            .id(item.id)
                            .onTapGesture { picker.click(index) }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 12)
            }
            .frame(maxHeight: 400)
            .onChange(of: picker.cursor) { _, _ in
                if let item = picker.visible[safe: picker.cursor] {
                    withAnimation(.easeOut(duration: 0.12)) { proxy.scrollTo(item.id, anchor: .center) }
                }
            }
        }
    }

    /// One heading per group, at the top of it. Filtering can empty a group
    /// entirely, so this follows what is actually on screen rather than the
    /// shape of the original list.
    private func sectionHeading(_ name: String) -> some View {
        let state = picker.folderState(name)
        // Pinning lives here rather than on a task: a pinned task would make a
        // folder of its own, and what changes through the day is the project.
        return HStack(spacing: 8) {
            if picker.multi {
                Text(state.all ? "◉" : (state.some ? "◍" : "○"))
                    .font(type.rowName)
                    .foregroundStyle(state.some ? ink.accent : ink.dim)
                    .contentShape(Rectangle())
                    .onTapGesture { picker.toggleFolder(name) }
            }
            Text(name.uppercased())
                .font(type.small)
                .tracking(2)
                .foregroundStyle(ink.dim)
                .contentShape(Rectangle())
                .onTapGesture { if picker.multi { picker.toggleFolder(name) } }
            Spacer()
        }
        .padding(.horizontal, 4)
        .padding(.top, 14)
        .padding(.bottom, 3)
    }

    private func card(_ item: Item, focused: Bool) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline, spacing: 9) {
                if picker.multi && item.marker != "action" {
                    Text(picker.chosen.contains(item.id) ? "◉" : "○")
                        .font(type.body)
                        .foregroundStyle(picker.chosen.contains(item.id) ? ink.accent : ink.dim)
                }
                if let icon = Icons.forBundle(item.bundle) {
                    Image(nsImage: icon)
                        .resizable()
                        .frame(width: 15 * picker.scale, height: 15 * picker.scale)
                }
                if item.marker == "here" || item.active == true {
                    Text("✓").font(type.rowName).foregroundStyle(ink.accent)
                } else if item.marker == "setting" {
                    // A setting that is off: keep the column, lose the tick.
                    Text("✓").font(type.rowName).foregroundStyle(ink.dim.opacity(0.25))
                }
                if item.marker == nil {
                    Text(item.pinned == true ? "★" : "☆")
                        .font(type.rowName)
                        .foregroundStyle(item.pinned == true ? ink.accent : ink.dim.opacity(0.35))
                        .padding(.horizontal, 2)
                        .contentShape(Rectangle())
                        .onTapGesture { picker.onConfirm?(["__pin__:" + item.id]) }
                }

                Text(item.label)
                    .font(item.marker == "action" ? type.body : type.rowName)
                    .foregroundStyle(item.marker == "action" ? ink.accent : ink.text)
                if let detail = item.detail, !detail.isEmpty {
                    Text(detail).font(type.small).foregroundStyle(ink.dim).lineLimit(1)
                }

                Spacer(minLength: 10)

                if let bundles = item.stack, !bundles.isEmpty {
                    icons(bundles)
                }

                if let count = item.count, count > 0 {
                    // The one big number per row, the way the clock gives the
                    // time. Everything else on the card stays quiet.
                    HStack(alignment: .firstTextBaseline, spacing: 4) {
                        Text("\(count)").font(type.bigNumber).foregroundStyle(ink.text)
                        Text(count == 1 ? "window" : "windows")
                            .font(type.small).foregroundStyle(ink.dim)
                    }
                } else if item.marker == nil {
                    // Only a workspace can be empty. A setting has no windows
                    // to count, so saying "empty" of it means nothing.
                    Text("empty").font(type.small).foregroundStyle(ink.dim.opacity(0.7))
                }
            }

        }
        .padding(.horizontal, 13)
        .padding(.vertical, 9)
        .background(focused ? ink.selected : ink.raised)
        .clipShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
        .overlay(alignment: .leading) {
            Capsule()
                .fill(focused ? ink.accent : .clear)
                .frame(width: 2.5)
                .padding(.vertical, 7)
        }
        .contentShape(Rectangle())
    }

    /// The applications a task holds, as a short overlapping run.
    ///
    /// This is the row's fastest read: a task with an editor and a spreadsheet
    /// in it looks different from a row of browsers before you have read
    /// either name. The overlap is what makes it one object rather than five —
    /// the gap between icons is the panel's own background showing through.
    private func icons(_ bundles: [String]) -> some View {
        let size = 16 * picker.scale
        return HStack(spacing: -size * 0.3) {
            ForEach(Array(bundles.enumerated()), id: \.offset) { _, bundle in
                if let icon = Icons.forBundle(bundle) {
                    Image(nsImage: icon)
                        .resizable()
                        .frame(width: size, height: size)
                        .background(
                            RoundedRectangle(cornerRadius: size * 0.24)
                                .fill(ink.base)
                                .padding(-1.5)
                        )
                }
            }
        }
        .padding(.trailing, 4)
    }

    private var footer: some View {
        HStack(spacing: 16) {
            hint("⇥", "mode")
            hint("↑↓", "move")
            if picker.multi { hint("space", "select") }
            hint("⏎", picker.multi ? "confirm" : "sling")
            if !picker.multi { hint("⌘p", "pin") }
            hint("⌘±", "size")
            hint("esc", "cancel")
            Spacer(minLength: 12)
            if picker.multi, !picker.chosen.isEmpty {
                Text("\(picker.chosen.count) selected").font(type.small).foregroundStyle(ink.accent)
            }
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 11)
        .background(ink.raised)
    }

    private func hint(_ key: String, _ what: String) -> some View {
        HStack(spacing: 5) {
            Text(key).font(type.key).foregroundStyle(ink.text)
            Text(what).font(type.small).foregroundStyle(ink.dim)
        }
    }
}

// MARK: - the board

/// Every window on the machine, grouped by the task it is in.
///
/// The one surface that answers more than once: windows are dragged between
/// tiles and nothing is slung until you confirm, because a tidy-up is a dozen
/// moves and closing after each would make it a dozen keypresses.
struct BoardView: View {
    @ObservedObject var picker: Picker

    private var type: Type { Type(scale: picker.scale) }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            head
            ScrollView {
                LazyVGrid(
                    columns: [GridItem(.adaptive(minimum: 232 * picker.scale), spacing: 11)],
                    alignment: .leading,
                    spacing: 11
                ) {
                    ForEach(picker.boardGroups, id: \.task) { group in
                        Tile(picker: picker, type: type, task: group.task, windows: group.windows)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 16)
            }
            foot
        }
        .background {
            ZStack {
                Vibrancy()
                ink.base.opacity(0.86)
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 13, style: .continuous)
                .stroke(ink.edge, lineWidth: 1)
        )
    }

    private var head: some View {
        VStack(alignment: .leading, spacing: 11) {
            HStack(spacing: 8) {
                Text("slingr").font(type.title).tracking(5).foregroundStyle(ink.text)
                Text("◈").font(type.rowName).foregroundStyle(ink.accent)
                Spacer()
                HStack(spacing: 6) {
                    Text("❯").font(type.body).foregroundStyle(ink.accent)
                    ZStack(alignment: .leading) {
                        if picker.query.isEmpty {
                            Text("type to filter")
                                .font(type.body).foregroundStyle(ink.dim.opacity(0.7))
                        }
                        Text(picker.query).font(type.body).foregroundStyle(ink.text)
                    }
                    .frame(width: 180, alignment: .leading)
                }
                .padding(.horizontal, 11)
                .padding(.vertical, 6)
                .background(ink.raised)
                .clipShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
            }
            tabs
        }
        .padding(.horizontal, 16)
        .padding(.top, 18)
        .padding(.bottom, 14)
    }

    private var tabs: some View {
        HStack(spacing: 4) {
            ForEach(picker.tabs) { tab in
                let on = tab.active == true
                Text(tab.label)
                    .font(type.tab)
                    .tracking(1.5)
                    .foregroundStyle(on ? ink.text : ink.dim)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 7)
                    .background(on ? ink.selected : Color.clear)
                    .clipShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
                    .overlay(alignment: .bottom) {
                        Rectangle()
                            .fill(on ? ink.accent : .clear)
                            .frame(height: 2)
                            .padding(.horizontal, 12)
                    }
                    .contentShape(Rectangle())
                    .onTapGesture { if !on { picker.onConfirm?([tab.id]) } }
            }
            Spacer()
            Text("\(picker.boardGroups.reduce(0) { $0 + $1.windows.count }) windows · \(picker.boardGroups.count) tasks")
                .font(type.small)
                .foregroundStyle(ink.dim)
        }
    }

    private var foot: some View {
        HStack(spacing: 16) {
            if picker.pendingCount > 0 {
                Text("\(picker.pendingCount) waiting to move")
                    .font(type.small)
                    .foregroundStyle(ink.accent)
                hint("⏎", "sling them")
                hint("esc", "put them back")
            } else {
                Text("drag a window onto a task")
                    .font(type.small)
                    .foregroundStyle(ink.dim)
                hint("click", "go there")
                hint("⇥", "mode")
                hint("⌘±", "size")
                hint("esc", "close")
            }
            Spacer(minLength: 12)
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 11)
        .background(ink.raised)
    }

    private func hint(_ key: String, _ what: String) -> some View {
        HStack(spacing: 5) {
            Text(key).font(type.key).foregroundStyle(ink.text)
            Text(what).font(type.small).foregroundStyle(ink.dim)
        }
    }
}

/// One task. Its own view so the drop highlight can be local state rather than
/// something the whole board has to re-render for.
private struct Tile: View {
    @ObservedObject var picker: Picker
    let type: Type
    let task: String
    let windows: [Item]

    @State private var over = false

    private var isHere: Bool { task == picker.here }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 6) {
                Text(task)
                    .font(type.rowName)
                    .foregroundStyle(isHere ? ink.accent : ink.text)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 4)
                Text("\(windows.count)")
                    .font(type.small)
                    .foregroundStyle(ink.dim)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 1)
                    .background(ink.raised)
                    .clipShape(Capsule())
            }
            .padding(.horizontal, 4)
            .padding(.bottom, 5)
            .contentShape(Rectangle())
            .onTapGesture { picker.goTo(task) }

            if windows.isEmpty {
                Text("empty")
                    .font(type.small)
                    .foregroundStyle(ink.dim.opacity(0.6))
                    .padding(.horizontal, 6)
                    .padding(.vertical, 5)
            }
            ForEach(windows) { window in
                chip(window)
            }
        }
        .padding(9)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(over ? ink.selected : ink.raised)
        .clipShape(RoundedRectangle(cornerRadius: 11, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .stroke(
                    over ? ink.accent : (isHere ? ink.accent.opacity(0.7) : ink.edge),
                    lineWidth: over || isHere ? 2 : 1
                )
        )
        .dropDestination(for: String.self) { ids, _ in
            for id in ids { picker.drop(id, on: task) }
            return true
        } isTargeted: { targeted in
            over = targeted
        }
    }

    private func chip(_ window: Item) -> some View {
        // A window sitting somewhere it has not been slung to yet: shown where
        // it is going, tinted so the board says what is about to happen rather
        // than pretending it already has.
        let waiting = picker.pending[window.id] != nil
        return HStack(spacing: 7) {
            if let icon = Icons.forBundle(window.bundle) {
                Image(nsImage: icon)
                    .resizable()
                    .frame(width: 15 * picker.scale, height: 15 * picker.scale)
            }
            Text(window.label)
                .font(type.small)
                .foregroundStyle(waiting ? ink.text : ink.dim)
                .lineLimit(1)
                .truncationMode(.tail)
            if window.pinned == true {
                Text("★").font(type.small).foregroundStyle(ink.pin)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 4)
        .background(waiting ? ink.accent.opacity(0.22) : Color.clear)
        .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
        .contentShape(Rectangle())
        .onTapGesture { picker.goTo(window.id) }
        .draggable(window.id)
    }
}

// MARK: - the panel itself

/// A borderless window refuses key status by default, and a window that cannot
/// become key receives no keyboard events at all — no typing, no arrows, no
/// escape. Overriding this is what makes the panel usable rather than
/// decorative.
final class KeyPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    /// Still not main: this must not become the active application, or it
    /// would take the place of the window being slung.
    override var canBecomeMain: Bool { false }

    var onCancel: (() -> Void)?

    /// The idiomatic escape route, in case the event monitor is ever bypassed.
    override func cancelOperation(_: Any?) {
        onCancel?()
    }
}

@MainActor
final class Controller: NSObject, NSApplicationDelegate, NSWindowDelegate {
    private var panel: KeyPanel!
    private let picker: Picker
    private var monitor: Any?
    private var wasKey = false

    init(picker: Picker) { self.picker = picker }

    func applicationDidFinishLaunching(_: Notification) {
        picker.onConfirm = { [weak self] ids in self?.finish(with: ids) }
        picker.onResize = { [weak self] in self?.refit() }
        // The board is a different shape of thing: it holds every window on the
        // machine at once, so it is wide where the picker is narrow.
        let room = (NSScreen.main?.visibleFrame.width ?? 1280) - 80
        let width: CGFloat = picker.isBoard ? min(1060, room) : 640
        let view = NSHostingView(
            rootView: picker.isBoard
                ? AnyView(BoardView(picker: picker))
                : AnyView(PanelView(picker: picker))
        )
        view.frame = NSRect(x: 0, y: 0, width: width, height: 560)
        // Let the content decide the height, within reason: a fixed frame
        // leaves a short list floating in dead space and a long one clipped.
        let fitted = view.fittingSize
        let height = min(max(fitted.height, picker.isBoard ? 340 : 220), tallest)
        view.frame = NSRect(x: 0, y: 0, width: width, height: height)

        panel = KeyPanel(
            contentRect: view.frame,
            // .nonactivatingPanel is the point: it takes keys without bringing
            // this process to the front, so the window being slung keeps its
            // place in the app switcher and in AeroSpace's idea of focus.
            styleMask: [.nonactivatingPanel, .borderless, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        panel.contentView = view
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.level = .floating
        panel.isFloatingPanel = true
        panel.hidesOnDeactivate = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        panel.onCancel = { [weak self] in self?.finish(with: []) }
        panel.delegate = self
        place(panel)

        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            self?.handle(event) == true ? nil : event
        }

        panel.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: false)

        // A panel that is not key receives no keys at all, which looks like
        // "escape does nothing" rather than like a broken window. Say so on
        // stderr, which is discarded in normal use and visible when testing.
        FileHandle.standardError.write(
            Data("slingr-panel: key=\(panel.isKeyWindow) active=\(NSApp.isActive)\n".utf8))

        // Never become furniture. A picker nobody answered is a mistake, and
        // an abandoned one should not need killing from a terminal.
        DispatchQueue.main.asyncAfter(deadline: .now() + 300) { [weak self] in
            self?.finish(with: [])
        }
    }

    /// Grow or shrink the window around the text, keeping the top edge put so
    /// the list does not appear to jump when the type changes size.
    /// How tall the surface may grow. The board earns more of the screen than
    /// the picker does: it is the one you read rather than answer.
    private var tallest: CGFloat { picker.isBoard ? 780 : 620 }

    private func refit() {
        guard let view = panel.contentView else { return }
        let top = panel.frame.maxY
        let height = min(max(view.fittingSize.height, 220), tallest)
        var frame = panel.frame
        frame.size.height = height
        frame.origin.y = top - height
        panel.setFrame(frame, display: true, animate: false)
    }

    /// Centred horizontally, and high — a picker belongs where the eye already
    /// is, not in the middle of the screen. `center()` puts a tall panel low
    /// and lands on whichever screen AppKit feels like; this follows the mouse
    /// to the screen actually being used.
    private func place(_ panel: NSPanel) {
        let mouse = NSEvent.mouseLocation
        let screen = NSScreen.screens.first { NSMouseInRect(mouse, $0.frame, false) }
            ?? NSScreen.main
        guard let area = screen?.visibleFrame else {
            panel.center()
            return
        }
        let size = panel.frame.size
        let x = area.midX - size.width / 2
        // A fifth of the way down, measured from the top.
        let y = area.maxY - size.height - (area.height * 0.18)
        panel.setFrameOrigin(NSPoint(x: x.rounded(), y: max(area.minY, y).rounded()))
    }

    func windowDidBecomeKey(_: Notification) {
        wasKey = true
    }

    /// Clicking away dismisses, the way any picker should. It is also the way
    /// out if the keyboard route ever fails: a panel with no mouse escape and
    /// no keys is one that has to be killed from a terminal.
    func windowDidResignKey(_: Notification) {
        guard wasKey else { return }
        finish(with: [])
    }

    /// Returns true when the key was ours to deal with.
    private func handle(_ event: NSEvent) -> Bool {
        let ctrl = event.modifierFlags.contains(.control)
        switch event.keyCode {
        case 53:                                                     // esc
            // On the board, escape undoes the tidy-up before it closes the
            // board — losing a dozen drags to a stray keypress is a worse
            // mistake than needing two presses to leave.
            if picker.isBoard, picker.pendingCount > 0 {
                picker.pending.removeAll()
                return true
            }
            finish(with: [])
            return true
        case 125: picker.move(1); return true                        // down
        case 126: picker.move(-1); return true                       // up
        case 36, 76:                                                 // return
            // A board with nothing waiting has nothing to confirm; answering
            // with an empty list would read as cancelled.
            let answer = picker.confirm()
            if picker.isBoard, answer.isEmpty { return true }
            finish(with: answer)
            return true
        case 48:                                                     // tab
            if let other = picker.otherTab { finish(with: [other.id]) }
            return true
        case 49 where picker.multi: picker.toggle(); return true     // space
        case 51:                                                     // delete
            if !picker.query.isEmpty { picker.query.removeLast(); picker.cursor = 0 }
            return true
        default: break
        }
        if event.modifierFlags.contains(.command),
           let c = event.charactersIgnoringModifiers {
            switch c {
            case "+", "=": picker.zoom(1); return true
            case "-", "_": picker.zoom(-1); return true
            case "0": picker.zoom(0); return true
            default: break
            }
        }
        // Pinning answers by asking again: the caller stores it and reopens,
        // because the order of the list changes underneath.
        if event.modifierFlags.contains(.command),
           event.charactersIgnoringModifiers?.lowercased() == "p",
           let item = picker.visible[safe: picker.cursor], item.marker == nil {
            finish(with: ["__pin__:" + item.id])
            return true
        }
        if ctrl, let c = event.charactersIgnoringModifiers?.lowercased() {
            switch c {
            case "n": picker.move(1); return true
            case "p": picker.move(-1); return true
            case "c": finish(with: []); return true
            default: break
            }
        }
        if !ctrl, !event.modifierFlags.contains(.command),
           let typed = event.characters, typed.allSatisfy({ !$0.isNewline }), !typed.isEmpty {
            picker.query += typed
            picker.cursor = 0
            return true
        }
        return false
    }

    private func finish(with ids: [String]) {
        if let monitor { NSEvent.removeMonitor(monitor) }
        for id in ids { print(id) }
        exit(ids.isEmpty ? 1 : 0)
    }
}

// MARK: - entry

let input = FileHandle.standardInput.readDataToEndOfFile()
guard let request = try? JSONDecoder().decode(Request.self, from: input) else {
    FileHandle.standardError.write(Data("slingr-panel: could not read the request\n".utf8))
    exit(2)
}

/// Top-level code is nonisolated under Swift 6 strict concurrency, but it does
/// run on the main thread, so this states what is already true rather than
/// hopping queues.
ink = Ink(request.theme)

MainActor.assumeIsolated {
    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)   // no Dock icon, no app switcher entry
    let controller = Controller(picker: Picker(request))
    app.delegate = controller
    // The delegate is the only strong reference the app holds; keep one here
    // too so it cannot be collected while the panel is up.
    withExtendedLifetime(controller) { app.run() }
}
