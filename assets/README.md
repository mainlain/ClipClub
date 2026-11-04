# 좋아 (✨) token logo

The token uses the existing fireworks mark. `joah.svg` is the canonical vector
source with explicit **256 × 256** intrinsic dimensions and a `0 0 64 64`
viewBox. The artwork has no fonts, embedded raster images, or external assets.
The app can continue displaying this vector at 32 pixels without losing detail.
`icon.svg` remains the separate ClipClub application mark.

| File | Dimensions | Format | Use |
| --- | --- | --- | --- |
| `joah.svg` | 256 × 256 | SVG, 615 bytes | Canonical source; MegaETH token-list logo |
| `joah-32.svg` | 32 × 32 | SVG, 613 bytes | Etherscan explorer vector submission |
| `joah-48.png` | 48 × 48 | RGBA PNG, 2,263 bytes | Blockscout icon submission |
| `joah-64.png` | 64 × 64 | RGBA PNG, 2,603 bytes | Etherscan explorer PNG submission |
| `joah-256.png` | 256 × 256 | RGBA PNG, 10,821 bytes | MegaETH token-list PNG; general wallet image |
| `joah-512.png` | 512 × 512 | RGBA PNG, 25,482 bytes | Larger reusable raster export |

All PNGs use sRGB, retain transparent corners around the original dark rounded
badge, and remain below 100 KB. The dark badge itself is intentionally opaque
so the fireworks remain visible on light and dark surfaces. The ticker remains
✨; it is separate from the fireworks artwork.

## Submission sizes

- **MegaETH:** its official token registry recommends 256 × 256 SVG or PNG and
  specifies a minimum of 200 × 200. Use `joah.svg` or `joah-256.png`.
  [MegaETH token registry](https://github.com/megaeth-labs/mega-tokenlist)
- **Etherscan / MegaETH Etherscan:** the published token-info guide specifies
  32 × 32 SVG or 64 × 64 PNG. Use `joah-32.svg` or `joah-64.png`.
  [Etherscan token submission guide](https://kb.etherscan.com/token-update-guide)
- **Blockscout:** its published token-info instructions specify a 48 × 48 SVG
  or PNG icon URL. Use `joah-48.png`.
  [Blockscout token information guide](https://docs.blockscout.com/using-blockscout/token-info)

Guidance checked on November 3, 2026. These files have been prepared locally;
no explorer submission or public hosting was performed.

## Reproduce and verify

On macOS, from the project root:

```sh
swift -module-cache-path .build/joah-module-cache scripts/export-joah.swift
```

This uses macOS's native SVG renderer to render each PNG directly from
`joah.svg`, with raster caching disabled. It also creates the 32-pixel SVG by
changing only its intrinsic dimensions. No PNG is enlarged from a smaller PNG.

The exporter reloads every written PNG and checks its actual pixel dimensions,
PNG format, alpha channel, fully transparent corners, opaque center, and file
size. Two consecutive runs produced byte-identical PNGs. The 256- and 64-pixel
exports were visually inspected against the original fireworks mark.
