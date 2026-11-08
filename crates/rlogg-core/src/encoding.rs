//! Text encodings: detection, line-feed layout and decoding.
//!
//! Port of klogg's `EncodingDetector` / `EncodingParameters`
//! (`src/logdata/src/encodingdetector.cpp`, GPLv3). klogg asks uchardet for a
//! guess and lets a byte-order mark override it (`QTextCodec::codecForUtfText`).
//! Here:
//! 1. a BOM (UTF-8, UTF-16LE, UTF-16BE) wins;
//! 2. BOM-less UTF-16 is recognised by its NUL bytes (ASCII-heavy logs);
//! 3. valid UTF-8, including pure ASCII, is UTF-8;
//! 4. anything else goes to `chardetng` (legacy single/multi-byte encodings).
//!
//! UTF-32 is not supported (encoding_rs has no decoder for it).

use std::borrow::Cow;
use std::fmt;

/// Bytes sampled from the start of a file for detection.
pub const DETECTION_SAMPLE: usize = 1 << 20;

/// How a line feed is encoded (klogg: `EncodingParameters::lineFeedWidth`,
/// `lineFeedIndex`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineFeed {
    /// Bytes per code unit: 1, or 2 for UTF-16.
    pub width: u8,
    /// Position of the `0x0A` byte within its code unit (1 for UTF-16BE).
    pub index: u8,
}

impl LineFeed {
    pub const SINGLE_BYTE: Self = Self { width: 1, index: 0 };
    pub const UTF_16LE: Self = Self { width: 2, index: 0 };
    pub const UTF_16BE: Self = Self { width: 2, index: 1 };

    /// Whether the code unit ending at byte `end` (exclusive) is `\n`.
    /// `end` is relative to an aligned buffer (`end % width == 0`).
    pub fn ends_with_lf(&self, data: &[u8], end: usize) -> bool {
        self.unit_before(data, end) == Some(b'\n')
    }

    /// The ASCII value of the code unit ending at `end`, if it is ASCII.
    fn unit_before(&self, data: &[u8], end: usize) -> Option<u8> {
        let w = self.width as usize;
        let unit = data.get(end.checked_sub(w)?..end)?;
        let (i, b) = (self.index as usize, unit[self.index as usize]);
        let rest_zero = unit.iter().enumerate().all(|(k, &x)| k == i || x == 0);
        (rest_zero && b < 0x80).then_some(b)
    }

    /// Strips a trailing `\n` and a `\r` before it from an aligned line.
    #[inline]
    pub fn strip_eol<'a>(&self, line: &'a [u8]) -> &'a [u8] {
        if self.width == 1 {
            // Hot path: called once per line while indexing.
            return match line.strip_suffix(b"\n") {
                Some(l) => l.strip_suffix(b"\r").unwrap_or(l),
                None => line,
            };
        }
        let w = self.width as usize;
        let mut end = line.len() - line.len() % w;
        if self.unit_before(line, end) == Some(b'\n') {
            end -= w;
            if self.unit_before(line, end) == Some(b'\r') {
                end -= w;
            }
        }
        &line[..end]
    }
}

/// A text encoding supported by rlogg.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Encoding(&'static encoding_rs::Encoding);

impl Encoding {
    pub const UTF_8: Self = Self(encoding_rs::UTF_8);
    pub const UTF_16LE: Self = Self(encoding_rs::UTF_16LE);
    pub const UTF_16BE: Self = Self(encoding_rs::UTF_16BE);
    /// Latin-1. As in browsers, ISO-8859-1 is decoded as windows-1252.
    pub const WINDOWS_1252: Self = Self(encoding_rs::WINDOWS_1252);

    /// Looks up an encoding by WHATWG label, e.g. `"utf-8"`, `"latin1"`,
    /// `"shift_jis"`.
    pub fn for_label(label: &str) -> Option<Self> {
        encoding_rs::Encoding::for_label(label.trim().as_bytes()).map(Self)
    }

    /// Canonical name, e.g. `"UTF-16LE"`.
    pub fn name(&self) -> &'static str {
        self.0.name()
    }

    pub fn line_feed(&self) -> LineFeed {
        if *self == Self::UTF_16LE {
            LineFeed::UTF_16LE
        } else if *self == Self::UTF_16BE {
            LineFeed::UTF_16BE
        } else {
            LineFeed::SINGLE_BYTE
        }
    }

    /// Length of this encoding's byte-order mark at the start of `data`, or 0.
    pub fn bom_len(&self, data: &[u8]) -> usize {
        match encoding_rs::Encoding::for_bom(data) {
            Some((enc, len)) if enc == self.0 => len,
            _ => 0,
        }
    }

    /// Decodes one line (without terminator). Invalid sequences become U+FFFD.
    pub fn decode<'a>(&self, bytes: &'a [u8]) -> Cow<'a, str> {
        self.0.decode_without_bom_handling(bytes).0
    }
}

impl fmt::Debug for Encoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl fmt::Display for Encoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Guesses the encoding of a file from its first bytes (klogg:
/// `EncodingDetector::detectEncoding`). Pass up to [`DETECTION_SAMPLE`] bytes.
pub fn detect(sample: &[u8]) -> Encoding {
    if let Some((enc, _)) = encoding_rs::Encoding::for_bom(sample) {
        return Encoding(enc);
    }
    if let Some(enc) = detect_bomless_utf16(sample) {
        return enc;
    }
    if is_utf8(sample) {
        return Encoding::UTF_8;
    }
    // Logs cannot run scripts, so ISO-2022-JP is a safe guess (unlike in a
    // browser).
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
    detector.feed(sample, true);
    Encoding(detector.guess(None, chardetng::Utf8Detection::Allow))
}

/// UTF-8 check that tolerates a sequence cut off by the end of the sample.
fn is_utf8(sample: &[u8]) -> bool {
    match std::str::from_utf8(sample) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none() && sample.len() - e.valid_up_to() < 4,
    }
}

