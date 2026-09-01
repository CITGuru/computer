// The input and measurement helper a macOS box is driven through.

import CoreGraphics
import Foundation

let source = CGEventSource(stateID: .hidSystemState)

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
}

// Posted at the HID tap so the event enters below every event tap, which is
// what makes it indistinguishable from real hardware to the application.
func post(_ event: CGEvent?) {
    guard let event else { fail("the event could not be created") }
    event.post(tap: .cghidEventTap)
}

func point(_ x: String, _ y: String) -> CGPoint {
    guard let x = Double(x), let y = Double(y) else { fail("not a coordinate: \(x) \(y)") }
    return CGPoint(x: x, y: y)
}

struct Buttons {
    let button: CGMouseButton
    let down: CGEventType
    let up: CGEventType
    let dragged: CGEventType

    static func named(_ name: String) -> Buttons {
        switch name.lowercased() {
        case "left":
            return Buttons(button: .left, down: .leftMouseDown, up: .leftMouseUp, dragged: .leftMouseDragged)
        case "right":
            return Buttons(button: .right, down: .rightMouseDown, up: .rightMouseUp, dragged: .rightMouseDragged)
        case "middle":
            return Buttons(button: .center, down: .otherMouseDown, up: .otherMouseUp, dragged: .otherMouseDragged)
        default:
            fail("unknown button: \(name)")
        }
    }
}

func mouse(_ type: CGEventType, _ at: CGPoint, _ buttons: Buttons, clicks: Int64 = 1) {
    let event = CGEvent(
        mouseEventSource: source, mouseType: type,
        mouseCursorPosition: at, mouseButton: buttons.button)
    // How macOS distinguishes a double click from two clicks: the count is on
    // the event, not in the timing between two of them.
    event?.setIntegerValueField(.mouseEventClickState, value: clicks)
    post(event)
}

/// Virtual keycodes for an ANSI layout.
let keycodes: [String: CGKeyCode] = [
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9,
    "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19,
    "3": 20, "4": 21, "6": 22, "5": 23, "=": 24, "9": 25, "7": 26, "-": 27, "8": 28,
    "0": 29, "]": 30, "o": 31, "u": 32, "[": 33, "i": 34, "p": 35, "l": 37, "j": 38,
    "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43, "/": 44, "n": 45, "m": 46, ".": 47,
    "return": 36, "enter": 36, "tab": 48, "space": 49, "backspace": 51, "delete": 51,
    "escape": 53, "esc": 53, "del": 117, "forwarddelete": 117,
    "left": 123, "right": 124, "down": 125, "up": 126,
    "home": 115, "end": 119, "pageup": 116, "pgup": 116, "pagedown": 121, "pgdn": 121,
    "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97, "f7": 98, "f8": 100,
    "f9": 101, "f10": 109, "f11": 103, "f12": 111,
]

func modifier(_ name: String) -> CGEventFlags? {
    switch name.lowercased() {
    case "cmd", "command", "meta", "super", "win": return .maskCommand
    case "ctrl", "control": return .maskControl
    case "alt", "option": return .maskAlternate
    case "shift": return .maskShift
    default: return nil
    }
}

/// `cmd+shift+p` as the flags to hold and the key to strike.
func chord(_ input: String) {
    var flags: CGEventFlags = []
    var key: CGKeyCode?

    for part in input.split(separator: "+").map({ $0.trimmingCharacters(in: .whitespaces) }) {
        if part.isEmpty { continue }
        if let held = modifier(part) {
            flags.insert(held)
        } else if let code = keycodes[part.lowercased()] {
            key = code
        } else {
            fail("unknown key: \(part)")
        }
    }

    guard let key else { fail("no key in chord: \(input)") }

    for down in [true, false] {
        let event = CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: down)
        event?.flags = flags
        post(event)
    }
}

/// Text as the characters themselves, not as keystrokes.
func type(_ text: String) {
    // In chunks: the field takes a bounded string, and a long paste sent as
    // one event arrives truncated.
    for chunk in Array(text).chunked(into: 16) {
        let piece = String(chunk)
        for down in [true, false] {
            guard let event = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: down)
            else { fail("the event could not be created") }
            let utf16 = Array(piece.utf16)
            event.keyboardSetUnicodeString(stringLength: utf16.count, unicodeString: utf16)
            post(event)
        }
    }
}

