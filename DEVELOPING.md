# Working on the interface

The interface is in `ui/`:

- `theme.slint`: colours (`Theme`), the icon shapes, and the data types the Rust code fills in
- `components.slint`: the reusable pieces (rows, buttons, toggles, scroll list, seek bar)
- `app.slint`: the sign-in window and the main window with all its screens

## See every screen at once

```
cargo run --release --features gallery -- --gallery gallery
```

This fills the window with made-up data and writes a PNG of every screen, in every theme,
into `gallery\`, plus `gallery\index.html` to look at them together. It draws into memory:
no window opens and nothing is clicked. The scenes are in `src/gallery.rs`; add a screen or
a state there when you add one to the app. The feature is off in normal builds, so none of
this ships in the released program.

## Themes

The palettes are in `Theme.palettes` in `theme.slint`: add a theme by adding a block there and a name in `Theme.names`. `Theme.palette` is 0 to follow Windows, or the 1-based number of a palette. A test checks every palette keeps its text readable.
