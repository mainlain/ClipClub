# ClipClub and 좋아 (✨) artwork

`fireworks.png` is the original **1254 × 1254** transparent PNG supplied by the
user. The desktop application and token use this same multicolor fireworks
artwork. The token name remains **좋아** and its ticker remains **✨**.

| File | Dimensions | Use |
| --- | --- | --- |
| `fireworks.png` | 1254 × 1254 | Unmodified source artwork |
| `icon.png` | 512 × 512 | Linux desktop icon |
| `joah-48.png` | 48 × 48 | Small token logo export |
| `joah-64.png` | 64 × 64 | Small token logo export |
| `joah-256.png` | 256 × 256 | Embedded application logo, favicon, deployment UI, and wallet image |
| `joah-512.png` | 512 × 512 | Larger token logo export |

All exported PNGs use sRGB and preserve the source's transparent background.
Every size is rendered directly from the original image, without cropping,
background compositing, or upscaling. The former SVG artwork has been replaced
by these PNG assets. No explorer submission or public hosting is performed.

## Reproduce and verify

On macOS, from the project root:

```sh
swift -module-cache-path .build/joah-module-cache scripts/export-joah.swift
swift -module-cache-path .build/swift-module-cache scripts/make-icon.swift .build/clipclub.iconset
python3 scripts/make-icns.py .build/clipclub.iconset .build/clipclub.icns
```

The token exporter reloads every PNG to check its dimensions, format, alpha
channel, transparent corners, and visible center. The macOS icon exporter
creates all ten iconset representations from 16 through 1024 pixels; packaging
places them in the application's `clipclub.icns` resource. Linux packages use
`icon.png`, including the Debian `hicolor/512x512/apps` icon.
