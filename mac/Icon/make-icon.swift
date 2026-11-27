// SPDX-License-Identifier: GPL-2.0-or-later
// Draws the app icon's pictures from the SVGs beside it, so they can be made again:
//
//     swiftc -O Icon/make-icon.swift -o /tmp/make-icon && /tmp/make-icon Icon App/Assets.xcassets
//
// It writes AppIcon.appiconset (every size macOS asks for, 16 to 1024, at 1x and 2x) and the
// menu bar's template image. Each picture is drawn from the SVG at its own size, not scaled down
// from one big one, and the small ones from AppIcon-small.svg.
import AppKit

let arguments = CommandLine.arguments
guard arguments.count == 3 else {
    FileHandle.standardError.write(Data("usage: make-icon ICON_FOLDER CATALOG_FOLDER\n".utf8))
    exit(2)
}
let source = URL(fileURLWithPath: arguments[1])
let catalog = URL(fileURLWithPath: arguments[2])

/// Sizes at or below this many pixels use the small drawing.
let smallest = 32

func image(_ name: String) -> NSImage {
    guard let picture = NSImage(contentsOf: source.appending(path: name)) else {
        fatalError("cannot read \(name)")
    }
    return picture
}

/// `picture` drawn into a square of `pixels` by `pixels`, with transparency.
func png(_ picture: NSImage, pixels: Int) -> Data {
    let bitmap = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels, bitsPerSample: 8,
        samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
        bytesPerRow: 0, bitsPerPixel: 0)!
    bitmap.size = NSSize(width: pixels, height: pixels)
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
    NSGraphicsContext.current?.imageInterpolation = .high
    picture.draw(in: NSRect(x: 0, y: 0, width: pixels, height: pixels))
    NSGraphicsContext.restoreGraphicsState()
    return bitmap.representation(using: .png, properties: [:])!
}

func write(_ data: Data, _ path: String) throws {
    let url = catalog.appending(path: path)
    try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    try data.write(to: url)
}

let large = image("AppIcon.svg"), small = image("AppIcon-small.svg")
var entries: [[String: String]] = []
for points in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let pixels = points * scale
        let file = "icon_\(points)x\(points)\(scale == 2 ? "@2x" : "").png"
        try write(png(pixels <= smallest ? small : large, pixels: pixels), "AppIcon.appiconset/\(file)")
        entries.append(["idiom": "mac", "size": "\(points)x\(points)", "scale": "\(scale)x", "filename": file])
    }
}
let contents: [String: Any] = ["images": entries, "info": ["version": 1, "author": "xcode"]]
try write(try JSONSerialization.data(withJSONObject: contents, options: [.prettyPrinted, .sortedKeys]),
          "AppIcon.appiconset/Contents.json")

// The menu bar's template image, at 1x and 2x of its 18 points.
let bar = image("MenuBar.svg")
var barEntries: [[String: String]] = []
for scale in [1, 2] {
    let file = "menubar@\(scale)x.png"
    try write(png(bar, pixels: 18 * scale), "MenuBarIcon.imageset/\(file)")
    barEntries.append(["idiom": "universal", "scale": "\(scale)x", "filename": file])
}
let barContents: [String: Any] = [
    "images": barEntries, "info": ["version": 1, "author": "xcode"],
    "properties": ["template-rendering-intent": "template"],
]
try write(try JSONSerialization.data(withJSONObject: barContents, options: [.prettyPrinted, .sortedKeys]),
          "MenuBarIcon.imageset/Contents.json")
try write(Data("{\n  \"info\" : { \"version\" : 1, \"author\" : \"xcode\" }\n}\n".utf8), "Contents.json")
