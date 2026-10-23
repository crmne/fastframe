# CJK variable font fixture

`CjkVariableTest.otf` is a test-only subset of Noto Sans CJK JP 2.004,
copyright 2014-2021 Adobe, under the accompanying SIL Open Font License 1.1.
It is renamed Fastframe CJK Test and is never bundled with an application.

The subset retains あ, 中 and 한 and the variable `wght` axis. Its default
weight is 100, not 400, reproducing thin fallback text without depending on
installed fonts. Production continues to use installed system fonts.

Made with FontTools 4.67.0 from face 0 of `NotoSansCJK-VF.ttc`:

```sh
fonttools subset NotoSansCJK-VF.ttc --font-number=0 \
  --unicodes=U+3042,U+4E2D,U+D55C --name-IDs='*' --name-languages='*' \
  --output-file=CjkVariableTest.otf
```

Family, style, unique, full, PostScript and variable PostScript prefix name
records (IDs 1, 2, 3, 4, 6, 16, 17, 25) were renamed with FontTools.
Copyright and license records are preserved.

Upstream: https://github.com/notofonts/noto-cjk

Original collection SHA256:
`d3d8256cdec8dbcb3552284bc6b20c734dd60c2ee9df83b5758e34807c4bac32`.

`CjkMediumToBlackTest.otf` restricts the same subset's weight axis to
500–900 with `fontTools.varLib.instancer.instantiateVariableFont(font,
{"wght": (500, 900)}, inplace=True)`. This checks that regular is clamped
to 500 when the face cannot draw 400.

`CjkCollectionTest.ttc` combines the existing `../yi/YiTest.ttf` as face 0
with `CjkVariableTest.otf` as face 1, using `fontTools.ttLib.TTCollection`.
The first face is static and the second variable, so reading the wrong face
cannot accidentally pass the weight regression. The Yi subset's attribution
and source are recorded in `../yi/README.md`; it is copyright 2022 The Noto
Project Authors, under the same SIL Open Font License 1.1. All names and
license records of both subsets are retained in the collection.
