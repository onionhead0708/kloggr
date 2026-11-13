//! `kloggr-grep`: searches log files with kloggr's engine and prints the
//! matching lines (klogg: `klogg_grep`).
//!
//! It indexes each file and searches it exactly as the app does: the same
//! encoding detection, line splitting and patterns (regular expressions,
//! plain text, klogg's boolean combinations). It is also how the benchmarks
//! time a search against `klogg_grep`.

use std::ffi::OsString;
use std::io::{self, BufWriter, ErrorKind, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use kloggr_core::encoding::Encoding;
use kloggr_core::file::{LogFile, OpenOptions};
use kloggr_core::search::{self, Matcher, Pattern};

const USAGE: &str = "\
Usage: kloggr-grep [OPTIONS] PATTERN FILE...
       kloggr-grep [OPTIONS] -e PATTERN FILE...

Prints the lines of FILE that match PATTERN, a regular expression unless
-F is given. Files are indexed and searched as kloggr does: the encoding is
detected (UTF-8, UTF-16, legacy encodings) and lines are printed in UTF-8.

Options:
  -e, --pattern PATTERN  the pattern (needed when it starts with '-')
  -F, --fixed-strings    PATTERN is plain text, not a regular expression
  -i, --ignore-case      ignore case
  -v, --invert-match     print the lines that don't match
  -b, --boolean          klogg's logical combining: \"a\" and not \"b\"
  -n, --line-number      print each line's number (from 1) before it
  -c, --count            print only the number of matching lines
  -H, --with-filename    print the file name before each line
  -h, --no-filename      never print file names (the default for one file)
      --encoding NAME    read the files as NAME (e.g. utf-16le, latin1)
                         instead of detecting the encoding
      --hide-ansi        remove ANSI colour sequences before matching
      --time             print indexing and search times on stderr
      --help             show this help
  -V, --version          show the version

Exit status: 0 if a line was printed or counted, 1 if none, 2 on an error.";

#[derive(Debug, Default, PartialEq)]
struct Args {
    pattern: String,
    files: Vec<PathBuf>,
    fixed: bool,
    ignore_case: bool,
    invert: bool,
    boolean: bool,
    line_numbers: bool,
    count: bool,
    /// `None`: only with several files.
    with_filename: Option<bool>,
    encoding: Option<Encoding>,
    hide_ansi: bool,
    time: bool,
}

#[derive(Debug, PartialEq)]
enum Parsed {
    Run(Args),
    Help,
    Version,
}

impl Args {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Parsed, String> {
        let mut out = Args::default();
        let mut pattern = None;
        let mut positional = Vec::new();
        let mut args = args.into_iter();
        let mut only_positional = false;
        while let Some(arg) = args.next() {
            let text = arg.to_string_lossy().into_owned();
            if only_positional || text == "-" || !text.starts_with('-') {
                positional.push(arg);
                continue;
            }
            if text == "--" {
                only_positional = true;
                continue;
            }
            let mut value = |name: &str| {
                args.next()
                    .map(|v| v.to_string_lossy().into_owned())
                    .ok_or_else(|| format!("{name} needs a value"))
            };
            // Long options with "=value".
            if let Some((name, v)) = text.split_once('=')
                && name.starts_with("--")
            {
                match name {
                    "--pattern" => pattern = Some(v.to_owned()),
                    "--encoding" => out.encoding = Some(encoding(v)?),
                    _ => return Err(format!("unknown option {name}")),
                }
                continue;
            }
            match text.as_str() {
                "--help" => return Ok(Parsed::Help),
                "-V" | "--version" => return Ok(Parsed::Version),
                "-e" | "--pattern" => pattern = Some(value(&text)?),
                "--encoding" => out.encoding = Some(encoding(&value(&text)?)?),
                "--fixed-strings" => out.fixed = true,
                "--ignore-case" => out.ignore_case = true,
                "--invert-match" => out.invert = true,
                "--boolean" => out.boolean = true,
                "--line-number" => out.line_numbers = true,
                "--count" => out.count = true,
                "--with-filename" => out.with_filename = Some(true),
                "--no-filename" => out.with_filename = Some(false),
                "--hide-ansi" => out.hide_ansi = true,
                "--time" => out.time = true,
                long if long.starts_with("--") => return Err(format!("unknown option {long}")),
                // Short flags, combined as in "-in"; "-e" may end them
                // ("-ie PATTERN").
                short => {
                    let flags: Vec<char> = short[1..].chars().collect();
                    for (k, flag) in flags.iter().enumerate() {
                        match flag {
                            'F' => out.fixed = true,
                            'i' => out.ignore_case = true,
                            'v' => out.invert = true,
                            'b' => out.boolean = true,
                            'n' => out.line_numbers = true,
                            'c' => out.count = true,
                            'H' => out.with_filename = Some(true),
                            'h' => out.with_filename = Some(false),
                            'V' => return Ok(Parsed::Version),
                            'e' => {
                                let rest: String = flags[k + 1..].iter().collect();
                                pattern = Some(if rest.is_empty() { value("-e")? } else { rest });
                                break;
                            }
                            _ => return Err(format!("unknown option -{flag}")),
                        }
                    }
                }
            }
        }
        let mut positional = positional.into_iter();
        out.pattern = match pattern {
            Some(p) => p,
            None => positional
                .next()
                .map(|p| p.to_string_lossy().into_owned())
                .ok_or("no pattern given")?,
        };
        out.files = positional.map(PathBuf::from).collect();
        if out.files.is_empty() {
            return Err("no file given".into());
        }
        Ok(Parsed::Run(out))
    }

    fn pattern(&self) -> Pattern {
        let mut p = if self.fixed {
            Pattern::plain(&self.pattern)
        } else {
            Pattern::regex(&self.pattern)
        };
        if self.ignore_case {
            p = p.case_insensitive();
        }
        if self.invert {
            p = p.excluding();
        }
        if self.boolean {
            p = p.boolean();
        }
        p
    }
}

fn encoding(label: &str) -> Result<Encoding, String> {
    Encoding::for_label(label).ok_or_else(|| format!("unknown encoding {label:?}"))
}

fn main() -> ExitCode {
    let args = match Args::parse(std::env::args_os().skip(1)) {
        Ok(Parsed::Run(args)) => args,
        Ok(Parsed::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::Version) => {
            println!("kloggr-grep {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("kloggr-grep: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let matcher = match Matcher::new(&args.pattern()) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("kloggr-grep: {e}");
            return ExitCode::from(2);
        }
    };
    let mut out = BufWriter::with_capacity(1 << 16, io::stdout().lock());
    let mut found = false;
    let mut failed = false;
    for path in &args.files {
        match grep(&args, &matcher, path, &mut out) {
            Ok(n) => found |= n > 0,
            // The reader went away (`| head`): stop quietly, as grep does.
            Err(Error::Io(e)) if e.kind() == ErrorKind::BrokenPipe => return ExitCode::SUCCESS,
            Err(e) => {
                let _ = out.flush();
                match e {
                    // "cannot open <path>: …" names the file already.
                    Error::Open(e) => eprintln!("kloggr-grep: {e}"),
                    Error::Io(e) => eprintln!("kloggr-grep: {}: {e}", path.display()),
                }
                failed = true;
            }
        }
    }
    match out.flush() {
        Err(e) if e.kind() != ErrorKind::BrokenPipe => {
            eprintln!("kloggr-grep: {e}");
            return ExitCode::from(2);
        }
        _ => {}
    }
    if failed {
        ExitCode::from(2)
    } else if found {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

#[derive(Debug)]
enum Error {
    Open(kloggr_core::file::OpenError),
    Io(io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Open(e) => e.fmt(f),
            Error::Io(e) => e.fmt(f),
        }
    }
}

/// Searches one file, writing its matching lines (or their count) to
/// `out`. Returns the number of matching lines.
fn grep(
    args: &Args,
    matcher: &Matcher,
    path: &PathBuf,
    out: &mut impl Write,
) -> Result<usize, Error> {
    let start = Instant::now();
    let options = OpenOptions {
        encoding: args.encoding,
        hide_ansi: args.hide_ansi,
        ..OpenOptions::default()
    };
    let log =
        LogFile::open_with(path, options, &AtomicBool::new(false), |_| {}).map_err(Error::Open)?;
    let indexed = start.elapsed();
    let with_filename = args.with_filename.unwrap_or(args.files.len() > 1);
    let prefix = with_filename.then(|| path.display().to_string());
    let encoding = log.encoding();

    let cancel = AtomicBool::new(false);
    let mut write_error = None;
    let mut matches = 0;
    let searching = Instant::now();
    let result = search::search(
        &log,
        matcher,
        0..log.line_count(),
        &cancel,
        |progress, lines| {
            matches = progress.matches;
            if args.count || write_error.is_some() {
                return;
            }
            if let Err(e) = write_lines(
                &log,
                encoding,
                lines,
                prefix.as_deref(),
                args.line_numbers,
                out,
            ) {
                write_error = Some(e);
                cancel.store(true, Ordering::Relaxed);
            }
        },
    );
    if let Some(e) = write_error {
        return Err(Error::Io(e));
    }
    // Only this function sets `cancel`, after a write error.
    debug_assert!(result.is_ok());
    let searched = searching.elapsed();
    if args.count {
        match &prefix {
            Some(p) => writeln!(out, "{p}:{matches}"),
            None => writeln!(out, "{matches}"),
        }
        .map_err(Error::Io)?;
    }
    if args.time {
        out.flush().map_err(Error::Io)?;
        eprintln!(
            "{}: {} lines, {} bytes, {encoding}: indexed in {:.1} ms, searched in {:.1} ms, {matches} matches",
            path.display(),
            log.line_count(),
            log.byte_len(),
            indexed.as_secs_f64() * 1000.,
            searched.as_secs_f64() * 1000.,
        );
    }
    Ok(matches)
}

fn write_lines(
    log: &LogFile,
    encoding: Encoding,
    lines: &[usize],
    prefix: Option<&str>,
    line_numbers: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    for &n in lines {
        let Some(line) = log.text_line(n) else {
            continue;
        };
        if let Some(p) = prefix {
            write!(out, "{p}:")?;
        }
        if line_numbers {
            write!(out, "{}:", n + 1)?;
        }
        // UTF-8 as it is, invalid bytes included (like grep); other
        // encodings converted to UTF-8.
        if encoding == Encoding::UTF_8 {
            out.write_all(&line)?;
        } else {
            out.write_all(encoding.decode(&line).as_bytes())?;
        }
        out.write_all(b"\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Parsed, String> {
        Args::parse(args.iter().map(OsString::from))
    }

    fn run(args: &[&str]) -> Args {
        match parse(args) {
            Ok(Parsed::Run(a)) => a,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn pattern_then_files() {
        let a = run(&["error", "a.log", "b.log"]);
        assert_eq!(a.pattern, "error");
        assert_eq!(a.files, [PathBuf::from("a.log"), PathBuf::from("b.log")]);
        assert_eq!(a.with_filename, None);
    }

    #[test]
    fn flags_combined_and_dash_e() {
        let a = run(&["-inv", "-e", "-x-", "a.log"]);
        assert!(a.ignore_case && a.line_numbers && a.invert && !a.fixed);
        assert_eq!(a.pattern, "-x-");
        assert_eq!(run(&["-ie", "foo", "a.log"]).pattern, "foo");
        assert_eq!(run(&["-efoo", "a.log"]).pattern, "foo");
        assert_eq!(run(&["--pattern=a=b", "a.log"]).pattern, "a=b");
        // klogg_grep's form.
        let a = run(&["-e", "foo", "a.log"]);
        assert_eq!((a.pattern.as_str(), a.files.len()), ("foo", 1));
    }

    #[test]
    fn long_options() {
        let a = run(&[
            "--fixed-strings",
            "--boolean",
            "--count",
            "--no-filename",
            "--encoding",
            "utf-16le",
            "--hide-ansi",
            "--time",
            "x",
            "a.log",
        ]);
        assert!(a.fixed && a.boolean && a.count && a.hide_ansi && a.time);
        assert_eq!(a.with_filename, Some(false));
        assert_eq!(a.encoding, Some(Encoding::UTF_16LE));
        assert_eq!(
            run(&["--encoding=latin1", "x", "a"]).encoding,
            Encoding::for_label("latin1")
        );
    }

    #[test]
    fn double_dash_ends_options() {
        let a = run(&["--", "-v", "-n.log"]);
        assert_eq!(a.pattern, "-v");
        assert!(!a.invert);
        assert_eq!(a.files, [PathBuf::from("-n.log")]);
    }

    #[test]
    fn errors_help_and_version() {
        assert!(parse(&[]).is_err());
        assert!(parse(&["foo"]).is_err(), "no file");
        assert!(parse(&["-e"]).is_err());
        assert!(parse(&["-q", "x", "a"]).is_err());
        assert!(parse(&["--nope", "x", "a"]).is_err());
        assert!(parse(&["--encoding", "klingon", "x", "a"]).is_err());
        assert_eq!(parse(&["--help"]), Ok(Parsed::Help));
        assert_eq!(parse(&["-V"]), Ok(Parsed::Version));
    }

    #[test]
    fn pattern_options() {
        let line = |a: &Args, text: &str| {
            Matcher::new(&a.pattern())
                .unwrap()
                .is_match(text.as_bytes())
        };
        let a = run(&["a.c", "f"]);
        assert!(line(&a, "abc") && !line(&a, "ABC"));
        let a = run(&["-Fi", "a.c", "f"]);
        assert!(line(&a, "A.C") && !line(&a, "abc"));
        let a = run(&["-v", "abc", "f"]);
        assert!(!line(&a, "abc") && line(&a, "xyz"));
        let a = run(&["-b", "\"a\" and not \"b\"", "f"]);
        assert!(line(&a, "a") && !line(&a, "ab"));
    }

    #[test]
    fn greps_a_utf16_file() {
        let dir = std::env::temp_dir().join(format!("kloggr-grep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("u16.log");
        let text = "one ok\ntwo error\nthree ok\nfour error été\n";
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        std::fs::write(&path, bytes).unwrap();

        let args = run(&["-n", "error", path.to_str().unwrap()]);
        let matcher = Matcher::new(&args.pattern()).unwrap();
        let mut out = Vec::new();
        assert_eq!(grep(&args, &matcher, &path, &mut out).unwrap(), 2);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "2:two error\n4:four error été\n"
        );

        let args = run(&["-cH", "ok", path.to_str().unwrap()]);
        let mut out = Vec::new();
        grep(
            &args,
            &Matcher::new(&args.pattern()).unwrap(),
            &path,
            &mut out,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("{}:2\n", path.display())
        );

        let missing = dir.join("missing.log");
        assert!(matches!(
            grep(&args, &matcher, &missing, &mut Vec::new()),
            Err(Error::Open(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
