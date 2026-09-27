# Assets

Images the examples load rather than generate.

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

## `snake.png`

Used by `examples/snake.rs`. Its provenance was not recorded when it was added,
so there is nothing to state here.
