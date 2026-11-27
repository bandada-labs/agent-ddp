// What AccessibilityPlatformTranslation sends when a Mac tool presses a simulator element, read without a simulator,
// a debugger or a disassembler. The Mac side turns an NSAccessibility press on its element into an AXPTranslatorRequest
// and hands it to the bridge delegate, which normally carries it to the simulator. Here the delegate is this tool: it
// prints every request it is handed (each property of it) and answers with an empty response. ddp then sends the same
// request the way simulator.rs sends its reads.
//
// Also lists the AXP* classes with their properties and methods, for what the requests do not show.
import AppKit
import Foundation
import ObjectiveC

setvbuf(stdout, nil, _IOLBF, 0)  // a crash mid-run keeps every line printed before it
let framework = "/System/Library/PrivateFrameworks/AccessibilityPlatformTranslation.framework/AccessibilityPlatformTranslation"
guard dlopen(framework, RTLD_NOW) != nil else {
    print("FAIL dlopen \(framework)")
    print("RESULT FAIL")
    exit(1)
}

func properties(of cls: AnyClass) -> [String] {
    var names: [String] = []
    var current: AnyClass? = cls
    while let c = current, String(cString: class_getName(c)) != "NSObject" {
        var count: UInt32 = 0
        if let list = class_copyPropertyList(c, &count) {
            for index in 0..<Int(count) { names.append(String(cString: property_getName(list[index]))) }
            free(list)
        }
        current = class_getSuperclass(c)
    }
    return names
}

// 1. The classes.
var classCount: UInt32 = 0
let classes = objc_copyClassList(&classCount)!
var names: [String] = []
for index in 0..<Int(classCount) {
    let name = String(cString: class_getName(classes[index]))
    if name.hasPrefix("AXP") { names.append(name) }
}
for name in names.sorted() {
    let cls: AnyClass = objc_getClass(name) as! AnyClass
    print("CLASS \(name) : \(class_getSuperclass(cls).map { String(cString: class_getName($0)) } ?? "-")")
    var count: UInt32 = 0
    if let list = class_copyPropertyList(cls, &count) {
        for index in 0..<Int(count) {
            let attributes = property_getAttributes(list[index]).map { String(cString: $0) } ?? ""
            print("  property \(String(cString: property_getName(list[index]))) \(attributes)")
        }
        free(list)
    }
    for (prefix, owner) in [("-", cls), ("+", object_getClass(cls)!)] {
        guard let list = class_copyMethodList(owner, &count) else { continue }
        for index in 0..<Int(count) {
            let types = method_getTypeEncoding(list[index]).map { String(cString: $0) } ?? ""
            print("  \(prefix)\(NSStringFromSelector(method_getName(list[index]))) \(types)")
        }
        free(list)
    }
}

// 2. The requests a press makes.
let requestClass: AnyClass = objc_getClass("AXPTranslatorRequest") as! AnyClass
let requestProperties = properties(of: requestClass)
var seen = 0

func describe(_ request: AnyObject) {
    seen += 1
    print("REQUEST \(seen)")
    for name in requestProperties {
        let value = (request as! NSObject).value(forKey: name)
        print("  \(name) = \(value.map { String(describing: $0) } ?? "nil")")
    }
}

final class Bridge: NSObject {
    @objc func accessibilityTranslationDelegateBridgeCallbackWithToken(_ token: NSString) -> AnyObject {
        let callback: @convention(block) (AnyObject) -> AnyObject? = { request in
            describe(request)
            let response = objc_getClass("AXPTranslatorResponse") as! NSObject.Type
            return response.init()
        }
        // A Swift closure returned as Any would be boxed, not handed over as a block.
        return unsafeBitCast(callback, to: AnyObject.self)
    }
    @objc func accessibilityTranslationConvertPlatformFrameToSystem(_ frame: NSRect, withToken token: NSString) -> NSRect { frame }
    @objc func accessibilityTranslationRootParent(withToken token: NSString) -> AnyObject? { nil }
}

let translatorClass = objc_getClass("AXPTranslator") as! NSObject.Type
let translator = translatorClass.perform(NSSelectorFromString("sharedInstance"))!.takeUnretainedValue() as! NSObject
let bridge = Bridge()
for (key, value) in [("bridgeTokenDelegate", bridge as Any), ("supportsDelegateTokens", true), ("accessibilityEnabled", true)] {
    if translator.responds(to: NSSelectorFromString("set\(key.prefix(1).uppercased())\(key.dropFirst()):")) {
        translator.setValue(value, forKey: key)
        print("SET translator.\(key)")
    } else {
        print("MISSING translator.\(key)")
    }
}

// A made-up element: pid 1, object id 1, our token. The press never reaches a simulator, only the delegate above.
let translationClass = objc_getClass("AXPTranslationObject") as! NSObject.Type
let translation = translationClass.init()
for (key, value) in [("pid", 1 as Any), ("objectID", 1), ("bridgeDelegateToken", "ddp-probe")] {
    translation.setValue(value, forKey: key)
}
let macElementSelector = NSSelectorFromString("macPlatformElementFromTranslation:")
guard translator.responds(to: macElementSelector),
      let element = translator.perform(macElementSelector, with: translation)?.takeUnretainedValue() as? NSObject else {
    print("FAIL no macPlatformElementFromTranslation: on AXPTranslator")
    print("RESULT FAIL")
    exit(1)
}
print("ELEMENT \(type(of: element))")

let actions: [(String, Any?)] = [("accessibilityPerformPress", nil), ("accessibilityPerformShowMenu", nil),
                                 ("accessibilityPerformAction:", "AXPress")]
for (selector, argument) in actions {
    let before = seen
    guard element.responds(to: NSSelectorFromString(selector)) else {
        print("MISSING \(selector)")
        continue
    }
    _ = element.perform(NSSelectorFromString(selector), with: argument)
    RunLoop.current.run(until: Date().addingTimeInterval(1))  // in case the request goes out on the next turn
    print("PERFORMED \(selector) \(argument.map { "\($0)" } ?? ""): \(seen - before) request(s)")
}
print("RESULT \(seen > 0 ? "PASS" : "FAIL") (\(seen) request(s) seen)")
