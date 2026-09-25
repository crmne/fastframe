# Yi font fixture

`YiTest.ttf` is a test-only subset of Noto Sans Yi 2.002, copyright 2022
The Noto Project Authors, under the accompanying SIL Open Font License 1.1.
It is not embedded in the application. Production uses installed system fonts.

The subset contains the characters in the artist name reported in Spotifast #439:
`ꉈꀧ꒒꒒ꁄꍈꍈꀧ꒦ꉈ ꉣꅔꎡꅔꁕꁄ`. It was made with FontTools 4.64.0,
retaining name/license records and glyph outlines, and renamed Fastpotify Yi Test (Spotifast's former name).
This lets the font scanner regression run without installing a system font.

Upstream: https://github.com/notofonts/yi

Original font SHA256: `9d5d3f9912f14bee3f32c9d8add8a5dd02910dcabc00e3702a2b9883556852c9`.

`YiTallLineGap.ttf` is the same subset with a 500-unit (0.5 em) line gap in
`hhea` and `OS/2`, as Hiragino Sans declares, made with FontTools 4.64.0. It
checks that a fallback with a taller line box is shifted onto Inter's
baseline.
