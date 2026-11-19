# KimiOS theme packs — sources, licenses & attribution

All schemes below are redistributable open-source color palettes. Each
`.ktheme` in `market/themes/` is a derived work (palette values mapped onto
the 11 KimiOS UI slots); the upstream copyright and permission notices are
reproduced here as required by the MIT license. License texts were fetched
from the upstream repositories on 2026-11-19.

## Slot mapping

`.ktheme` = 11 `RRGGBB` lines, parsed by `theme.rs::load_theme_file`:

| # | Slot | Meaning | Typical source |
|---|------|---------|----------------|
| 0 | `black` | deepest background | scheme's darkest bg |
| 1 | `surface` | main background | editor/background color |
| 2 | `surface_hi` | elevated surface | selection / current-line |
| 3 | `surface_max` | highest surface | indent guide / line numbers |
| 4 | `outline` | borders | border / comment color |
| 5 | `text` | foreground | editor foreground |
| 6 | `muted` | comment / subtext | comment color |
| 7 | `red` | primary accent (`RED()`) | scheme accent (red, or its signature accent) |
| 8 | `red_dim` | dim accent (`RED_DIM()`) | darker red, or documented blend |
| 9 | `green` | green accent | string/success green |
| 10 | `panel` | panel background | sidebar/panel bg, or documented blend |

Slots marked **derived** below are not upstream hex values: they are linear
RGB blends (`mix(a, b, t)` = t·a + (1−t)·b, round half up) computed by
`tools/make_ktheme.py`, needed because the 11-slot UI model requires more
background steps / dim accents than some schemes define. All other slots
are canonical upstream values. Light themes (Catppuccin Latte) invert the
ladder like the built-in Graphite Light: `black` is the lightest page
background and `red_dim` is a light accent tint.

Regenerate any pack: `python3 tools/make_ktheme.py --pack <name>` (or
`--all`). Validate: `python3 tools/make_ktheme.py --check <file>`.

---

## Catppuccin (Mocha & Latte)

