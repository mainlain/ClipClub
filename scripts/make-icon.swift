// Original ClipCrop vector mark, rendered natively without external packages.
// Keep geometry in sync with assets/icon.svg.
import Foundation
import CoreGraphics
import ImageIO

guard CommandLine.arguments.count == 2 else {
    fatalError("Usage: swift scripts/make-icon.swift PATH.iconset")
}
let directory = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
for size in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let pixels = size * scale
        guard let context = CGContext(data: nil, width: pixels, height: pixels,
            bitsPerComponent: 8, bytesPerRow: pixels * 4,
            space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
            fatalError("Cannot create icon canvas")
        }
        context.translateBy(x: 0, y: CGFloat(pixels))
        context.scaleBy(x: CGFloat(pixels) / 1024, y: -CGFloat(pixels) / 1024)
        context.setAllowsAntialiasing(true)
        context.setFillColor(red: 1, green: 99.0 / 255, blue: 47.0 / 255, alpha: 1)
        context.addPath(CGPath(roundedRect: CGRect(x: 40, y: 40, width: 944, height: 944),
            cornerWidth: 210, cornerHeight: 210, transform: nil))
        context.fillPath()
        context.setStrokeColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
        context.setLineWidth(84)
        context.setLineCap(.round)
        context.setLineJoin(.round)
        context.move(to: CGPoint(x: 340, y: 210))
        context.addLine(to: CGPoint(x: 340, y: 644))
        context.addQuadCurve(to: CGPoint(x: 380, y: 684), control: CGPoint(x: 340, y: 684))
        context.addLine(to: CGPoint(x: 814, y: 684))
        context.strokePath()
        context.move(to: CGPoint(x: 210, y: 340))
        context.addLine(to: CGPoint(x: 644, y: 340))
        context.addQuadCurve(to: CGPoint(x: 684, y: 380), control: CGPoint(x: 684, y: 340))
        context.addLine(to: CGPoint(x: 684, y: 814))
        context.strokePath()
        let suffix = scale == 2 ? "@2x" : ""
        let file = directory.appendingPathComponent("icon_\(size)x\(size)\(suffix).png")
        guard let image = context.makeImage(),
              let destination = CGImageDestinationCreateWithURL(file as CFURL, "public.png" as CFString, 1, nil) else {
            fatalError("Cannot write icon image")
        }
        CGImageDestinationAddImage(destination, image, nil)
        guard CGImageDestinationFinalize(destination) else { fatalError("Cannot finish icon image") }
    }
}
