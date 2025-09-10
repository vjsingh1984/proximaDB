# Brand Assets

This folder contains the ProximaDB brand assets in SVG format. Variants are provided for different sizes, backgrounds, and usage (print/web/apps).

## Logo Variants

- Symbol-only (small sizes): `logos/proximadb-symbol.svg`
- Wordmark lockup (default): `logos/proximadb-logo.svg`
- Flat ring (small sizes): use `proximadb-symbol.svg` (flat ring) instead of gradient ring
- Monochrome (print/emboss): `logos/proximadb-logo-mono.svg`
- Inverted (dark backgrounds): `logos/proximadb-logo-inverted.svg`

## Icons / Favicons

- Favicon symbol (SVG): `icons/favicon-symbol.svg`
- Recommended PNG exports: 16×16, 32×32, 48×48, 180×180

### Export (local examples)

Using Inkscape or rsvg:

```
# 16/32/48 PNGs
inkscape logos/proximadb-symbol.svg -w 48 -h 48 -o icons/favicon-48.png
inkscape logos/proximadb-symbol.svg -w 32 -h 32 -o icons/favicon-32.png
inkscape logos/proximadb-symbol.svg -w 16 -h 16 -o icons/favicon-16.png

# 180 PNG (Apple Touch)
inkscape logos/proximadb-symbol.svg -w 180 -h 180 -o icons/apple-touch-icon-180.png

# ICO (requires ImageMagick)
convert icons/favicon-16.png icons/favicon-32.png icons/favicon-48.png icons/favicon.ico
```

## Cover Exports

- Standard: `proximadb_cover.svg` → 1200×600 PNG
- Retina: 2400×1200 PNG

Examples:

```
inkscape proximadb_cover.svg -w 1200 -h 600 -o proximadb_cover_1200x600.png
inkscape proximadb_cover.svg -w 2400 -h 1200 -o proximadb_cover_2400x1200.png
```

## Usage Guidelines

- For tiny sizes (≤ 32 px), prefer the symbol-only flat ring variant.
- On dark backgrounds, use the inverted logo.
- Keep outline strokes visible (avoid placing over busy imagery).
- Maintain clear space around the symbol equal to the ring thickness.

