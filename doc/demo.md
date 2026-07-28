# mdroll

A terminal Markdown viewer with **big headings** and real *GitHub Flavored
Markdown* — tables, task lists, footnotes, `> [!NOTE]` alerts, mermaid
diagrams, and inline images, all in the terminal you already have.

![A generated gradient, 320×180](example.png)

## パイプライン

見出しはビットマップとして描画されるので、日本語でも同じように大きくなります。
本文の折り返しは UAX #14 に従い、禁則処理によって句読点が行頭に来ません。

```mermaid
flowchart LR
    A[source] --> B[comrak] --> C[layout] --> D[screen]
```

## What it renders

| Construct | Key | Notes |
| --- | :---: | --- |
| Tables | | Column widths in display columns, so 日本語 lines up |
| Images | `i` | Kitty graphics, cropped as they scroll |
| Contents | `T` | Every heading, as links |

- [x] UAX #14 line breaking with kinsoku
- [x] OSC 8 hyperlinks and [clickable links](https://example.com)
- [x] Rasterized headings where DECDHL is unavailable

> [!NOTE]
> The headings above are bitmaps: kitty has graphics but no DECDHL, so they
> are rasterized. On WezTerm the same headings use DECDHL instead.
