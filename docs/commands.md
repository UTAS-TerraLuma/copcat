# Commands

Every command in copcat, as of 2026-11-27. The source of truth is
`src/app/commands.rs`; if this list and the code disagree, the code is right.

Shortcuts are the defaults. Ctrl is Cmd on a Mac. Any command, listed in the
palette or not, can be given a different shortcut under `keybindings` in
`settings.json`, by its id:

```jsonc
{ "keybindings": { "view.reset": "F5" } }
```

## Two lists, two shortcuts

| Shortcut | Opens | What it lists |
|---|---|---|
| Ctrl+P | The command palette (also **View > Command palette**) | Things to do: a curated set of commands |
| Ctrl+O | Quick open | Things to open |

They're separate lists: each shortcut opens its own, in place of the other if
that's showing. Up and Down move through the list, Page Up and Page Down move
by a page, Enter chooses, and Esc goes back a level or closes. Backspace in an
empty field also goes back a level. Some items open a sub-list or ask for text
(Select theme…, Add STAC source…, From URL…) above the list they're in; Esc
goes back to it.

## Quick open (Ctrl+O)

Most of this list depends on your setup.

| Item | What it does |
|---|---|
| Open `<what you typed>` | Shown first when the text is a URL or an existing file path |
| Recent sources | Clouds opened this session |
| Your data sources | Each STAC source you've added; choosing one browses into it |
| Load more… | Inside a data source, fetches the next page when there is one |
| Refresh | Inside a data source, fetches its list again |
| From file… | Choose a COPC file on disk (`file.open`) |
| From URL… | Type or paste a COPC URL or path, in the palette (`file.open_url`) |
| From STAC… | List the data sources to browse, or add one (`sources.list`) |

## The command palette (Ctrl+P)

The palette lists the commands marked "yes" below, in the order shown.
Commands that can't run right now are greyed out, such as "Retry failed
nodes" when nothing has failed. The rest have buttons of their own (the
subcloud tools, the gizmo's views from each side) or shortcuts, and can all be
bound in `keybindings`.

### Opening

| Command | Shortcut | Id | In the palette |
|---|---|---|---|
| Quick open… | Ctrl+O | `palette.open` | yes |
| Open file… | | `file.open` | yes |
| Open URL… | | `file.open_url` | yes |
| Browse STAC sources… | | `sources.list` | yes |

### View

| Command | Shortcut | Id | In the palette |
|---|---|---|---|
| Perspective projection | | `view.perspective` | yes |
| Orthographic projection | | `view.orthographic` | yes |
| Toggle perspective / orthographic | 5 | `view.toggle_projection` | yes |
| Reset view | | `view.reset` | yes |
| View from top | 7 | `view.top` | no |
| View from bottom | Ctrl+7 | `view.bottom` | no |
| View from front | 1 | `view.front` | no |
| View from back | Ctrl+1 | `view.back` | no |
| View from right | 3 | `view.right` | no |
| View from left | Ctrl+3 | `view.left` | no |

### Rendering

| Command | Shortcut | Id | In the palette |
|---|---|---|---|
| Toggle eye-dome lighting | | `display.toggle_edl` | yes |
| Colour by height | | `display.color_height` | yes |
| Colour by RGB | | `display.color_rgb` | yes |
| Colour by classification | | `display.color_class` | yes |

### Export

| Command | Shortcut | Id | In the palette |
|---|---|---|---|
| Save image… | Ctrl+S | `export.save_image` | yes |
| Copy render command | | `export.copy_command` | yes |

### Loading

| Command | Shortcut | Id | In the palette |
|---|---|---|---|
| Retry failed nodes | | `view.retry_failed_nodes` | yes |
| Toggle diagnostics | Ctrl+Shift+D | `view.toggle_diagnostics` | yes |

### Settings

| Command | Shortcut | Id | In the palette |
|---|---|---|---|
| Settings… (the Settings window) | | `settings.open` | yes |
| Open user settings | | `settings.open_user` | yes |
| Open workspace settings | | `settings.open_workspace` | yes |
| Reload settings | | `settings.reload` | yes |
| Save current rendering settings as defaults | | `settings.save_display_defaults` | yes |
| Select theme… | | `settings.select_theme` | yes |

### Data sources

| Command | Shortcut | Id | In the palette |
|---|---|---|---|
| Add STAC source… | | `sources.add_stac` | yes |
| Remove STAC source… | | `sources.remove` | yes |
| Clear data source cache | | `sources.clear_cache` | yes |

### Modes and subclouds

| Command | Shortcut | Id | In the palette |
|---|---|---|---|
| Explore mode | | `mode.explore` | no |
| Return to the subcloud | | `mode.subcloud` | no |
| Draw a line subcloud | | `subcloud.tool_line` | no |
| Draw a circle subcloud | | `subcloud.tool_circle` | no |
| Draw a square subcloud | | `subcloud.tool_square` | no |
| Cancel the subcloud tool | Esc, while a tool is picked | `subcloud.cancel_tool` | no |
| Close the subcloud | | `subcloud.close` | no |

### Palette

| Command | Shortcut | Id | In the palette |
|---|---|---|---|
| Command palette | Ctrl+P | `palette.commands` | no |
