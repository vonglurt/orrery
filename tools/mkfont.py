#!/usr/bin/env python3
"""Turn an X11 PCF bitmap font into src/font.rs.

Run once, by hand, when the font changes -- which is approximately never. The
generated file is committed, because a node compiling orrery has no font
package, no Python and no internet, and `copal-build` must not need any of
them.

    python3 tools/mkfont.py 8x13.pcf.gz 10x20.pcf.gz > src/font.rs

PCF is a binary format with a table of contents at the front. The three tables
that matter here are BITMAPS (the pixels), METRICS (how wide each glyph is)
and BDF_ENCODINGS (which codepoint maps to which glyph index). Everything else
is skipped.
"""
import gzip
import struct
import sys

PCF_PROPERTIES = 1
PCF_METRICS = 4
PCF_BITMAPS = 8
PCF_ACCELERATORS = 2
PCF_BDF_ENCODINGS = 32

PCF_COMPRESSED_METRICS = 0x100
PCF_BYTE_MASK = 4          # set: big-endian
PCF_BIT_MASK = 8           # set: most significant bit is leftmost pixel
PCF_GLYPH_PAD_MASK = 3     # rows padded to 1, 2, 4 or 8 bytes

# The range baked into the binary: printable ASCII through Latin-1, which
# covers everything the console prints. The two that earn their keep past ASCII
# are U+00B0 DEGREE SIGN and U+00B7 MIDDLE DOT, both of which the wall uses.
FIRST, LAST = 0x20, 0xFF

# A short list of characters past Latin-1 that the console genuinely prints.
# Kept explicit and tiny rather than baking a range: each of these earns its
# place, and a font that silently lacks one is a bug that shows up as a gap.
#
#   U+2026  the truncation marker. Without it `text_in` cuts a string and says
#           nothing, which is worse than either cutting or not cutting.
#   U+2192  "ran:", pointing at what a verb would do.
#   U+25CF  a node that is there.
#   U+25CB  a node that is only declared.
#   U+2580  the seat's half block, so the native face can draw a framebuffer
#           the same way the terminal one does.
EXTRAS = [0x2026, 0x2192, 0x25CF, 0x25CB, 0x2580]


def read(path):
    opener = gzip.open if path.endswith(".gz") else open
    with opener(path, "rb") as fh:
        return fh.read()


