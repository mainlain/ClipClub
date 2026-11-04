// Resize the shared fireworks artwork for every macOS icon representation.
import Foundation
import CoreGraphics
import ImageIO

guard CommandLine.arguments.count == 2 else {
    fatalError("Usage: swift scripts/make-icon.swift PATH.iconset")
}
let directory = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
let project = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
let source = project.appendingPathComponent("assets/fireworks.png")
guard let input = CGImageSourceCreateWithURL(source as CFURL, nil),
      let artwork = CGImageSourceCreateImageAtIndex(input, 0, nil),
      artwork.width == artwork.height, artwork.width >= 1024 else {
    fatalError("The fireworks source must be a square PNG at least 1024 pixels wide")
}
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
        context.interpolationQuality = .high
        context.draw(artwork, in: CGRect(x: 0, y: 0, width: pixels, height: pixels))
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
