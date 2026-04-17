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

At the time I'd it was cobbled out of necessity but I ended up using it for many years. Recently I needed the same kind of thing and decided to rebuild it in Rust so it's simpler to bundle.


## Features

- open a local text file
- read-only viewer
- hide on focus loss
- reveal on focus
- `Always Visible` toggle
- `Auto Show On Focus` toggle
- dark mode
- window opacity control
- remembers last file and window position
- manual `Show` and `Hide` controls

## Rust Build

Check the makefile to build for your platform

```bash
make run
```

You can also use:

```bash
make check
make release
make package
```
