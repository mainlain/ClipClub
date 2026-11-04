// Resize the user-supplied artwork directly for token logos and the Linux icon.
// Usage (from the project root):
// swift -module-cache-path .build/joah-module-cache scripts/export-joah.swift
import Foundation
import CoreGraphics
import ImageIO

let project = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
let assets = project.appendingPathComponent("assets", isDirectory: true)
let source = assets.appendingPathComponent("fireworks.png")
guard let input = CGImageSourceCreateWithURL(source as CFURL, nil),
      let artwork = CGImageSourceCreateImageAtIndex(input, 0, nil),
      artwork.width == artwork.height, artwork.width >= 1024 else {
    fatalError("The fireworks source must be a square PNG at least 1024 pixels wide")
}

func verifyPNG(_ url: URL, size: Int) throws {
    let data = try Data(contentsOf: url)
    guard let png = CGImageSourceCreateWithData(data as CFData, nil),
          CGImageSourceGetType(png) as String? == "public.png",
          let decoded = CGImageSourceCreateImageAtIndex(png, 0, nil),
          decoded.width == size, decoded.height == size,
          decoded.alphaInfo != .none, decoded.alphaInfo != .noneSkipFirst, decoded.alphaInfo != .noneSkipLast,
          let colorSpace = CGColorSpace(name: CGColorSpace.sRGB),
          let inspection = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8,
            bytesPerRow: size * 4, space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue),
          let bytes = inspection.data?.assumingMemoryBound(to: UInt8.self) else {
        fatalError("PNG dimensions, format, or alpha verification failed: \(url.lastPathComponent)")
    }
    inspection.draw(decoded, in: CGRect(x: 0, y: 0, width: size, height: size))
    for (x, y) in [(0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)] {
        guard bytes[(y * size + x) * 4 + 3] == 0 else { fatalError("Logo corners must remain transparent") }
    }
    guard bytes[((size / 2) * size + size / 2) * 4 + 3] > 0 else {
        fatalError("The central fireworks must remain visible")
    }
    print("\(url.lastPathComponent): \(size)×\(size) RGBA PNG, transparent corners, \(data.count) bytes")
}

for size in [48, 64, 256, 512] {
    guard let colorSpace = CGColorSpace(name: CGColorSpace.sRGB),
          let context = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8,
            bytesPerRow: size * 4, space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
        fatalError("Cannot create transparent sRGB canvas")
    }
    context.interpolationQuality = .high
    context.draw(artwork, in: CGRect(x: 0, y: 0, width: size, height: size))
    let target = assets.appendingPathComponent("joah-\(size).png")
    guard let rendered = context.makeImage(),
          let destination = CGImageDestinationCreateWithURL(target as CFURL, "public.png" as CFString, 1, nil) else {
        fatalError("Cannot create PNG output")
    }
    CGImageDestinationAddImage(destination, rendered, nil)
    guard CGImageDestinationFinalize(destination) else { fatalError("Cannot finish PNG output") }
    try verifyPNG(target, size: size)
}
let desktop = assets.appendingPathComponent("icon.png")
try Data(contentsOf: assets.appendingPathComponent("joah-512.png")).write(to: desktop, options: .atomic)
try verifyPNG(desktop, size: 512)
