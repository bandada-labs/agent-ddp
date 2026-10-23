import AgentDDP
import SwiftUI

@main
struct DDPSampleApp: App {
    init() {
        AgentDDP.start()
        AgentDDP.redact(values: [SampleNetwork.devSecret])
    }

    var body: some Scene {
        WindowGroup { ContentView() }
    }
}

struct ContentView: View {
    @State private var isSelected = false
    @State private var lastAction = "none"
    @State private var isFlashing = false

    var body: some View {
        VStack(spacing: 24) {
            Button {
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

            // An app timeline on ddp's clock: `ddp time pause/step/seek` freeze, step and rewind it.
            TimelineView(.animation) { _ in
                let phase = AgentDDP.elapsedTime().truncatingRemainder(dividingBy: 2) / 2
                Rectangle().fill(Color.orange).frame(width: 20 + 100 * phase, height: 8)
                    .frame(width: 120, alignment: .leading)
                    .accessibilityElement().accessibilityIdentifier("clockBar")
            }

            List {
                Text("Row with context menu")
                    .accessibilityIdentifier("menuRow")
                    .contextMenu {
                        Button("Rename") { lastAction = "rename" }
                        Button("Delete", role: .destructive) { lastAction = "delete" }
                    }
                Text("Last action: \(lastAction)")
                Button("Load") { Task { lastAction = await SampleNetwork.load() } }
                    .accessibilityIdentifier("netButton")
            }
        }
        .padding(.top, 60)
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
        var request = URLRequest(url: URL(string: "http://127.0.0.1:18765/build.json?token=abc&page=1")!)
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

/// Debug decorator around URLSession: reports each request to ddp. Release builds would use the
/// plain session; `AgentDDP.logRequest` is empty there anyway.
struct ObservedSession {
    let session: URLSession

    func data(for request: URLRequest) async throws -> (Data, URLResponse) {
        let startedAt = Date()
        do {
            let (data, response) = try await session.data(for: request)
            AgentDDP.logRequest(request, response: response, body: data, error: nil, startedAt: startedAt)
            return (data, response)
        } catch {
            AgentDDP.logRequest(request, response: nil, body: nil, error: error, startedAt: startedAt)
            throw error
        }
    }
}
