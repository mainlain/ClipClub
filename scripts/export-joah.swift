// Render the canonical SVG directly with macOS's native vector renderer.
// Usage (from the project root):
// swift -module-cache-path .build/joah-module-cache scripts/export-joah.swift
// No fonts, network, packages, background compositing, or raster upscaling.
import AppKit
import Foundation
import CoreGraphics
import ImageIO

let project = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
let assets = project.appendingPathComponent("assets", isDirectory: true)
let source = assets.appendingPathComponent("joah.svg")
let svg = try String(contentsOf: source, encoding: .utf8)
guard let rootEnd = svg.firstIndex(of: ">") else { fatalError("Invalid canonical SVG") }
let header = String(svg[...rootEnd])
guard header.contains("width=\"256\"") && header.contains("height=\"256\""),
      header.contains("viewBox=\"0 0 64 64\""),
      let image = NSImage(contentsOf: source), image.size == NSSize(width: 256, height: 256) else {
    fatalError("The canonical logo must have explicit 256×256 dimensions and its original 64-unit viewBox")
}
image.cacheMode = .never
let explorerHeader = header.replacingOccurrences(of: "width=\"256\"", with: "width=\"32\"")
    .replacingOccurrences(of: "height=\"256\"", with: "height=\"32\"")
let explorerSVG = explorerHeader + svg[svg.index(after: rootEnd)...]
try explorerSVG.write(to: assets.appendingPathComponent("joah-32.svg"), atomically: true, encoding: .utf8)

func verifyPNG(_ url: URL, size: Int) throws {
    let data = try Data(contentsOf: url)
    guard data.count < 100_000,
          let png = CGImageSourceCreateWithData(data as CFData, nil),
          CGImageSourceGetType(png) as String? == "public.png",
          let decoded = CGImageSourceCreateImageAtIndex(png, 0, nil),
          decoded.width == size, decoded.height == size,
          decoded.alphaInfo != .none, decoded.alphaInfo != .noneSkipFirst, decoded.alphaInfo != .noneSkipLast,
          let colorSpace = CGColorSpace(name: CGColorSpace.sRGB),
          let inspection = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8,
            bytesPerRow: size * 4, space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue),
          let bytes = inspection.data?.assumingMemoryBound(to: UInt8.self) else {
        fatalError("PNG dimensions, format, alpha, or file-size verification failed: \(url.lastPathComponent)")
    }
    inspection.draw(decoded, in: CGRect(x: 0, y: 0, width: size, height: size))
    for (x, y) in [(0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)] {
        guard bytes[(y * size + x) * 4 + 3] == 0 else { fatalError("Logo corners must remain transparent") }
    }
    guard bytes[((size / 2) * size + size / 2) * 4 + 3] == 255 else {
        fatalError("The central fireworks badge must remain opaque")
    }
    print("\(url.lastPathComponent): \(decoded.width)×\(decoded.height) RGBA PNG, transparent corners, \(data.count) bytes")
}
for size in [48, 64, 256, 512] {
    guard let colorSpace = CGColorSpace(name: CGColorSpace.sRGB),
          let context = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8,
            bytesPerRow: size * 4, space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
        fatalError("Cannot create transparent sRGB canvas")
    }
    context.clear(CGRect(x: 0, y: 0, width: size, height: size))
    context.setAllowsAntialiasing(true)
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: false)
    // _NSSVGImageRep remains vector-backed and draws at each requested output size.
    image.draw(in: NSRect(x: 0, y: 0, width: size, height: size), from: .zero, operation: .copy, fraction: 1)
    NSGraphicsContext.restoreGraphicsState()
    let target = assets.appendingPathComponent("joah-\(size).png")
    guard let rendered = context.makeImage(),
          let destination = CGImageDestinationCreateWithURL(target as CFURL, "public.png" as CFString, 1, nil) else {
        fatalError("Cannot create PNG output")
    }
    CGImageDestinationAddImage(destination, rendered, nil)
    guard CGImageDestinationFinalize(destination) else { fatalError("Cannot finish PNG output") }
    try verifyPNG(target, size: size)
}
print("joah.svg: 256×256 vector master; joah-32.svg: 32×32 explorer vector")
