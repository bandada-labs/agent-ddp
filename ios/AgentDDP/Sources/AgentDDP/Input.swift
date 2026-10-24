#if DEBUG && canImport(UIKit)
import Darwin
import UIKit

/// Real touches and typing from inside the app. iOS has no public API for an app to touch
/// itself, so touches are IOHIDEvents handed to UIKit the way backboardd hands it a finger (the approach of Lyft's
/// Hammer). UIKit then builds the UITouch itself, so gesture recognizers and SwiftUI see a real touch. Private API,
/// so a new iOS version can break it (decisions log). Main thread only.
// ponytail: one finger. Add more when a client sends multi-touch (pinch).
enum Input {
    private static var finger: (window: UIWindow, point: CGPoint)?
    static var isTouching: Bool { finger != nil }

    /// Moves the one finger to `point` in screen points, or lifts it when nil. Pressing, moving and lifting follow
    /// from whether a finger is down, so mouse and touch events share this.
    static func touch(_ point: CGPoint?) throws {
        switch (finger, point) {
        case (nil, let point?):
            guard let window = window(at: point) else { throw Control.Failure(description: "No window at that point") }
            finger = (window, point)
            try send(point, phase: .began, to: window)
        case (let down?, let point?):
            finger = (down.window, point)
            try send(point, phase: .moved, to: down.window)
        case (let down?, nil):
            finger = nil
            try send(down.point, phase: .ended, to: down.window)
        case (nil, nil):
            break
        }
    }

    /// Text into the first responder, as the keyboard types it. Nothing happens when no field has focus, like keys
    /// sent to a page with nothing focused.
    static func type(_ text: String) {
        (firstResponder() as? UIKeyInput)?.insertText(text.replacingOccurrences(of: "\r", with: "\n"))
    }

    /// A key without text of its own, by name (`Enter`, `Tab`, `Backspace`). Other keys do nothing.
    static func press(_ key: String) {
        guard let field = firstResponder() as? UIKeyInput else { return }
        switch key {
        case "Backspace": field.deleteBackward()
        case "Enter": field.insertText("\n")
        case "Tab": field.insertText("\t")
        default: break
        }
    }

    private static func window(at point: CGPoint) -> UIWindow? {
        FrameCapture.visibleWindows().sorted { $0.windowLevel > $1.windowLevel }.first { $0.hitTest(point, with: nil) != nil }
    }

    private static func firstResponder() -> UIResponder? {
        func find(_ view: UIView) -> UIView? { view.isFirstResponder ? view : view.subviews.lazy.compactMap(find).first }
        return FrameCapture.visibleWindows().lazy.compactMap(find).first
    }

    // IOHIDEvent values from IOKit's IOHIDEventTypes.h.
    private static let transducerHand: UInt32 = 3
    private static let eventRange: UInt32 = 1 << 0, eventTouch: UInt32 = 1 << 1, eventPosition: UInt32 = 1 << 2
    private static let fieldIsDisplayIntegrated: UInt32 = 11 << 16 | 25
    private static let senderId: UInt64 = 0x0000_0000_DDDD_0001

    private typealias CreateHand = @convention(c) (CFAllocator?, UInt64, UInt32, UInt32, UInt32, UInt32, UInt32,
                                                   Double, Double, Double, Double, Double, Bool, Bool, UInt32) -> UnsafeMutableRawPointer?
    private typealias CreateFinger = @convention(c) (CFAllocator?, UInt64, UInt32, UInt32, UInt32, Double, Double, Double, Double,
                                                     Double, Double, Double, Double, Double, Double, Bool, Bool, UInt32) -> UnsafeMutableRawPointer?
    private typealias Append = @convention(c) (UnsafeMutableRawPointer, UnsafeMutableRawPointer, UInt32) -> Void
    private typealias SetInteger = @convention(c) (UnsafeMutableRawPointer, UInt32, Int32) -> Void
    private typealias SetSender = @convention(c) (UnsafeMutableRawPointer, UInt64) -> Void
    private typealias SetDigitizerInfo = @convention(c) (UnsafeMutableRawPointer, UInt32, Bool, Bool, CFString?, Double, Float) -> Void
    private typealias Enqueue = @convention(c) (NSObject, Selector, UnsafeMutableRawPointer) -> Void

    /// One hand event holding one finger, addressed to the window's render context as backboardd addresses it.
    private static func send(_ point: CGPoint, phase: UITouch.Phase, to window: UIWindow) throws {
        let createHand: CreateHand = try symbol("IOHIDEventCreateDigitizerEvent")
        let createFinger: CreateFinger = try symbol("IOHIDEventCreateDigitizerFingerEventWithQuality")
        let append: Append = try symbol("IOHIDEventAppendEvent")
        let setInteger: SetInteger = try symbol("IOHIDEventSetIntegerValue")
        let setSender: SetSender = try symbol("IOHIDEventSetSenderID")
        let setDigitizerInfo: SetDigitizerInfo = try symbol("BKSHIDEventSetDigitizerInfo")
        guard let contextId = (window.value(forKey: "_contextId") as? NSNumber)?.uint32Value else { throw unsupported("_contextId") }
        let selector = NSSelectorFromString("_enqueueHIDEvent:")
        guard UIApplication.shared.responds(to: selector) else { throw unsupported("_enqueueHIDEvent:") }

        let time = mach_absolute_time()
        let touching = phase != .ended
        let mask = phase == .moved ? eventPosition : eventRange | eventTouch
        guard let hand = createHand(kCFAllocatorDefault, time, transducerHand, 0, 0, mask, 0, 0, 0, 0, 0, 0, touching, touching, 0),
              let finger = createFinger(kCFAllocatorDefault, time, 1, 2, mask, point.x, point.y, 0, touching ? 1 : 0, 0, 5, 5, 1, 1, 1,
                                        touching, touching, 0) else {
            throw unsupported("IOHIDEventCreateDigitizerEvent")
        }
        defer {
            Unmanaged<AnyObject>.fromOpaque(finger).release()
            Unmanaged<AnyObject>.fromOpaque(hand).release()
        }
        setInteger(hand, fieldIsDisplayIntegrated, 1)
        setInteger(finger, fieldIsDisplayIntegrated, 1)
        append(hand, finger, 0)
        setSender(hand, senderId)
        setDigitizerInfo(hand, contextId, false, false, nil, 0, 0)
        unsafeBitCast(UIApplication.shared.method(for: selector), to: Enqueue.self)(UIApplication.shared, selector, hand)
        Hooks.ok("input.hid")
    }

    private static func symbol<T>(_ name: String) throws -> T {
        guard let address = dlsym(UnsafeMutableRawPointer(bitPattern: -2), name) else { throw unsupported(name) }
        return unsafeBitCast(address, to: T.self)
    }

    private static func unsupported(_ name: String) -> Control.Failure {
        Hooks.failed("input.hid", "no \(name)")
        return Control.Failure(description: "Touches need \(name), which this iOS version does not have")
    }
}
#endif
