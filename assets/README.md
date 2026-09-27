# Assets

Images and fonts the examples load rather than generate.

## `great-wave.jpg`

| | |
|---|---|
| Title | *The Great Wave off Kanagawa* (神奈川沖浪裏, *Kanagawa-oki nami ura*) |
| Artist | Katsushika Hokusai, c. 1831 |
| Source | <https://commons.wikimedia.org/wiki/File:The_Great_Wave_off_Kanagawa.jpg> |
| Licence | Public domain (`PD-old-100-expired`, CC-PD-Mark) |
| Here | The 960px-wide Wikimedia render, unmodified. 4335×2990 is 2.5 MB, and the example is not trying to show off the decoder. |

Public domain rather than CC0, which is a strictly weaker claim that happens to
come out the same way for a work this old: Hokusai died in 1849, so nothing here
is under copyright anywhere.

Loaded by `examples/photo.rs`, which needs the `image` feature. The file is
committed rather than downloaded at run time so the example works with no
network.

## `ascii.ttf`

| | |
|---|---|
| Title | Montserrat Regular, subset to printable ASCII |
| Designer | The Montserrat Project Authors, <https://github.com/JulietaUla/Montserrat> |
| Source | `Montserrat-Regular.ttf` from the `fonts-montserrat` package |
| Licence | SIL Open Font License 1.1, reproduced in the font's name table as name IDs 13 and 14 |
| Here | 104 glyphs, 18 KB, from the 435 KB original |

Loaded by `examples/text.rs`, which needs no `image` feature. Committed rather
than loaded from the system so the example renders the same thing everywhere,
and subset rather than shipped whole because the example draws Latin letters
and a subset is 24 times smaller. The file is named for what it covers rather
than for the font it came from; the licence declares no Reserved Font Name, so
the subset is free to keep the family name in its own name table, which it
does — name ID 1 still reads "Montserrat".

Regenerate with `fontTools`, which is not needed to build anything here:

```
python3 -m fontTools.subset Montserrat-Regular.ttf \
  --unicodes="U+0020-007E" \
  --layout-features='' --no-layout-closure --notdef-outline --recommended-glyphs \
  --name-IDs='*' --name-legacy --name-languages='*' \
  --drop-tables+=DSIG \
  --output-file=ascii.ttf
```

`--name-IDs='*'` is the part that matters for the licence: the default drops the
copyright and licence records, and the OFL requires the notice to travel with
the font. `--recommended-glyphs` is what keeps the layout tables and the hinting
instructions (`cvt `, `fpgm`, `prep`) — nothing in this crate hints, but a
subset that silently dropped them would be a worse thing to hand anyone.

`subsetter`, the Rust crate of the same job, is not usable here: it strips the
`cmap` table because it exists to embed fonts in PDFs, where the PDF supplies
the character mapping instead. A text rasterizer needs the opposite.

## `snake.png`

Used by `examples/snake.rs`. Its provenance was not recorded when it was added,
so there is nothing to state here.
