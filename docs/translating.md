# Translating kloggr

kloggr's interface is in English. A translation is a TOML file, a *catalog*,
that maps each English message to its translation. No tools are needed besides
a text editor.

## Make a catalog

1. Copy `crates/kloggr-app/locales/template.toml` to a file named after your
   language, such as `de.toml` or `zh-CN.toml`.
2. Fill in `language` (a [BCP 47](https://www.rfc-editor.org/info/bcp47) tag:
   `de`, `fr`, `zh-CN`, `zh-TW`, `pt-BR`) and `name` (the language's name in
   itself: `Deutsch`, `简体中文`).
3. Translate the messages: the text right of each `=`. An empty translation
   keeps the English text, so a catalog can be finished bit by bit.

```toml
language = "de"
name = "Deutsch"

[messages]
"Open…" = "Öffnen…"
"{count} lines" = "{count} Zeilen"
"Search in progress ({percent} %)… {matches} found so far." = "Suche läuft ({percent} %)… bisher {matches} gefunden."
```

Words in braces, like `{count}` or `{file}`, are filled in by kloggr: keep them
as they are (same names), but move them wherever your language needs them.
`\n` is a line break. Messages that start with a space or ` — ` are joined to
other text: keep that space or dash.

Some messages are Markdown (the About text): keep the links and `**bold**`
marks.

## Try it

Put the catalog in the `translations` folder of kloggr's configuration folder:

| system | folder |
|---|---|
| Linux | `~/.config/kloggr/translations/` |
| macOS | `~/Library/Application Support/kloggr/translations/` |
| Windows | `%APPDATA%\kloggr\translations\` |

Then choose it in **Tools > Options… > General > Language** and restart
kloggr. With **System** chosen, kloggr uses the catalog that matches the
system's language (`zh-TW` finds a `zh-TW` catalog, else any `zh` one).

## Ship it with kloggr

Send the catalog as a pull request:

1. Add it to `crates/kloggr-app/locales/`.
2. Add `include_str!("../locales/<tag>.toml")` to `BUILT_IN` in
   `crates/kloggr-app/src/i18n.rs`.
3. `cargo test -p kloggr-app` checks that it parses.

## For developers

Every message shown to the user goes through `tr!`:

```rust
.label(tr!("Search"))
tr!("{count} matches", count = n)
```

Messages kept in constants are marked with `msg!` and translated where they are
shown with `crate::i18n::tr(label)`. After adding or changing a message,
regenerate the template:

```bash
KLOGGR_UPDATE_TEMPLATE=1 cargo test -p kloggr-app template
```

The `template_lists_every_message` test fails while the template is out of
date. When an English message changes, its old translations no longer match
and fall back to English until they are updated.

Not translated: key names (Ctrl, Shift, Esc), encoding names, error details
from the system, log file messages and the command line's `--help`.
