// Lists what AccessibilityPlatformTranslation offers for pressing an element: every AXP class with its properties and
// methods, and the names of the methods worth disassembling (press, perform, action, activate). Read once on the Mac to
// learn the request that presses an element; ddp then sends it the way simulator.rs sends its reads.
//
//   axp_dump            prints the classes, then one "DISASSEMBLE <name>" line per method to look at
//   axp_dump --wait     the same, then "READY <pid>" and waits on stdin, so lldb can attach and disassemble in here
import Foundation
import ObjectiveC

let framework = "/System/Library/PrivateFrameworks/AccessibilityPlatformTranslation.framework/AccessibilityPlatformTranslation"
guard dlopen(framework, RTLD_NOW) != nil else {
    print("FAIL dlopen \(framework)")
    exit(1)
}

let interesting = try! NSRegularExpression(pattern: "press|perform|action|activate|requestType|processTranslatorRequest", options: [.caseInsensitive])
func matches(_ text: String) -> Bool {
    interesting.firstMatch(in: text, range: NSRange(text.startIndex..., in: text)) != nil
}

var count: UInt32 = 0
let classes = objc_copyClassList(&count)!
var names: [String] = []
for index in 0..<Int(count) {
    let name = String(cString: class_getName(classes[index]))
    if name.hasPrefix("AXP") { names.append(name) }
}
names.sort()

var toDisassemble: [String] = []
for name in names {
    let cls: AnyClass = objc_getClass(name) as! AnyClass
    let superName = class_getSuperclass(cls).map { String(cString: class_getName($0)) } ?? "-"
    print("CLASS \(name) : \(superName)")
    var propertyCount: UInt32 = 0
    if let properties = class_copyPropertyList(cls, &propertyCount) {
        for index in 0..<Int(propertyCount) {
            let property = properties[index]
            let attributes = property_getAttributes(property).map { String(cString: $0) } ?? ""
            print("  property \(String(cString: property_getName(property))) \(attributes)")
        }
        free(properties)
    }
    for (prefix, owner) in [("-", cls), ("+", object_getClass(cls)!)] {
        var methodCount: UInt32 = 0
        guard let methods = class_copyMethodList(owner, &methodCount) else { continue }
        for index in 0..<Int(methodCount) {
            let selector = NSStringFromSelector(method_getName(methods[index]))
            let types = method_getTypeEncoding(methods[index]).map { String(cString: $0) } ?? ""
            print("  \(prefix)\(selector) \(types)")
            if matches(selector) || matches(name) && selector.hasPrefix("process") {
                toDisassemble.append("\(prefix)[\(name) \(selector)]")
            }
        }
        free(methods)
    }
}
for name in toDisassemble.prefix(60) {
    print("DISASSEMBLE \(name)")
}
print("CLASSES \(names.count) TO_DISASSEMBLE \(toDisassemble.count)")

if CommandLine.arguments.contains("--wait") {
    print("READY \(getpid())")
    fflush(stdout)
    _ = readLine()
}
