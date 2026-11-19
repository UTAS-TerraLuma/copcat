# Commands

Every command in copcat, as of 2026-11-19. The source of truth is
`src/app/commands.rs`; if this list and the code disagree, the code is right.

Shortcuts are the defaults. Ctrl is Cmd on a Mac. Any command can be given a
different shortcut under `keybindings` in `settings.json`, by its id:

```jsonc
{ "keybindings": { "view.reset": "F5" } }
```

## The palette

One palette, two modes. A leading `>` in the text field means command mode,
so typing or deleting `>` switches between them.

| Shortcut | Mode | What it lists |
|---|---|---|
| Ctrl+P | Quick open | Things to open |
| Ctrl+Shift+P | All commands | Things to do |

Up and Down move through the list, Page Up and Page Down move by a page,
Enter chooses, and Esc goes back a level or closes. Backspace in an empty
field also goes back a level.

## Quick open (Ctrl+P)

Most of this list depends on your setup.

| Item | What it does |
|---|---|
| Open `<what you typed>` | Shown first when the text is a URL or an existing file path |
| Your data sources | Each STAC or folder source you've added; choosing one browses into it |
| Load more… | Inside a data source, fetches the next page when there is one |
| Refresh | Inside a data source, fetches its list again |
| Recent sources | Clouds opened this session |
| Open file… | Ctrl+O |
| Open URL… | Ctrl+L |

## All commands (Ctrl+Shift+P)

Commands that can't run right now are greyed out, such as "Return to the
subcloud" before one has been drawn.

### File

| Command | Shortcut | Id |
|---|---|---|
| Open file… | Ctrl+O | `file.open` |
| Open URL… | Ctrl+L | `file.open_url` |

### Mode

| Command | Shortcut | Id |
|---|---|---|
| Explore mode | | `mode.explore` |
| Return to the subcloud | | `mode.subcloud` |

### Subcloud

| Command | Shortcut | Id |
|---|---|---|
| Draw a line subcloud | | `subcloud.tool_line` |
| Draw a circle subcloud | | `subcloud.tool_circle` |
| Draw a square subcloud | | `subcloud.tool_square` |
| Cancel the subcloud tool | Esc, while a tool is picked | `subcloud.cancel_tool` |

### View

| Command | Shortcut | Id |
|---|---|---|
| View from top | 7 | `view.top` |
| View from bottom | Ctrl+7 | `view.bottom` |
| View from front | 1 | `view.front` |
| View from back | Ctrl+1 | `view.back` |
| View from right | 3 | `view.right` |
| View from left | Ctrl+3 | `view.left` |
| Perspective projection | | `view.perspective` |
| Orthographic projection | | `view.orthographic` |
| Toggle perspective / orthographic | 5 | `view.toggle_projection` |
| Reset view | | `view.reset` |
| Toggle diagnostics | Ctrl+Shift+D | `view.toggle_diagnostics` |
| Retry failed nodes | | `view.retry_failed_nodes` |

### Display

| Command | Shortcut | Id |
|---|---|---|
| Toggle eye-dome lighting | | `display.toggle_edl` |
| Colour by height | | `display.color_height` |
| Colour by RGB | | `display.color_rgb` |
| Colour by classification | | `display.color_class` |

### Export

| Command | Shortcut | Id |
|---|---|---|
| Save image… | Ctrl+S | `export.save_image` |
| Copy render command | | `export.copy_command` |

### Palette

| Command | Shortcut | Id |
|---|---|---|
| Show all commands | Ctrl+Shift+P | `palette.commands` |
| Quick open… | Ctrl+P | `palette.open` |

### Settings

| Command | Shortcut | Id |
|---|---|---|
| Open user settings | | `settings.open_user` |
| Open workspace settings | | `settings.open_workspace` |
| Reload settings | | `settings.reload` |
| Save current display settings as defaults | | `settings.save_display_defaults` |
| Select theme… | | `settings.select_theme` |

### Data sources

| Command | Shortcut | Id |
|---|---|---|
| Add STAC source… | | `sources.add_stac` |
| Add folder source… | | `sources.add_folder` |
| Remove data source… | | `sources.remove` |
| Clear data source cache | | `sources.clear_cache` |
