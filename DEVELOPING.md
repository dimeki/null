# Working on the interface

Everything is in `ui/`:

- `theme.slint`: colour palettes (`Theme`), the layout choice, icon shapes, and the data types the Rust code fills in
- `state.slint`: `AppState`, all of the interface's state and actions in one global. Screens read and write it, and the Rust side talks only to it
- `components.slint`: reusable pieces (rows, buttons, toggles, scroll list, seek bar, sidebar item, chips)
- `screens/*.slint`: one file per screen (search, library, playlist, artist, lyrics, party, settings, sign-in)
- `chrome-*.slint`: the parts around the screens (tab bar, sidebar, queue, player bar, update notice)
- `screen-host.slint`: picks which screen to show. Every layout puts it where its content goes
- `layout-*.slint`: how it is all arranged: `layout-tabs.slint` is the original, `layout-sidebar.slint` has a sidebar and a centred player bar, `layout-art.slint` puts a big cover and the player in a left column, `layout-compact.slint` has a segmented header and a floating player pill
- `app.slint`: the two windows and the keyboard shortcuts

## Adding a layout

Copy `layout-sidebar.slint`, arrange `SideNav` or `TabBar`, `ScreenHost`, `QueuePanel` and `PlayerBar` as you like,
give it the next number in `Theme.layout-names` (and the `clamp(0, N)` in `state.rs` and `engine.rs`), and add an
`if Theme.layout == N: ...` line in `app.slint`. Nothing about the screens changes.

## Adding a theme

Add a block to `Theme.palettes` in `theme.slint` and a name to `Theme.names`. A test checks that every palette keeps its text readable.

## See every screen at once

```
cargo run --release --features gallery -- --gallery gallery
```

This fills the window with made-up data and writes a PNG of every screen, in every layout and theme, into
`gallery\`, plus `gallery\index.html` to look at them together. It draws into memory: no window opens and nothing
is clicked. The scenes are in `src/gallery.rs`; add a screen or a state there when you add one to the app. The
feature is off in normal builds, so none of this ships in the released program.

A handy check after a refactor: generate the gallery before and after, and compare the PNG files. Identical files mean nothing changed.

## Motion

Animation is deliberately small and quiet. Durations live in `Theme` (`fade-hover` 100 ms, `fade` 120 ms,
`fade-screen` 140 ms, `fade-move` 160 ms) and are all 0 in performance saving. The curve used for movement is
`cubic-bezier(0.23, 1, 0.32, 1)`; colour changes use `ease`. Rules followed:

- Things done constantly (row hover, nav items, tabs) do not animate. Shortcut keys never animate
  (`AppState.skip-anim` switches the screen fade off while one is used).
- A new screen fades in (`FadeIn`), a loading line eases across the top (`LoadingLine`), covers cross-fade and
  the song title fades when the song changes (`CoverImage`, `FadeText`), a switch knob glides, and buttons sink
  slightly while pressed.
- These are driven by small timers rather than `animate`, so they always start from the beginning, and each timer
  runs only while its animation does.

To see an animation part-way through, run the gallery with `GALLERY_FRAME_MS=40` (the picture is taken that many
milliseconds after the screen changes).
