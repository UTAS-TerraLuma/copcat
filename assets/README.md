# Assets

## Icon

`icon.png` (256 px) and `icon.ico` (16, 24, 32, 48, 64, 128 and 256 px, each
stored as a PNG) are Copcat's icon: a cat's head drawn as points coloured by
height, blue at the chin up to amber at the ear tips, on a dark rounded square.
Below 48 px the cat is drawn solid, since the points would blur together.

Both are generated, not drawn by hand. To change the icon, edit
`examples/make_icon.rs` and run:

```sh
cargo run --example make_icon
```

It overwrites both files here. Commit them along with the change.

Where they are used:

- `icon.ico`: `build.rs` embeds it in `copcat-viewer.exe` and `copcat.exe` on
  Windows, so Explorer, the taskbar and the Start Menu shortcut show it.
- `icon.png`: the viewer window's icon (`src/lib.rs`), and the icon `copcat
  integrate` installs on Linux (`src/integrate/`).

To replace the icon with a drawn one, overwrite both files (keep `icon.png` at
256 px, and give `icon.ico` at least 16, 32, 48 and 256 px images), and remove
or update the example.

## Fonts

`fonts/` holds Inter and JetBrains Mono, with their licences; see the main
README.
