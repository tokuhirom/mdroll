# Inline images

`mdroll` places images with the Kitty graphics protocol, sized to fit the column
budget and capped so there is always text left to orient by.

![A generated gradient, 320×180](example.png)

The alt text becomes a caption. Scrolling crops the image rather than dropping
it, so a picture stays glued to the paragraph that introduced it.

## Where it falls back

| Terminal | Result |
| --- | --- |
| WezTerm, kitty, ghostty | Rendered inline |
| Anything else | Alt text, dimmed |

> [!NOTE]
> Inside `tmux`, or inside herdr without `kitty_graphics = true`, images fall
> back to alt text on purpose.
