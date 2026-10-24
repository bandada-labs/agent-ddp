import AgentDDP
import OSLog
import SwiftUI

@main
struct DDPSampleApp: App {
    init() {
        AgentDDP.start()
        AgentDDP.redact(values: [SampleNetwork.devSecret])
        AgentDDP.command("load") { _ in await SampleNetwork.load() }
    }

    var body: some Scene {
        WindowGroup { ContentView() }
    }
}

struct ContentView: View {
    @State private var isSelected = false
    @State private var name = ""
    @State private var lastAction = "none"
    @State private var isFlashing = false

    var body: some View {
        VStack(spacing: 24) {
            Button {
                Logger(subsystem: "dev.agentddp.sample", category: "ui").info("animatedButton tapped")
                withAnimation(.easeInOut(duration: 2.0)) { isSelected.toggle() }
                // Planted bug for the v1 proof: a border that exists only while animating.
                withAnimation(.easeInOut(duration: 0.4)) { isFlashing = true }
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) {
                    withAnimation(.easeInOut(duration: 0.4)) { isFlashing = false }
                }
            } label: {
                Text(isSelected ? "Selected" : "Tap me")
                    .font(.headline)
                    .foregroundStyle(isSelected ? .white : .blue)
                    .padding(.horizontal, 32)
                    .padding(.vertical, 14)
                    .background(
                        RoundedRectangle(cornerRadius: 12)
                            .fill(isSelected ? Color.blue : Color.white)
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: 12)
                            .stroke(isSelected ? Color.orange : Color.blue, lineWidth: isSelected ? 6 : 2)
                    )
            }
            .accessibilityIdentifier("animatedButton")

            // Harder case: gradient, shadow, custom path. Checks whether these stay readable as layers.
            Capsule()
                .fill(LinearGradient(colors: [.purple, isSelected ? .green : .pink], startPoint: .leading, endPoint: .trailing))
                .frame(width: isSelected ? 220 : 120, height: 30)
                .shadow(color: .black.opacity(0.4), radius: isSelected ? 10 : 2)
                .accessibilityIdentifier("gradientCapsule")

            // Survey cases that may not map to plain CALayers: custom Path, Canvas, drawingGroup.
            HStack(spacing: 20) {
                Triangle()
                    .fill(isSelected ? Color.green : Color.red)
                    .overlay(Triangle().stroke(isSelected ? Color.black : Color.orange, lineWidth: isSelected ? 6 : 2))
                    .frame(width: 60, height: 60)
                    .offset(x: isSelected ? 20 : 0)
                    .accessibilityIdentifier("customPath")

                CanvasDot(progress: isSelected ? 1 : 0)
                    .frame(width: 80, height: 60)
                    .accessibilityIdentifier("canvasDot")

                RoundedRectangle(cornerRadius: 10)
                    .fill(isSelected ? Color.yellow : Color.purple)
                    .overlay(RoundedRectangle(cornerRadius: 10).stroke(Color.cyan, lineWidth: isSelected ? 6 : 2))
                    .frame(width: isSelected ? 80 : 50, height: 50)
                    .drawingGroup()
                    .accessibilityIdentifier("drawingGroupBox")
            }
            .frame(height: 70)