class Pcf:
    def __init__(self, data):
        if data[:4] != b"\x01fcp":
            raise SystemExit("%s: not a PCF font" % path)
        (count,) = struct.unpack_from("<i", data, 4)
        self.data = data
        self.tables = {}
        for i in range(count):
            kind, fmt, size, offset = struct.unpack_from("<4i", data, 8 + i * 16)
            self.tables[kind] = (fmt, size, offset)

    def table(self, kind):
        if kind not in self.tables:
            raise SystemExit("font has no table %d" % kind)
        fmt, size, offset = self.tables[kind]
        # Each table repeats its format as its first word.
        (stored,) = struct.unpack_from("<i", self.data, offset)
        return stored, offset + 4

    def properties(self):
        fmt, at = self.table(PCF_PROPERTIES)
        e = ">" if fmt & PCF_BYTE_MASK else "<"
        (n,) = struct.unpack_from(e + "i", self.data, at)
        at += 4
        props = []
        for i in range(n):
            name_off, is_str, value = struct.unpack_from(e + "iBi", self.data, at + i * 9)
            props.append((name_off, is_str, value))
        at += n * 9
        at += (4 - (n * 9) % 4) % 4          # pad to a word
        (str_size,) = struct.unpack_from(e + "i", self.data, at)
        at += 4
        blob = self.data[at:at + str_size]

        def cstr(off):
            end = blob.index(b"\0", off)
            return blob[off:end].decode("latin-1")

        out = {}
        for name_off, is_str, value in props:
            name = cstr(name_off)
            out[name] = cstr(value) if is_str else value
        return out

    def accelerators(self):
        """Font-wide ascent and descent, which are not properties."""
        fmt, at = self.table(PCF_ACCELERATORS)
        e = ">" if fmt & PCF_BYTE_MASK else "<"
        # Eight one-byte flags come first, then the two we want.
        at += 8
        ascent, descent = struct.unpack_from(e + "2i", self.data, at)
        return ascent, descent

    def metrics(self):
        fmt, at = self.table(PCF_METRICS)
        e = ">" if fmt & PCF_BYTE_MASK else "<"
        if fmt & PCF_COMPRESSED_METRICS:
            (n,) = struct.unpack_from(e + "h", self.data, at)
            at += 2
            out = []
            for i in range(n):
                lb, rb, w, asc, desc = struct.unpack_from("5B", self.data, at + i * 5)
                # Compressed metrics are stored biased by 0x80.
                out.append((lb - 0x80, rb - 0x80, w - 0x80, asc - 0x80, desc - 0x80))
            return out
        (n,) = struct.unpack_from(e + "i", self.data, at)
        at += 4
        out = []
        for i in range(n):
            lb, rb, w, asc, desc, _attr = struct.unpack_from(e + "5hH", self.data, at + i * 12)
            out.append((lb, rb, w, asc, desc))
        return out

    def encodings(self):
        fmt, at = self.table(PCF_BDF_ENCODINGS)
        e = ">" if fmt & PCF_BYTE_MASK else "<"
        min2, max2, min1, max1, default = struct.unpack_from(e + "5h", self.data, at)
        at += 10
        table = {}
        rows = max1 - min1 + 1
        cols = max2 - min2 + 1
        for r in range(rows):
            for c in range(cols):
                (idx,) = struct.unpack_from(e + "H", self.data, at + (r * cols + c) * 2)
                if idx != 0xFFFF:
                    code = ((min1 + r) << 8) | (min2 + c) if max1 > 0 else (min2 + c)
                    table[code] = idx
        return table

    def bitmaps(self):
        fmt, at = self.table(PCF_BITMAPS)
        e = ">" if fmt & PCF_BYTE_MASK else "<"
        (n,) = struct.unpack_from(e + "i", self.data, at)
        at += 4
        offsets = list(struct.unpack_from(e + "%di" % n, self.data, at))
        at += n * 4
        sizes = struct.unpack_from(e + "4i", self.data, at)
        at += 16
        data = self.data[at:at + sizes[fmt & PCF_GLYPH_PAD_MASK]]
        pad = 1 << (fmt & PCF_GLYPH_PAD_MASK)
        msb_first = bool(fmt & PCF_BIT_MASK)
        return offsets, data, pad, msb_first


