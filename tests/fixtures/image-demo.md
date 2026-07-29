# Inline images

`mdroll` places images with the Kitty graphics protocol, sized to fit the column
budget and capped so there is always text left to orient by.

![A generated gradient, 320×180](example.png)

The alt text becomes a caption. Scrolling crops the image rather than dropping
it, so a picture stays glued to the paragraph that introduced it.

## A row of badges

A paragraph holding nothing but pictures is a figure, and several of them share
a row until it fills up. No caption: the badges say it themselves.

[![build](badge.svg)](https://example.com/build)
[![docs](badge.svg)](https://example.com/docs)
[![release](badge.svg)](https://example.com/release)
[![coverage](badge.svg)](https://example.com/coverage)
[![downloads](badge.svg)](https://example.com/downloads)

Opening one goes where it links, not to the picture of it.

## SVG, at the size the document asked for

<p align="center">
  <a href="https://example.com">
    <img src="logo.svg" alt="A vector logo" width="240">
  </a>
</p>

Rasterized at display size, so it is as sharp as the cells allow. Without the
`width`, this logo would arrive 1200 pixels wide.

## Where it falls back

| Terminal | Result |
| --- | --- |
| WezTerm, kitty, ghostty | Rendered inline |
| Anything else | Alt text, dimmed |

> [!NOTE]
> Inside `tmux`, or inside herdr without `kitty_graphics = true`, images fall
> back to alt text on purpose.

Images behind an `http(s)` URL are fetched in the background and cached under
`~/.cache/mdroll/images`, owner-only. `--no-remote-images` turns that off, and
nothing is fetched at all when stdout is not a terminal.
