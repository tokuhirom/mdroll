# mdroll — Key bindings

Press `H` or `Esc` to close this help.

## Navigation

| Key | Action |
| --- | --- |
| `j` `↓` | Scroll down one line |
| `k` `↑` | Scroll up one line |
| `d` | Half page down |
| `u` | Half page up |
| `f` `Space` `PgDn` | Page down |
| `b` `PgUp` | Page up |
| `g` | Jump to top |
| `G` | Jump to bottom |
| `h` `←` | Scroll left, in no-wrap mode |
| `l` `→` | Scroll right, in no-wrap mode |
| `0` | Reset horizontal scroll |

## Modes

| Key | Action |
| --- | --- |
| `w` | Toggle wrap / no-wrap |
| `s` | Toggle rendered / source view |
| `t` | Cycle theme |
| `z` | Toggle double-height headings |
| `i` | Toggle inline images |

## Copying

| Key | Action |
| --- | --- |
| `Tab` | Move the block cursor forward |
| `Shift-Tab` | Move the block cursor backward |
| `y` | Yank the block as Markdown source |
| `Y` | Yank the block as rendered plain text |
| `yc` | Yank code block contents, without the fences |
| `yp` | Yank the file path |
| `V` | Line selection — extend with `j` and `k`, confirm with `y` |

## Links and search

| Key | Action |
| --- | --- |
| `F` | Link picker — label every link, jump by keystroke |
| `o` `Enter` | Open the link under the block cursor |
| `/` | Search forward |
| `?` | Search backward |
| `n` `N` | Next / previous match |

## Other

| Key | Action |
| --- | --- |
| `r` | Reload the file |
| `H` | Show this help |
| `q` `Esc` | Quit |

Links are also emitted as OSC 8 hyperlinks, so `Cmd`-click or `Ctrl`-click opens
them through the terminal without mdroll capturing the mouse.
