// Draws the app icon source, a 1024×1024 PNG: three stacked layers (the inheritance chain) on a
// rounded square that follows the macOS icon grid. Run: swift scripts/icon.swift <output.png>
import AppKit

let size = 1024.0
let output = CommandLine.arguments.dropFirst().first ?? "icon.png"
let space = CGColorSpace(name: CGColorSpace.sRGB)!
let ctx = CGContext(data: nil, width: Int(size), height: Int(size), bitsPerComponent: 8, bytesPerRow: 0,
                    space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!

func color(_ hex: UInt32, _ alpha: Double = 1) -> CGColor {
    CGColor(srgbRed: Double((hex >> 16) & 0xff) / 255, green: Double((hex >> 8) & 0xff) / 255,
            blue: Double(hex & 0xff) / 255, alpha: alpha)
}

// Background: 824 pt square with a 185 pt corner radius, a soft shadow and a diagonal gradient.
let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
let tilePath = CGPath(roundedRect: tile, cornerWidth: 185, cornerHeight: 185, transform: nil)
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: color(0x000000, 0.35))
ctx.addPath(tilePath)
ctx.setFillColor(color(0x1f3a5f))
ctx.fillPath()
ctx.restoreGState()
ctx.saveGState()
ctx.addPath(tilePath)
ctx.clip()
let background = CGGradient(colorsSpace: space, colors: [color(0x2b6cb0), color(0x16304f)] as CFArray,
                            locations: [0, 1])!
ctx.drawLinearGradient(background, start: CGPoint(x: 100, y: 924), end: CGPoint(x: 924, y: 100), options: [])
ctx.restoreGState()

// One layer: a rhombus seen from above, centred at `y`.
func layer(y: Double, fill: CGColor, edge: CGColor) {
    let cx = 512.0, half = 250.0, depth = 140.0
    let path = CGMutablePath()
    path.move(to: CGPoint(x: cx, y: y + depth))
    path.addLine(to: CGPoint(x: cx + half, y: y))
    path.addLine(to: CGPoint(x: cx, y: y - depth))
    path.addLine(to: CGPoint(x: cx - half, y: y))
    path.closeSubpath()
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 24, color: color(0x000000, 0.3))
    ctx.addPath(path)
    ctx.setFillColor(fill)
    ctx.fillPath()
    ctx.restoreGState()
    ctx.addPath(path)
    ctx.setStrokeColor(edge)
    ctx.setLineWidth(10)
    ctx.setLineJoin(.round)
    ctx.strokePath()
}

// Bottom to top: the root, a parent, and the preset itself (brightest, with its own accent edge).
layer(y: 372, fill: color(0x9cc3e8, 0.55), edge: color(0xffffff, 0.55))
layer(y: 512, fill: color(0xc6ddf3, 0.8), edge: color(0xffffff, 0.75))
layer(y: 652, fill: color(0xffffff), edge: color(0xf6ad55))

let image = ctx.makeImage()!
let rep = NSBitmapImageRep(cgImage: image)
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: output))
