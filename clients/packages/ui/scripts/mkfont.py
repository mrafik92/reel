"""Build the numbered row's digits-only numeral font (#301).

Usage (any Python with `pip install fonttools brotli`):

    python mkfont.py 'Archivo[wdth,wght].ttf' ../src/fonts/archivo-rank-digits.woff2

The source is Archivo's variable font from https://github.com/google/fonts/tree/main/ofl/archivo
(SIL OFL 1.1, see ../src/fonts/OFL.txt). This pins it to a static instance, width 62 and weight 900,
so old TV browsers need no variable-font support, and keeps only the digits 0-9. It prints the
advance widths that `components/rank.ts` hard-codes; update them there if the source changes.
"""

import sys

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

src, out = sys.argv[1], sys.argv[2]
font = instancer.instantiateVariableFont(TTFont(src), {"wdth": 62, "wght": 900}, updateFontNames=False)
opts = subset.Options()
opts.flavor = "woff2"
opts.layout_features = []  # single numerals: no kerning or alternates
opts.name_IDs = [0, 1, 2, 3, 4, 5, 6, 13, 14]  # keep the copyright and licence records
opts.hinting = False
opts.notdef_outline = True
sub = subset.Subsetter(opts)
sub.populate(unicodes=[ord(c) for c in "0123456789"])
sub.subset(font)
subset.save_font(font, out, opts)

built = TTFont(out)
cmap, hmtx = built.getBestCmap(), built["hmtx"]
print("unitsPerEm", built["head"].unitsPerEm, "capHeight", built["OS/2"].sCapHeight)
print("advances 0-9", [hmtx[cmap[ord(c)]][0] for c in "0123456789"])
