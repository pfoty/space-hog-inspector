// Draws the Space Hog Inspector app icon (a treemap with a pig snout)
// and writes it as a 1024x1024 PNG to the path given as the first argument.
import AppKit

let size: CGFloat = 1024
let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "icon_1024.png"

let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: Int(size), pixelsHigh: Int(size),
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

func color(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat) -> NSColor {
    NSColor(srgbRed: r / 255, green: g / 255, blue: b / 255, alpha: 1)
}

// Rounded-square background, inset like standard macOS icons.
let inset: CGFloat = 100
let bg = NSRect(x: inset, y: inset, width: size - 2 * inset, height: size - 2 * inset)
let bgPath = NSBezierPath(roundedRect: bg, xRadius: 185, yRadius: 185)
NSGradient(starting: color(44, 52, 72), ending: color(22, 26, 38))!.draw(in: bgPath, angle: -90)

// Treemap tiles.
let pad: CGFloat = 70
let area = bg.insetBy(dx: pad, dy: pad)
let gap: CGFloat = 14
func tile(_ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat, _ c: NSColor) {
    let r = NSRect(x: area.minX + x * area.width + gap / 2, y: area.minY + y * area.height + gap / 2,
                   width: w * area.width - gap, height: h * area.height - gap)
    c.setFill()
    NSBezierPath(roundedRect: r, xRadius: 26, yRadius: 26).fill()
}
// Big pink "hog" tile on the left, smaller tiles on the right.
tile(0.0, 0.0, 0.62, 1.0, color(240, 140, 170))
tile(0.62, 0.55, 0.38, 0.45, color(96, 164, 232))
tile(0.62, 0.25, 0.22, 0.30, color(110, 196, 124))
tile(0.84, 0.25, 0.16, 0.30, color(222, 198, 84))
tile(0.62, 0.0, 0.38, 0.25, color(178, 132, 222))

// Pig snout in the big tile.
let cx = area.minX + 0.31 * area.width
let cy = area.minY + 0.5 * area.height
let snout = NSRect(x: cx - 150, y: cy - 110, width: 300, height: 220)
color(214, 98, 136).setFill()
NSBezierPath(ovalIn: snout).fill()
color(120, 40, 70).setFill()
NSBezierPath(ovalIn: NSRect(x: cx - 85, y: cy - 45, width: 55, height: 90)).fill()
NSBezierPath(ovalIn: NSRect(x: cx + 30, y: cy - 45, width: 55, height: 90)).fill()

NSGraphicsContext.restoreGraphicsState()
let data = rep.representation(using: .png, properties: [:])!
try! data.write(to: URL(fileURLWithPath: out))
print("wrote \(out)")
