// Draws the Space Hog Inspector app icon (the header's hexagon logo on a
// dark rounded square) and writes it as a 1024x1024 PNG to the path given as
// the first argument.
import AppKit

let size: CGFloat = 1024
let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "icon_1024.png"

let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: Int(size), pixelsHigh: Int(size),
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> NSColor {
    NSColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
}
let cyan = color(0x19E3FF)
let magenta = color(0xFF2E88)

// Rounded-square background, inset like standard macOS icons.
let inset: CGFloat = 100
let bg = NSRect(x: inset, y: inset, width: size - 2 * inset, height: size - 2 * inset)
let bgPath = NSBezierPath(roundedRect: bg, xRadius: 185, yRadius: 185)
color(0x0E1017).setFill()
bgPath.fill()
color(0x262A38).setStroke()
bgPath.lineWidth = 6
bgPath.stroke()

// The logo, on the same 32-unit grid the app draws it on (y grows downward
// there, so flip it here).
let logo: CGFloat = 600
let s = logo / 32
let origin = CGPoint(x: (size - logo) / 2, y: (size + logo) / 2)
func p(_ x: CGFloat, _ y: CGFloat) -> NSPoint { NSPoint(x: origin.x + x * s, y: origin.y - y * s) }

func glow(_ c: NSColor, _ blur: CGFloat, _ draw: () -> Void) {
    NSGraphicsContext.saveGraphicsState()
    let shadow = NSShadow()
    shadow.shadowColor = c
    shadow.shadowBlurRadius = blur
    shadow.shadowOffset = .zero
    shadow.set()
    draw()
    NSGraphicsContext.restoreGraphicsState()
}

let hex = NSBezierPath()
hex.move(to: p(16, 2))
for (x, y) in [(28.0, 9.0), (28.0, 23.0), (16.0, 30.0), (4.0, 23.0), (4.0, 9.0)] {
    hex.line(to: p(CGFloat(x), CGFloat(y)))
}
hex.close()
hex.lineWidth = 1.5 * s
hex.lineJoinStyle = .miter
cyan.setStroke()
glow(color(0x19E3FF, 0.8), 40) { hex.stroke() }
hex.stroke()

for (y, x2) in [(13.0, 22.0), (17.0, 18.0), (21.0, 15.0)] {
    let bar = NSBezierPath()
    bar.move(to: p(10, CGFloat(y)))
    bar.line(to: p(CGFloat(x2), CGFloat(y)))
    bar.lineWidth = 2 * s
    bar.lineCapStyle = .square
    magenta.setStroke()
    glow(color(0xFF2E88, 0.7), 30) { bar.stroke() }
    bar.stroke()
}

NSGraphicsContext.restoreGraphicsState()
let data = rep.representation(using: .png, properties: [:])!
try! data.write(to: URL(fileURLWithPath: out))
print("wrote \(out)")
