# kloggr user guide

kloggr is a fast log viewer for very large files. It is a Rust rewrite of
[klogg](https://github.com/variar/klogg), and this guide is adapted from
klogg's documentation.

## Getting started

Start kloggr from the desktop menu, or from a terminal with the file to open:

```
kloggr /var/log/syslog
```

On macOS, files can also be opened from Finder with **Open With > kloggr**.

Several files can be given. If kloggr is already running, the files open in
its last active window instead, and the new launch exits; `kloggr -m`
(`--multi`) starts a separate kloggr. `kloggr --help` lists the options.

By default kloggr reopens the files that were open when it was last closed,
with their marks, search and view settings.

The window has two views. The top one, the *main view*, shows the file. The
bottom one, the *filtered view*, shows the result of the search typed in the
bar between them. Lines that match are listed in file order and get a bullet
in the margin of both views.

The status line at the bottom shows the file's path, its number of lines and
its encoding, or the progress of indexing, searching and saving.

## Opening files

- **File > Open…** (Ctrl+O), or **File > Open Recent**.
- Drop files from the file manager on the window.
- Give the file on the command line.
- **File > Open from Clipboard** (Ctrl+V in a log view) opens the text on the
  clipboard as a temporary file.
- **File > Open from URL…** downloads a file over http or https (the URL on
  the clipboard is offered) and opens it. The status line shows the
  progress; Esc cancels. Compressed downloads are extracted like local ones.

Temporary files (from the clipboard, downloaded, or extracted from archives)
are removed when kloggr closes, and are not remembered in the session or the
recent files.

Each file opens in its own tab. Ctrl+1 to Ctrl+8 show the first eight tabs,
Ctrl+9 the last one, and Ctrl+Tab / Ctrl+Shift+Tab cycle through them.
**View > Opened Files** lists the open files; its **Switch to File…**
(Ctrl+Shift+O) filters them as you type (`*` and `?` are wildcards). The
tab's context menu renames the tab, copies the file's path, opens its folder
and closes tabs.

**File > New Window** opens another window, with its own tabs. kloggr
reopens every window that was open when it closed, at the same size and
place; closing one window while others stay open forgets its tabs.
**View > Full Screen** (F11) toggles full screen; maximizing and minimizing
can be given keys in **Tools > Options… > Shortcuts**.

### Favorites

**Favorites > Add to Favorites** keeps the current file in the Favorites
menu, for files you open often that drop out of the recent files.
**Remove from Favorites…** asks which one to remove.

### Archives and compressed files

kloggr opens compressed files (gzip, bzip2, xz, lzma) and archives (zip, 7z,
tar, also compressed) by extracting them to a temporary folder. When an
archive holds several files, a list lets you choose which ones to open. The
extracted files are removed when kloggr closes. **Options > File** sets
whether kloggr extracts them, and whether it asks first.

### Encodings

kloggr detects the encoding of a file (UTF-8, UTF-16, and the common legacy
encodings). If the guess is wrong, pick the encoding from the **Encoding**
menu; **Auto** goes back to the detected one. The choice is remembered for
the file.

## Searching

Type a pattern in the search bar and press Enter. The buttons next to it
choose how the pattern is read:

| Button | Meaning | Key |
|---|---|---|
| Aa | Case sensitive | Alt+4 |
| .\* | Regular expression (otherwise plain text) | Alt+5 |
| ! | Inverse: show the lines that do *not* match | Alt+6 |
| &\| | Boolean combinations (see below) | Alt+7 |
| ⟳ | Auto-refresh: search new lines as the file grows | Alt+8 |
| Keep | Keep results: the next search opens a new results tab | Alt+9 |

Regular expressions use the syntax of Rust's
[regex](https://docs.rs/regex/latest/regex/#syntax) crate, which is close to
Perl's but has no look-around and no back-references. Alternation is the
most useful feature for logs:

```
Entering (Open|Close)Connection
```

lists the opening and closing calls together, so an open without a close
stands out.

The arrow at the end of the search box lists earlier searches. **Filters**
lists the predefined filters, and **Save search as filter…** in it saves the
current search as one.

### Boolean combinations

With &| on, the pattern is a combination of quoted sub-patterns:

| Operator | Meaning |
|---|---|
| `"x" and "y"`, `"x" & "y"` | both match |
| `"x" or "y"`, `"x" \| "y"` | either matches |
| `not("x")`, `!"x"` | does not match |
| `xor`, `nand`, `nor`, `xnor` | as in logic |

Each sub-pattern follows the other buttons (regular expression or plain
text, case). Inside a sub-pattern, `\"` stands for a quote.

### Marks

Click the margin in front of a line, or select lines and press `m`, to mark
them. Marks show in the filtered view with the matches. `[` and `]` jump to
the previous and next mark.

The menu at the left of the search bar (or `v`, Alt+1, Alt+2, Alt+3) chooses
what the filtered view shows: marks and matches, only marks, or only
matches.

### Search limits

Right-click a line and choose **Set search start** or **Set search end** to
search only part of the file. Lines outside the limits are shown dimmed.
**Clear search limits** searches the whole file again.

### Quick find

Ctrl+F opens a find bar that looks for the next occurrence in the current
view without changing the search. In the log views, `'` and `"` start a quick
find forward or backward, `n` and `N` (or F3 and Shift+F3) repeat it, and
`*` / `.` and `/` / `,` look for the next or previous occurrence of the
selected text.

### The overview bar

The bar at the right of the main view shows where the matches (red) and marks
(blue) are in the whole file. Click or drag in it to scroll there. Hovering
over a line's margin in the filtered view flashes its place in the overview.

### Predefined filters

**Tools > Predefined Filters…** edits filters you use often. Each has a name,
a pattern, and whether the pattern is a regular expression. Checking filters
in the **Filters** drop-down builds the search from them; several checked
filters are combined with "or".

## Moving around

The keys follow *vi* and *less*. All shortcuts can be changed in
**Tools > Options… > Shortcuts**.

| Keys | Action |
|---|---|
| arrows, `j` `k` `h` `l` | move the selection, scroll left or right |
| \[number\] `j` / `k` | move the selection that many lines |
| PageUp / PageDown | scroll a page |
| `^` / `$`, Home | start or end of the selected line |
| \[number\] `g` | go to that line, or the first one |
| Ctrl+Home, Ctrl+End, Shift+G | first or last line |
| Ctrl+L | go to line… |
| `f`, F10 | follow the end of the file (like `tail -f`) |
| `m`, `[`, `]` | mark, previous mark, next mark |
| `+` / `-` | make the main view bigger or smaller |
| `w` | text wrap |
| Space | switch between the main and filtered views |
| Ctrl+S | focus the search box |
| F5 | reload the file |
| Esc | stop the search, or the loading |

Shift + mouse wheel scrolls horizontally.

## Live files

kloggr watches the open files with the system's change notifications. On
network shares and some mounts these don't arrive: turn on **Check files at
an interval** in **Options > File** (on by default on Windows). When a file
grows, the new lines appear;
with auto-refresh on, they are searched too. When a file is truncated,
rewritten or rotated, kloggr indexes it again and drops the marks and
search results.

`f` follows the end of the file as it grows. Scrolling up stops following;
scrolling down at the end, or Shift+G, starts again (**Options > File** can
turn off the scrolling part).

**View > Reload** (F5) indexes the file again. **View > Stop** (Esc) stops a
search; while a file is loading, it closes the tab.

## Selecting and copying

Click and drag to select text, Shift+click to extend, double-click to select
a word. Ctrl+C copies, Ctrl+Shift+C copies with line numbers. Copying more
than 64 MiB asks first, and more than 1 GiB is refused.

Right-click a line for more:

- **Save to file** writes the visible lines of that view to a new file;
  **Save selected to file** only the selected ones.
- **Add to search**, **Exclude from search** and **Replace search** change
  the search pattern with the selected text.
- **Send to scratchpad** and **Replace scratchpad** copy the selection to the
  scratchpad.
- **Set selection start** and **Set selection end** select a range of lines
  that is too long to drag.

## Edit menu

- **Copy**, **Select All**, **Find…** and **Go to Line…**, as above.
- **Copy Path** and **Open Containing Folder** for the current file.
- **Open in Editor** opens the file in the system's default application.
- **Clear File…** (Ctrl+X) empties the file on disk after asking. This
  can't be undone.

## View menu

Turns the overview, line numbers in each view, and text wrap on or off.
**Freeze Columns** keeps the start of each line in place while scrolling
horizontally; drag the separator to choose how many columns. Text wrap
shows long lines over several rows (at most one screen per line).

## Highlighters and color labels

*Highlighters* colour lines that match a pattern, to draw attention to errors
or to tell kinds of events apart. They are grouped in *sets*; activate sets
from **Tools > Highlighters** or the log view's context menu. Several sets
can be active.

**Tools > Highlighters…** edits the sets. Each highlighter has a pattern
(regular expression or plain text, case sensitive or not), foreground and
background colours, and colours either the whole line or only the matching
text. When only the matching text is coloured and the pattern has capture
groups, only the groups are coloured. *Colour variance* gives different
matching strings slightly different colours. Highlighters lower in a set win
over the ones above. Sets can be exported to and imported from klogg's
`.conf` files.

*Color labels* are quick highlights of selected text, for this tab only:
Ctrl+Shift+1 to Ctrl+Shift+9 apply a label, Ctrl+D the next one, and
Ctrl+Shift+0 removes them all. Their colours are set in the highlighters
dialog.

## Scratchpad

**Tools > Scratchpad** opens a window for pieces of text: paste or send text
there to convert Base64 or hex, decode URLs, or format JSON or XML. Ctrl+Z sends the
selection from a log view, Ctrl+Shift+Z replaces the scratchpad's text with
it.

## Options

**Tools > Options…** has four tabs:

- **General**: reopen the last session, follow new files, check for new
  versions once a week, the default search options, match highlighting and
  its colour, search history size, and quick find options.
- **View**: font, overview bar, line numbers, and hiding ANSI colour
  sequences (`ESC[…m`) that some programs write for terminals.
- **File**: follow on scrolling past the end, archive extraction, the
  number of recent files, and how files are watched for changes
  (notifications, checking at an interval, or both).
- **Shortcuts**: a primary and a secondary key for every action.

Settings are kept in `settings.toml` in kloggr's configuration folder
(`~/.config/kloggr` on Linux), next to the session, highlighters and
searches.

## When something goes wrong

If kloggr crashes, it saves a report and shows it at the next start, with a
button that opens a GitHub issue with the report. **Help > Report Issue**
opens a new issue with the version and system already filled in.