- **Author:** Catppuccin organization
- **License:** MIT — `Copyright (c) 2021 Catppuccin`
  (https://github.com/catppuccin/catppuccin/blob/main/LICENSE)
- **Source:** https://github.com/catppuccin/catppuccin — palette tables in
  `README.md` (flavors: Latte, Frappé, Macchiato, Mocha)
- **Packs:** `catppuccin-mocha.ktheme`, `catppuccin-latte.ktheme`

Canonical values used:

| Role | Mocha | Latte |
|------|-------|-------|
| crust | `#11111b` | `#dce0e8` |
| mantle | `#181825` | `#e6e9ef` |
| base | `#1e1e2e` | `#eff1f5` |
| surface0 | `#313244` | `#ccd0da` |
| surface1 | `#45475a` | `#bcc0cc` |
| surface2 | `#585b70` | `#acb0be` |
| overlay1 | `#7f849c` | `#8c8fa1` |
| subtext0 | `#a6adc8` | `#6c6f85` |
| text | `#cdd6f4` | `#4c4f69` |
| red | `#f38ba8` | `#d20f39` |
| green | `#a6e3a1` | `#40a02b` |
| peach | `#fab387` | `#fe640b` |
| yellow | `#f9e2af` | `#df8e1d` |
| blue | `#89b4fa` | `#1e66f5` |
| mauve | `#cba6f7` | `#8839ef` |
| teal | `#94e2d5` | `#179299` |

Mapping — Mocha: black=crust, surface=base, surface_hi=surface0,
surface_max=surface1, outline=surface2, text=text, muted=overlay1, red=red,
green=green, panel=mantle; **red_dim derived** `#5e3f53` = 30% red over base.
Mapping — Latte (light): black=base, surface=mantle, surface_hi=surface0,
surface_max=surface1, outline=surface2, text=text, muted=subtext0, red=red,
green=green, panel=crust; **red_dim derived** `#e9c4cf` = 20% red over base.

## Nord

- **Author:** Sven Greb
- **License:** MIT — `Copyright (c) 2016-present Sven Greb <development@svengreb.de> (https://www.svengreb.de)`
  (https://github.com/nordtheme/nord/blob/develop/license)
- **Source:** https://github.com/nordtheme/nord — `src/nord.scss`
- **Pack:** `nord.ktheme` (canonical reissue of the original `nord.theme`,
  which is kept for compatibility)

Canonical palette (all 16): nord0 `#2e3440`, nord1 `#3b4252`, nord2
`#434c5e`, nord3 `#4c566a`, nord4 `#d8dee9`, nord5 `#e5e9f0`, nord6
`#eceff4`, nord7 `#8fbcbb`, nord8 `#88c0d0`, nord9 `#81a1c1`, nord10
`#5e81ac`, nord11 `#bf616a`, nord12 `#d08770`, nord13 `#ebcb8b`, nord14
`#a3be8c`, nord15 `#b48ead`.

Mapping: black=nord0, surface=nord1, surface_hi=nord2, surface_max=nord3,
text=nord6, red=nord8 (Nord's documented accent color), green=nord14.
**Derived:** outline `#6f788a` = 75% nord3 + 25% nord4; muted `#a0a8b6` =
40% nord3 + 60% nord4; red_dim `#495e6b` = 30% nord8 over nord0; panel
`#272c36` = 85% nord0 over black. (Nord's aurora red is nord11 `#bf616a`;
the accent slot follows Nord's official accent, nord8, as in the original
pack.)

## Gruvbox (dark: medium / hard / soft)

- **Author:** Pavel Pertsev (morhetz) `<morhetz@gmail.com>`
- **License:** MIT/X11 — declared in the upstream README ("License:
  MIT/X11") and `package.json` (`"license": "MIT"`);
  https://github.com/morhetz/gruvbox. No standalone LICENSE file upstream;
  the standard MIT/X11 text below applies with the copyright notice
  `Copyright (c) Pavel Pertsev <morhetz@gmail.com>`.
- **Source:** https://github.com/morhetz/gruvbox — `colors/gruvbox.vim`
  (palette variables + dark-mode assignments)
- **Packs:** `gruvbox-dark.ktheme` (medium), `gruvbox-dark-hard.ktheme`,
  `gruvbox-dark-soft.ktheme`

Canonical values used (dark mode): dark0_hard `#1d2021`, dark0 `#282828`,
dark0_soft `#32302f`, dark1 `#3c3836`, dark2 `#504945`, dark3 `#665c54`,
dark4 `#7c6f64`, gray_245 `#928374`, light0 `#fbf1c7` (fg0), light1
`#ebdbb2` (fg1), bright_red `#fb4934`, bright_green `#b8bb26`, bright_yellow
`#fabd2f`, bright_blue `#83a598`, bright_purple `#d3869b`, bright_aqua
`#8ec07c`, bright_orange `#fe8019`, neutral_red `#cc241d`, neutral_orange
`#d65d0e`, faded_red `#9d0006`.

Mapping (all three): text=light0, muted=gray_245 (gruvbox comment color),
red=bright_red, red_dim=neutral_red (canonical), green=bright_green.
Background ladders:
- medium: black=dark0_hard, surface=dark0, hi=dark1, max=dark2,
  outline=dark3; **panel derived** `#232425` = midpoint dark0_hard/dark0.
- hard: surface=dark0_hard, hi=dark0, max=dark1, outline=dark2;
  **black derived** `#0f1011` = 50% dark0_hard over black;
  **panel derived** `#161819` = 75% dark0_hard over black.
- soft: black=dark0_hard, surface=dark0_soft, hi=dark1, max=dark2,
  outline=dark3; **panel derived** `#282828` = midpoint dark0_hard/dark0_soft
  (equals canonical dark0).

## Tokyo Night (Night & Storm)

- **Author:** Enkia
- **License:** MIT — `Copyright (c) 2018-present Enkia`
  (https://github.com/tokyo-night/tokyo-night-vscode-theme/blob/master/LICENSE.txt;
  repo moved from `enkia/tokyo-night-vscode-theme`)
- **Source:** https://github.com/tokyo-night/tokyo-night-vscode-theme —
  `themes/tokyo-night-color-theme.json` (Night) and
  `themes/tokyo-night-storm-color-theme.json` (Storm)
- **Packs:** `tokyo-night.ktheme`, `tokyo-storm.ktheme`

Canonical values used:

| Role | Night | Storm |
|------|-------|-------|
| editor.background | `#1a1b26` | `#24283b` |
| editor.foreground | `#a9b1d6` | `#a9b1d6` |
| sideBar/panel.background | `#16161e` | `#1f2335` |
| editorWidget.border | `#101014` | `#1b1e2e` |
| list.activeSelectionBackground | `#202330` | `#2c324a` |
| editorIndentGuide.background1 | `#232433` | `#2d324a` |
| editorLineNumber.foreground | `#363b54` | `#3b4261` |
| terminal.ansiBlack | `#363b54` | `#414868` |
| comment | `#51597d` | `#5f6996` |
| red (ansiRed / tags) | `#f7768e` | `#f7768e` |
| markup.deleted (dim red) | `#914c54` | `#914c54` |
| green (strings) | `#9ece6a` | `#9ece6a` |
| yellow | `#e0af68` | `#e0af68` |
| orange | `#ff9e64` | `#ff9e64` |
| blue | `#7aa2f7` | `#7aa2f7` |
| cyan | `#7dcfff` | `#7dcfff` |
| magenta | `#bb9af7` | `#bb9af7` |
| teal-green (ansiGreen) | `#73daca` | `#73daca` |

Mapping — Night: black=widget border, surface=editor bg, hi=selection,
max=indent guide, outline=line numbers, text=foreground, muted=comment,
red=ansiRed, red_dim=markup.deleted (canonical), green=string green,
panel=sideBar bg. Mapping — Storm: same roles; max=line numbers,
outline=ansiBlack (its ladder has no brighter border step). All slots
canonical — no derived values.

## One Dark

- **Authors:** Binaryify (One Dark Pro port); original Atom One Dark by
  GitHub Inc. (atom/one-dark-syntax)
- **License:** MIT — `Copyright (c) 2013-2022 Binaryify`
  (https://github.com/Binaryify/OneDark-Pro/blob/master/LICENSE.txt) and
  `Copyright (c) 2016 GitHub Inc.`
  (https://github.com/atom/one-dark-syntax/blob/master/LICENSE.md)
- **Source:** https://github.com/Binaryify/OneDark-Pro —
  `themes/OneDark-Pro.json` (MIT port of the Atom One Dark scheme; syntax
  colors match `atom/one-dark-syntax/styles/colors.less`)
- **Pack:** `one-dark.ktheme`

Canonical values used: editor.background `#282c34`, editor.foreground
`#abb2bf`, sideBar.background `#21252b`, tab.border/editorGroup.border
`#181a1f`, list.activeSelectionBackground `#2c313a`,
editorIndentGuide.background1 `#3b4048`, focusBorder `#3e4452`, comment
`#5c6370`, red (tags/deleted) `#e06c75`, green (strings) `#98c379`, yellow
`#e5c07b`, orange `#d19a66`, blue `#61afef`, purple `#c678dd`, cyan
`#56b6c2`.

Mapping: black=tab.border (deepest), surface=editor bg, hi=selection,
max=indent guide, outline=focusBorder, text=foreground, muted=comment,
red=`#e06c75`, green=`#98c379`, panel=sideBar bg; **red_dim derived**
`#5f3f48` = 30% red over surface.

## Dracula

- **Author:** Zeno Rocha and the Dracula Theme organization
- **License:** MIT — `Copyright (c) 2023 Dracula Theme`
  (https://github.com/dracula/dracula-theme/blob/master/LICENSE)
- **Source:** https://github.com/dracula/dracula-theme — "Color Palette
  (OSS)" table in `README.md`
- **Pack:** `dracula.ktheme` (canonical reissue of the original
  `dracula.theme`, which is kept for compatibility)

Canonical palette (OSS): Background `#282a36`, Current Line `#44475a`,
Selection `#44475a`, Foreground `#f8f8f2`, Comment `#6272a4`, Cyan
`#8be9fd`, Green `#50fa7b`, Orange `#ffb86c`, Pink `#ff79c6`, Purple
`#bd93f9`, Red `#ff5555`, Yellow `#f1fa8c`.

Mapping: surface=Background, surface_hi=Current Line, outline=Comment,
text=Foreground, red=Purple (Dracula's signature accent, as in the original
pack), green=Green. **Derived:** black `#1e2029` = 75% Background over
black; surface_max `#565969` = 90% Current Line + 10% Foreground; muted
`#97a1bf` = 65% Comment + 35% Foreground; red_dim `#554a71` = 30% Purple
over Background; panel `#21222c` = 82% Background over black.

---

## License texts

### MIT License (applies to all schemes above, with their respective copyright notices)

Copyright notices, per scheme:

- Catppuccin: `Copyright (c) 2021 Catppuccin`
- Nord: `Copyright (c) 2016-present Sven Greb <development@svengreb.de> (https://www.svengreb.de)`
- Gruvbox: `Copyright (c) Pavel Pertsev <morhetz@gmail.com>` (MIT/X11)
- Tokyo Night: `Copyright (c) 2018-present Enkia`
- One Dark Pro: `Copyright (c) 2013-2022 Binaryify`
- Atom One Dark: `Copyright (c) 2016 GitHub Inc.`
- Dracula: `Copyright (c) 2023 Dracula Theme`

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

### Notes

- The `.ktheme` files themselves contain only the 11 mapped hex values (the
  parser accepts nothing else); this document carries the attribution and
  ships with the repo (`market/THEMES.md`).
- The original `nord.theme` / `dracula.theme` remain in `market/themes/`
  for backward compatibility; `repo.json` now serves the canonical
  `.ktheme` reissues under the same package names.
- `cyberpunk.theme` and `sakura.theme` are original KimiOS palettes (not
  derived from the schemes above).