def glyph_rows(pcf, index, height, width):
    """One glyph as a list of `height` ints, bit 0 = leftmost pixel."""
    offsets, data, pad, msb_first = pcf._bitmaps
    start = offsets[index]
    stride = ((width + 7) // 8 + pad - 1) // pad * pad
    rows = []
    for y in range(height):
        bits = 0
        base = start + y * stride
        for x in range(width):
            byte = data[base + (x >> 3)]
            bit = (byte >> (7 - (x & 7))) & 1 if msb_first else (byte >> (x & 7)) & 1
            if bit:
                bits |= 1 << x
        rows.append(bits)
    return rows


def convert(path):
    pcf = Pcf(read(path))
    pcf._bitmaps = pcf.bitmaps()
    props = pcf.properties()
    metrics = pcf.metrics()
    enc = pcf.encodings()

    ascent, descent = pcf.accelerators()
    height = ascent + descent

    # Take the cell width from a glyph that is certainly full width.
    idx = enc.get(ord("M"))
    if idx is None:
        raise SystemExit("%s: no 'M' to measure" % path)
    width = metrics[idx][2]

    glyphs = []
    missing = []
    for code in list(range(FIRST, LAST + 1)) + EXTRAS:
        i = enc.get(code)
        if i is None:
            if code in EXTRAS:
                missing.append(code)
            glyphs.append([0] * height)
            continue
        glyphs.append(glyph_rows(pcf, i, height, width))
    if missing:
        raise SystemExit("%s lacks %s -- pick another face or drop it from EXTRAS"
                         % (path, ", ".join("U+%04X" % c for c in missing)))

    return {
        "path": path,
        "width": width,
        "height": height,
        "ascent": ascent,
        "glyphs": glyphs,
        "copyright": props.get("COPYRIGHT", "(none stated)"),
        "family": props.get("FAMILY_NAME", "?"),
    }


def emit(fonts):
    out = []
    w = out.append
    w("//! Bitmap glyphs, baked in.")
    w("//!")
    w("//! GENERATED BY tools/mkfont.py -- do not edit by hand.")
    w("//!")
    w("//! A node has no font package, no fontconfig and no internet, so the")
    w("//! console carries its letters with it. That is not a compromise here:")
    w("//! a fixed bitmap font at a fixed size is exactly what a wall of small")
    w("//! labels wants, and it renders identically on every machine rather")
    w("//! than depending on what happens to be installed.")
    w("//!")
    w("//! Source: the X.Org \"misc-fixed\" fonts, whose own COPYRIGHT property")
    w("//! is reproduced for each face below.")
    for f in fonts:
        w("//!")
        w("//!   %-6s %dx%d  %s" % (f["path"].split("/")[-1].split(".")[0],
                                    f["width"], f["height"], f["family"]))
        w("//!            %s" % f["copyright"])
    w("")
    w("/// One face: a dense block for %d..=%d, then EXTRAS in order." % (FIRST, LAST))
    w("pub struct Font {")
    w("    pub width: usize,")
    w("    pub height: usize,")
    w("    /// Rows from the top. Bit 0 is the leftmost pixel.")
    w("    pub glyphs: &'static [[u16; %d]]," % max(f["height"] for f in fonts))
    w("    pub first: u8,")
    w("}")
    w("")
    w("/// The code points baked past Latin-1, in the order they are stored.")
    w("pub static EXTRAS: [u32; %d] = [%s];"
      % (len(EXTRAS), ", ".join("0x%04x" % c for c in EXTRAS)))
    w("")
    w("impl Font {")
    w("    /// The rows for one character, or the blank cell for anything")
    w("    /// outside the baked range -- a console must not panic on a stray")
    w("    /// byte from a node's own output.")
    w("    pub fn glyph(&self, c: char) -> &'static [u16] {")
    w("        let u = c as u32;")
    w("        if u >= self.first as u32 && u <= %d {" % LAST)
    w("            let row = &self.glyphs[(u - self.first as u32) as usize];")
    w("            return &row[..self.height];")
    w("        }")
    w("        // A handful of characters live past the dense block. The scan is")
    w("        // over five entries and only runs for non-Latin-1 text.")
    w("        let dense = (%d - self.first as u32 + 1) as usize;" % LAST)
    w("        if let Some(i) = EXTRAS.iter().position(|e| *e == u) {")
    w("            let row = &self.glyphs[dense + i];")
    w("            return &row[..self.height];")
    w("        }")
    w("        &BLANK[..self.height]")
    w("    }")
    w("")
    w("    pub fn advance(&self) -> usize {")
    w("        self.width")
    w("    }")
    w("")
    w("    /// How wide `s` will be, in pixels.")
    w("    pub fn measure(&self, s: &str) -> usize {")
    w("        s.chars().count() * self.width")
    w("    }")
    w("}")
    w("")
    maxh = max(f["height"] for f in fonts)
    w("static BLANK: [u16; %d] = [0; %d];" % (maxh, maxh))

    for f in fonts:
        name = "F%dX%d" % (f["width"], f["height"])
        w("")
        w("static %s_GLYPHS: [[u16; %d]; %d] = [" % (name, maxh, len(f["glyphs"])))
        for code, rows in zip(list(range(FIRST, LAST + 1)) + EXTRAS, f["glyphs"]):
            ch = chr(code)
            label = ch if 33 <= code <= 126 and ch not in "\\" else "U+%04X" % code
            padded = list(rows) + [0] * (maxh - len(rows))
            w("    [%s], // %s" % (", ".join("0x%04x" % r for r in padded), label))
        w("];")
        w("")
        w("/// %dx%d." % (f["width"], f["height"]))
        w("pub static %s: Font = Font {" % name)
        w("    width: %d," % f["width"])
        w("    height: %d," % f["height"])
        w("    glyphs: &%s_GLYPHS," % name)
        w("    first: %d," % FIRST)
        w("};")

    return "\n".join(out) + "\n"


if __name__ == "__main__":
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    fonts = [convert(p) for p in sys.argv[1:]]
    for f in fonts:
        sys.stderr.write("%s: %dx%d, ascent %d -- %s\n"
                         % (f["path"], f["width"], f["height"], f["ascent"], f["copyright"]))
    sys.stdout.write(emit(fonts))
