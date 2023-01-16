[![build badge](https://github.com/vamolessa/pepper/workflows/rust/badge.svg?branch=master)](https://github.com/vamolessa/pepper)
[![liberapay badge](https://img.shields.io/liberapay/goal/lessa.svg?logo=liberapay)](https://liberapay.com/lessa/donate)

### A simple and opinionated modal code editor for your terminal

![main screenshot](https://vamolessa.github.io/pepper/_content/screenshots/main.png)

[more screenshots](https://vamolessa.github.io/pepper/_content/screenshots/)

Pepper is an experiment of mine to simplify code editing from the terminal.
It's mission is to be a minimal and fast code editor with an orthogonal set of both editing and navigation features.

## [help page](https://vamolessa.github.io/pepper/pepper/rc/help)
## [default keybindings](https://vamolessa.github.io/pepper/pepper/rc/bindings)
## [command reference](https://vamolessa.github.io/pepper/pepper/rc/command_reference)
## [expansion reference](https://vamolessa.github.io/pepper/pepper/rc/expansion_reference)
## [defining language syntaxes](https://vamolessa.github.io/pepper/pepper/rc/language_syntax_definitions)
## [config recipes](https://vamolessa.github.io/pepper/pepper/rc/config_recipes)
## [changelog](https://vamolessa.github.io/pepper/pepper/rc/changelog)

### [try it on your browser!](https://vamolessa.github.io/pepper/web)

# installation

## binaries
Pepper is open-source, which means you're free to build it and access all of its features.
However, to support the development, prebuilt binaries are available for purchase at itch.

[vamolessa.itch.io/pepper](https://vamolessa.itch.io/pepper)

This will not only keep you updated with the latest features/fixes but also support further
pepper development!

## using [`cargo`](https://doc.rust-lang.org/cargo/)
Simply running `cargo install pepper` will get you the vanilla pepper editor experience.

However, if you also want [LSP](https://microsoft.github.io/language-server-protocol/) support,
you can run `cargo install pepper-plugin-lsp` which will install the pepper editor together with its lsp plugin.

## from source
```
cargo install --git https://github.com/vamolessa/pepper pepper
```

**NOTE(1)**: installing from source still requires `cargo` (at least it's easier this way).

**NOTE(2)**: installing from source will actually install the editor with the configurations I use
(you can check [my setup](https://github.com/vamolessa/pepper/blob/master/mine/src/main.rs)).

## if you find a bug or need help
Please [open an issue](https://github.com/vamolessa/pepper/issues)

# goals

- small, however orthogonal, set of editing primitives
- mnemonic and easy to reach default keybindings (assuming a qwerty keyboard)
- cross-platform (Windows, Linux, BSD, Mac and even [Web](https://vamolessa.github.io/pepper/web))
- extensible through external cli tools
- be as fast and reponsive as possible
- zero runtime dependencies (besides platform libs)

# non goals

- support every possible workflow (it will never ever get close to feature parity with vim or emacs)
- complex ui (like breadcumbs, floating windows, extra status bars, etc)
- multiple viewports (leave that to your window manager/terminal multiplexer). Instead clients can connect to each other and act together as if they're a single application)
- undo tree
- support for text encodings other than UTF-8
- fuzzy file picker (you can integrate with fzf, skim, fd, etc)
- workspace wide search (you can integrate with grep, ripgrep, etc)
- having any other feature that could instead be implemented by integrating an external tool

# features

- everything is reachable through the keyboard
- modal editing
- multiple cursors
- caret style cursors (like most text editors,
cursors can move past last line character and text is always inserted to its left)
- text-object selection
- keyboard macros
- client/server architecture
- simple syntax highlighting
- language server protocol

# philosophy

In the spirit of [Handmade](https://handmade.network/),
all features are coded from scratch using simple stable Rust code.
These are the only external crates being used in the project:
- `winapi` (windows-only): needed to implement the windows platform layer
- `libc` (unix-only): needed to implement the unix platform layer
- `wasm-bindgen` (web-only): needed to implement the web platform layer

# modal editing

Pepper is modal which means keypresses do different things depending on which mode you're in.
However, it's also designed to have few modes so the overhead is minimal. Most of the time, users will be in
either `normal` or `insert` mode.

# comparing to vim

Like Vim, you have to actively start text selection.
However, unlike it, you can also manipulate selections in normal mode.
Also, there's no 'action' then 'movement'. There's only selections and actions.
That is, `d` will always only delete selected text. If the selection was empty, it does nothing.

Pepper expands on Vim's editing capabilities by supporting multiple cursors.
This enables you to make several text transformations at once.
Also, cursors behave like carets instead of blocks and can always go one-past-last-character-in-line.

In [config recipes](https://vamolessa.github.io/pepper/pepper/rc/config_recipes#vim-bindings) you'll find some basic "vim-like" keybindigns
for more vim comparisons.

# comparing to kakoune

Like Kakoune, you can manipulate selections while in normal mode and actions always operate on selections.
However, unlike it, normal mode remembers if you're selecting text or nor (think a pseudo-mode).
This way, there's no need for extra `alt-` based keybindings.

Pepper is heavily inspired by Kakoune's selection based workflow and multiple cursors.
However its cursors behave like caret ranges instead of block selections.
That is, the cursor is not a one-char selection but only a visual cue to indicate the caret location.

# keybindings at a glance

![keybindings](https://vamolessa.github.io/pepper/_content/images/keybindings.png)
Also at [keyboard-layout-editor](http://www.keyboard-layout-editor.com/#/gists/175ca15e15b350e1a2a808fe0b5eecb5).

# development thread
It's possible to kinda follow Pepper's development history in this
[twitter thread](https://twitter.com/ahvamolessa/status/1276978064166182913)

# support pepper development
Pepper is open-source, which means you're free to build it and access all of its features.

However, prebuilt binaries are available for purchase at itch.

<iframe src="https://itch.io/embed/810985?linkback=true" width="552" height="167" frameborder="0">
  <a href="https://vamolessa.itch.io/pepper">pepper code editor by Matheus Lessa Rodrigues</a>
</iframe>

You can also directly buy me a coffee :)

<a href="https://liberapay.com/lessa/donate"><img alt="Donate using Liberapay" src="https://liberapay.com/assets/widgets/donate.svg"></a>
[![ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/K3K86X3RD)

Please consider supporting Pepper's development and I'll be forever grateful :)