extension Array {
    func chunked(into size: Int) -> [[Element]] {
        stride(from: 0, to: count, by: size).map { Array(self[$0..<Swift.min($0 + size, count)]) }
    }
}

let arguments = Array(CommandLine.arguments.dropFirst())
guard let verb = arguments.first else { fail("usage: computer-input <verb> [...]") }
let rest = Array(arguments.dropFirst())

switch verb {
case "move":
    guard rest.count == 2 else { fail("usage: move X Y") }
    post(CGEvent(
        mouseEventSource: source, mouseType: .mouseMoved,
        mouseCursorPosition: point(rest[0], rest[1]), mouseButton: .left))

case "click", "double":
    guard rest.count == 3 else { fail("usage: \(verb) X Y BUTTON") }
    let at = point(rest[0], rest[1])
    let buttons = Buttons.named(rest[2])
    let clicks: Int64 = verb == "double" ? 2 : 1

    // Moved first: a click at a position the pointer is not at leaves hover
    // state behind, and a menu that never opened swallows the click.
    mouse(.mouseMoved, at, buttons)
    for count in 1...clicks {
        mouse(buttons.down, at, buttons, clicks: count)
        mouse(buttons.up, at, buttons, clicks: count)
    }

case "drag":
    guard rest.count == 5 else { fail("usage: drag X1 Y1 X2 Y2 BUTTON") }
    let from = point(rest[0], rest[1])
    let to = point(rest[2], rest[3])
    let buttons = Buttons.named(rest[4])

    mouse(.mouseMoved, from, buttons)
    mouse(buttons.down, from, buttons)
    // Through the middle: an application tracking motion sees nothing in a
    // drag that teleports.
    for step in 1...8 {
        let fraction = Double(step) / 8.0
        mouse(
            buttons.dragged,
            CGPoint(
                x: from.x + (to.x - from.x) * fraction,
                y: from.y + (to.y - from.y) * fraction),
            buttons)
    }
    mouse(buttons.up, to, buttons)

case "scroll":
    guard rest.count == 3, let notches = Int32(rest[2]) else { fail("usage: scroll X Y NOTCHES") }
    let at = point(rest[0], rest[1])
    mouse(.mouseMoved, at, Buttons.named("left"))
    // Negative scrolls down on this API, and positive dy means down for this
    // crate, so the sign is flipped exactly once and here.
    post(CGEvent(
        scrollWheelEvent2Source: source, units: .line, wheelCount: 1,
        wheel1: -notches, wheel2: 0, wheel3: 0))

case "type":
    guard rest.count == 1 else { fail("usage: type TEXT") }
    type(rest[0])

case "key":
    guard rest.count == 1 else { fail("usage: key CHORD") }
    chord(rest[0])

case "cursor":
    guard let event = CGEvent(source: nil) else { fail("the cursor could not be read") }
    print("X=\(Int(event.location.x)) Y=\(Int(event.location.y))")

case "geometry":
    guard let mode = CGDisplayCopyDisplayMode(CGMainDisplayID()) else {
        fail("the display mode could not be read")
    }
    // The backing store, not `CGDisplayPixelsWide`: that one reports the
    // mode's point size, so on a 2x display it equals the bounds and a check
    // written against it never fires.
    let pixels = (mode.pixelWidth, mode.pixelHeight)
    let points = (mode.width, mode.height)

    // The whole coordinate contract: a screenshot returns pixels and CGEvent
    // takes points. On a 2x display every click lands at half its coordinate
    // with nothing reporting it, so a scaled display is refused rather than
    // driven.
    guard pixels == points else {
        fail("display is \(pixels.0)x\(pixels.1) pixels but \(points.0)x\(points.1) points: pin it to 1x")
    }
    print("\(pixels.0) \(pixels.1)")

default:
    fail("unknown verb: \(verb)")
}
