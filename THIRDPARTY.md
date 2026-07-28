# Third-party attribution

## Bundled color themes

The UI palettes in `themes/*.toml` are derived from the following projects,
each MIT licensed. Only their color values are reused; the file format and the
role assignments are `mdroll`'s own.

| Theme | Project | License |
| --- | --- | --- |
| Dracula | [dracula/dracula-theme](https://github.com/dracula/dracula-theme) | MIT |
| Nord | [nordtheme/nord](https://github.com/nordtheme/nord) | MIT |
| Gruvbox | [morhetz/gruvbox](https://github.com/morhetz/gruvbox) | MIT |
| Solarized Dark, Solarized Light | [altercation/solarized](https://github.com/altercation/solarized) | MIT |

`themes/Dracula.tmTheme` is a syntect syntax theme written for this project
using the Dracula palette. syntect ships Solarized and the base16 family but
not Dracula, so it is vendored here rather than downloaded at runtime.

## Rust dependencies

Licenses for every crate `mdroll` links against can be listed with:

```console
$ cargo install cargo-license
$ cargo license
```

The notable ones are [comrak](https://github.com/kivikakk/comrak) (BSD-2-Clause)
for Markdown parsing, [syntect](https://github.com/trishume/syntect) (MIT) for
code highlighting, and [crossterm](https://github.com/crossterm-rs/crossterm)
(MIT) for terminal control.