            HStack(spacing: 40) {
                Text("Flash")
                    .padding(10)
                    .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color.red, lineWidth: isFlashing ? 4 : 0))
                    .accessibilityIdentifier("flashBox")

                // Planted bug for the v1 proof: a banner drawn on top of a label.
                Text("Covered label")
                    .accessibilityIdentifier("coveredLabel")
                    .overlay(Rectangle().fill(Color.gray).frame(width: 140, height: 30))
            }

            HStack(spacing: 24) {
                // Too small to tap comfortably (24 pt, Apple asks for 44).
                Button { lastAction = "small" } label: { Color.gray.opacity(0.3).frame(width: 24, height: 24) }
                    .accessibilityIdentifier("smallTarget")
                    .accessibilityLabel("Small")
                Text("Faint text")
                    .foregroundStyle(Color(white: 0.8))
                    .accessibilityIdentifier("faintText")
                // Must stay quiet: its touch area is 12 pt wider than its frame on every side, so a touch just
                // outside the frame still reaches it. ddp must not call it covered or small.
                Button("Wide") { lastAction = "wide" }
                    .contentShape(Rectangle().inset(by: -12))
                    .accessibilityIdentifier("wideHit")
            }

            // An app timeline on ddp's clock: `ddp time pause/step/seek` freeze, step and rewind it.
            TimelineView(.animation) { _ in
                let phase = AgentDDP.elapsedTime().truncatingRemainder(dividingBy: 2) / 2
                Rectangle().fill(Color.orange).frame(width: 20 + 100 * phase, height: 8)
                    .frame(width: 120, alignment: .leading)
                    .accessibilityElement().accessibilityIdentifier("clockBar")
            }

            // Buttons that show but cannot be tapped, for `ddp check tappable` and `ddp tap`.
            HStack(spacing: 12) {
                Button("Off") { lastAction = "off" }
                    .disabled(true)
                    .accessibilityIdentifier("trapDisabled")
                ZStack {
                    Button("Under") { lastAction = "under" }
                        .accessibilityIdentifier("trapCovered")
                    // A clear view that takes the touch and draws nothing.
                    Color.clear.contentShape(Rectangle()).onTapGesture {}
                }
                .frame(width: 70, height: 44)
                HStack {
                    Button("Ignored") { lastAction = "ignored" }
                        .accessibilityIdentifier("trapIgnored")
                }
                .allowsHitTesting(false)
                OutsideButton { lastAction = "outside" }
                    .frame(width: 20, height: 44)
            }
            .padding(.trailing, 60)

            // Controls a parent takes the touch from while it travels down to them.
            HStack(spacing: 12) {
                HStack {
                    Button("High") { lastAction = "high" }
                        .accessibilityIdentifier("trapHighPriority")
                }
                .padding(6)
                .highPriorityGesture(TapGesture())
                TapAwayRow(
                    row: { lastAction = "row" },
                    button: { lastAction = "uikit button" },
                    swiftUI: AnyView(Button("SwiftUI") { lastAction = "hosted" }.accessibilityIdentifier("tapAwaySwiftUI"))
                )
                .frame(width: 240, height: 44)
            }

            List {
                Text("Row with context menu")
                    .accessibilityIdentifier("menuRow")
                    .contextMenu {
                        Button("Rename") { lastAction = "rename" }
                        Button("Delete", role: .destructive) { lastAction = "delete" }
                    }
                Text("Last action: \(lastAction)")
                // For `Input` typing: Enter submits and shows the text as the last action.
                TextField("Name", text: $name)
                    .accessibilityIdentifier("nameField")
                    .onSubmit { lastAction = "submit \(name)" }
                Button("Load") { Task { lastAction = await SampleNetwork.load() } }
                    .accessibilityIdentifier("netButton")
            }
        }
        .padding(.top, 60)
        // A deep link (ddpsample://...) shows up as the last action, for `ddp open`.
        .onOpenURL { lastAction = "link \($0.absoluteString)" }
    }
}

/// A UIKit button placed outside its parent's bounds: it draws (the parent does not clip) but no touch reaches it.
struct OutsideButton: UIViewRepresentable {
    let action: () -> Void

    func makeUIView(context: Context) -> UIView {
        let parent = UIView()
        let button = UIButton(type: .system, primaryAction: UIAction { _ in action() })
        button.setTitle("Outside", for: .normal)
        button.accessibilityIdentifier = "trapOutside"
        button.frame = CGRect(x: 30, y: 0, width: 70, height: 44)
        parent.addSubview(button)
        return parent
    }

    func updateUIView(_ view: UIView, context: Context) {}
}

/// "Tap anywhere to close the keyboard": a tap recognizer on the parent. It takes the tap from a table row (the row is
/// never selected) but not from a UIButton, which UIKit lets win.
struct TapAwayRow: UIViewRepresentable {
    let row: () -> Void
    let button: () -> Void
    let swiftUI: AnyView

    func makeCoordinator() -> Coordinator { Coordinator(row: row, hosted: UIHostingController(rootView: swiftUI)) }