/// Logs are mostly ASCII, so UTF-16 text has a NUL in one byte of most code
/// units and almost never in the other. Text in other encodings has no NULs.
fn detect_bomless_utf16(sample: &[u8]) -> Option<Encoding> {
    let units = sample.len() / 2;
    if units == 0 {
        return None;
    }
    let (mut even, mut odd) = (0usize, 0usize);
    for pair in sample.as_chunks::<2>().0 {
        even += usize::from(pair[0] == 0);
        odd += usize::from(pair[1] == 0);
    }
    let mostly = |n: usize| n * 10 >= units * 6;
    let rarely = |n: usize| n * 100 <= units;
    if mostly(odd) && rarely(even) {
        Some(Encoding::UTF_16LE)
    } else if mostly(even) && rarely(odd) {
        Some(Encoding::UTF_16BE)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> Vec<u8> {
        let path = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    #[test]
    fn detects_klogg_samples() {
        for (file, expected) in [
            ("random_block_100byte.txt", Encoding::UTF_8),
            ("utf8_tab_test.txt", Encoding::UTF_8),
            ("random_block_100_byte_utf16le.txt", Encoding::UTF_16LE),
            ("random_block_100_byte_utf16be.txt", Encoding::UTF_16BE),
            ("utf16_tab_test.txt", Encoding::UTF_16LE),
            ("utf16be_tab_test.txt", Encoding::UTF_16BE),
            ("Chinese-Lipsum.utf16.txt", Encoding::UTF_16LE),
            ("Chinese-Lipsum.utf16be.txt", Encoding::UTF_16BE),
        ] {
            assert_eq!(detect(&sample(file)), expected, "{file}");
        }
    }

    #[test]
    fn detects_utf8_bom_and_plain_text() {
        assert_eq!(detect(b"\xEF\xBB\xBFhello\n"), Encoding::UTF_8);
        assert_eq!(detect(b"plain ascii\n"), Encoding::UTF_8);
        assert_eq!(detect(b""), Encoding::UTF_8);
        assert_eq!(detect("caf\u{e9} \u{2603}\n".as_bytes()), Encoding::UTF_8);
        // Multi-byte sequence cut by the sample boundary.
        assert_eq!(detect(&"ab\u{2603}".as_bytes()[..4]), Encoding::UTF_8);
    }

    #[test]
    fn detects_legacy_encodings() {
        let latin1 = b"Fran\xe7ois a \xe9t\xe9 \xe0 la plage. Caf\xe9 cr\xe8me br\xfbl\xe9e.\n";
        assert_eq!(detect(&latin1.repeat(20)), Encoding::WINDOWS_1252);
        let (sjis, _, _) =
            encoding_rs::SHIFT_JIS.encode("ログファイルを開きました。エラーが発生しました。\n");
        assert_eq!(detect(&sjis.repeat(20)).name(), "Shift_JIS");
    }

    #[test]
    fn labels() {
        assert_eq!(Encoding::for_label("latin1"), Some(Encoding::WINDOWS_1252));
        assert_eq!(Encoding::for_label(" UTF-16 "), Some(Encoding::UTF_16LE));
        assert_eq!(Encoding::for_label("nope"), None);
    }

    #[test]
    fn line_feed_layout() {
        assert_eq!(Encoding::UTF_8.line_feed(), LineFeed::SINGLE_BYTE);
        assert_eq!(Encoding::WINDOWS_1252.line_feed(), LineFeed::SINGLE_BYTE);
        assert_eq!(Encoding::UTF_16LE.line_feed(), LineFeed::UTF_16LE);
        assert_eq!(Encoding::UTF_16BE.line_feed(), LineFeed::UTF_16BE);
    }

    #[test]
    fn strip_eol_per_layout() {
        let le = LineFeed::UTF_16LE;
        assert_eq!(le.strip_eol(b"a\0\r\0\n\0"), b"a\0");
        assert_eq!(le.strip_eol(b"a\0\n\0"), b"a\0");
        // U+0A0A is not a line feed.
        assert_eq!(le.strip_eol(b"a\0\n\n"), b"a\0\n\n");
        let be = LineFeed::UTF_16BE;
        assert_eq!(be.strip_eol(b"\0a\0\r\0\n"), b"\0a");
        assert_eq!(be.strip_eol(b"\0a\n\0"), b"\0a\n\0");
        let single = LineFeed::SINGLE_BYTE;
        assert_eq!(single.strip_eol(b"x\r\n"), b"x");
        assert_eq!(single.strip_eol(b"x\n"), b"x");
        assert_eq!(single.strip_eol(b"x\r"), b"x\r");
        assert_eq!(le.strip_eol(b"x\0\r\0"), b"x\0\r\0");
    }

    #[test]
    fn bom_len_only_for_matching_encoding() {
        assert_eq!(Encoding::UTF_16LE.bom_len(b"\xFF\xFEa\0"), 2);
        assert_eq!(Encoding::UTF_8.bom_len(b"\xFF\xFEa\0"), 0);
        assert_eq!(Encoding::UTF_8.bom_len(b"\xEF\xBB\xBFa"), 3);
    }
}
