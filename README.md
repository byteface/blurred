# blurred

blurred is a tiny desktop utility for reading sensitive text without leaving it exposed on screen.

When the window loses focus, the content hides itself. When you focus the window again, the content can reveal immediately. It is intentionally read-only, so you can keep a reference file open without accidentally editing it.

## Why it exists

Sometimes you need a local reference pane for something private or awkward:

- passwords copied from an approved system
- recovery codes
- support snippets
- deployment notes
- one-off credentials
- private prompts or personal notes

blurred is not a password manager. It is a quick-reference window for text you need to glance at, then conceal.

## Origin

This Rust app is inspired by this tiny python prototype:

https://gist.github.com/byteface/9ca8c3d885d08284bfaebef6256591b2

At the time it was cobbled together out of necessity, but I ended up using it for years. Recently I needed the same kind of thing again and decided to rebuild it in Rust so it is easier to bundle and ship.

## Features

- open a local text file
- open recent files
- read-only viewer
- hide on focus loss
- reveal on focus
- `Always Visible` toggle
- `Auto Show On Focus` toggle
- dark mode
- window opacity control
- spacebar show/hide toggle
- remembers last file and window position
- manual `Show` and `Hide` controls

## Rust Build

Visit the releases page to download a ready made app.

Check the `Makefile` to build for your platform.

```bash
make run
```

You can also use:

```bash
make check
make release
make package
```