    func makeUIView(context: Context) -> UIView {
        let parent = UIView()
        parent.addGestureRecognizer(UITapGestureRecognizer(target: parent, action: #selector(UIView.endEditing(_:))))
        let table = UITableView(frame: CGRect(x: 0, y: 0, width: 80, height: 44))
        table.dataSource = context.coordinator
        table.delegate = context.coordinator
        table.rowHeight = 44
        table.isScrollEnabled = false
        parent.addSubview(table)
        let uiButton = UIButton(type: .system, primaryAction: UIAction { _ in button() })
        uiButton.setTitle("UIKit", for: .normal)
        uiButton.accessibilityIdentifier = "tapAwayButton"
        uiButton.frame = CGRect(x: 90, y: 0, width: 60, height: 44)
        parent.addSubview(uiButton)
        let hosted = context.coordinator.hosted.view!
        hosted.backgroundColor = .clear
        hosted.frame = CGRect(x: 160, y: 0, width: 80, height: 44)
        parent.addSubview(hosted)
        return parent
    }

    func updateUIView(_ view: UIView, context: Context) {}

    final class Coordinator: NSObject, UITableViewDataSource, UITableViewDelegate {
        let row: () -> Void
        let hosted: UIHostingController<AnyView>
        init(row: @escaping () -> Void, hosted: UIHostingController<AnyView>) {
            self.row = row
            self.hosted = hosted
        }

        func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int { 1 }

        func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
            let cell = UITableViewCell()
            cell.textLabel?.text = "Row"
            cell.isAccessibilityElement = true
            cell.accessibilityLabel = "Row"
            cell.accessibilityTraits = .button
            cell.accessibilityIdentifier = "trapTapAway"
            return cell
        }

        func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) { row() }
    }
}

struct Triangle: Shape {
    func path(in rect: CGRect) -> Path {
        Path { path in
            path.move(to: CGPoint(x: rect.midX, y: rect.minY))
            path.addLine(to: CGPoint(x: rect.maxX, y: rect.maxY))
            path.addLine(to: CGPoint(x: rect.minX, y: rect.maxY))
            path.closeSubpath()
        }
    }
}

// Canvas does not interpolate on its own; Animatable feeds it the in-between progress.
struct CanvasDot: View, Animatable {
    var progress: Double
    var animatableData: Double {
        get { progress }
        set { progress = newValue }
    }

    var body: some View {
        Canvas { context, size in
            let diameter = 20 + 20 * progress
            let dotRect = CGRect(x: (size.width - diameter) * progress, y: (size.height - diameter) / 2, width: diameter, height: diameter)
            let dotColor = Color(red: 1 - progress, green: 0, blue: progress)
            context.fill(Path(ellipseIn: dotRect), with: .color(dotColor))
            context.stroke(Path(ellipseIn: dotRect), with: .color(.black), lineWidth: 1 + 3 * progress)
        }
    }
}

/// The sample's network call. `ddp net` sees it through the Debug decorator below, the way an app
/// wraps its own network port in its Debug composition root.
enum SampleNetwork {
    static let devSecret = "dev-secret-123"
    // A local test server (python3 -m http.server style) on the host; nothing leaves the machine.
    static let session = ObservedSession(session: .shared)

    static func load() async -> String {
        var request = URLRequest(url: URL(string: "http://127.0.0.1:18765/items.json?token=abc&page=1")!)
        request.setValue("Bearer \(devSecret)", forHTTPHeaderField: "Authorization")
        request.setValue("sample", forHTTPHeaderField: "X-Client")
        do {
            let (_, response) = try await session.data(for: request)
            return "loaded \((response as? HTTPURLResponse)?.statusCode ?? 0)"
        } catch {
            return "load failed"
        }
    }
}

/// Debug decorator around URLSession: ddp sees each request and can pause, answer or fail it. Release builds would
/// use the plain session; `AgentDDP.send` only calls the session there anyway.
struct ObservedSession {
    let session: URLSession

    func data(for request: URLRequest) async throws -> (Data, URLResponse) {
        try await AgentDDP.send(request) { try await session.data(for: $0) }
    }
}
