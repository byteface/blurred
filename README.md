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

This rebuild comes from the original tiny prototype by byteface:

https://gist.github.com/byteface/9ca8c3d885d08284bfaebef6256591b2

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

The real app is rebuilt in Rust as a lightweight desktop utility:

```bash
make run
```

You can also use:

```bash
make check
make release
make package
```

## Release process

GitHub Actions is set up to build release artifacts for macOS, Windows, and Linux whenever you push a version tag.

Create and push a tag like this:

```bash
git tag v0.2.0
git push origin v0.2.0
```

That workflow will build the platform artifacts and attach them to the GitHub release page for that tag.

## Positioning

The cleanest way to describe blurred is:

> A privacy-first desktop reference window for sensitive text that auto-conceals itself when you are not using it.

That framing is more honest and more durable than pretending it replaces secure credential storage.

## Next steps

- package it for macOS, Windows, and Linux
- add a global hotkey
- offer optional encrypted file support
