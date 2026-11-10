//! The scratchpad's transformations (klogg: `ScratchPad`,
//! `src/ui/src/scratchpad.cpp`, GPLv3, Copyright (C) 2020 Anton Filimonov
//! and other contributors).
//!
//! Transformations of the text (or the selected part) that replace it, and
//! read-outs shown next to it. Each returns `None` when it doesn't apply
//! (klogg: an empty result, "Empty transformation").

use base64::Engine as _;

/// klogg: "From base64". Invalid UTF-8 in the result is replaced.
pub fn from_base64(text: &str) -> Option<String> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(compact.trim_end_matches('='))
        .or_else(|_| {
            base64::engine::general_purpose::STANDARD_NO_PAD.decode(compact.trim_end_matches('='))
        })
        .ok()?;
    non_empty(String::from_utf8_lossy(&bytes).into_owned())
}

/// klogg: "To base64".
pub fn to_base64(text: &str) -> Option<String> {
    non_empty(base64::engine::general_purpose::STANDARD.encode(text.as_bytes()))
}

/// klogg: "From hex" (`QByteArray::fromHex`: whitespace and other
/// characters between the digits are skipped).
pub fn from_hex(text: &str) -> Option<String> {
    let digits: Vec<u8> = text
        .chars()
        .filter_map(|c| c.to_digit(16).map(|d| d as u8))
        .collect();
    if digits.is_empty() {
        return None;
    }
    // An odd count is padded at the front, as Qt does.
    let start = digits.len() % 2;
    let mut bytes = Vec::with_capacity(digits.len() / 2 + 1);
    if start == 1 {
        bytes.push(digits[0]);
    }
    for pair in digits[start..].chunks(2) {
        bytes.push(pair[0] << 4 | pair[1]);
    }
    non_empty(String::from_utf8_lossy(&bytes).into_owned())
}

/// klogg: "To hex", lower case.
pub fn to_hex(text: &str) -> Option<String> {
    non_empty(text.bytes().map(|b| format!("{b:02x}")).collect())
}

/// klogg: "Decode url" (`QUrl::fromPercentEncoding`).
pub fn decode_url(text: &str) -> Option<String> {
    non_empty(
        percent_encoding::percent_decode_str(text)
            .decode_utf8_lossy()
            .into_owned(),
    )
}

/// klogg: "Format json": the first JSON value from the first `{` or `[`,
/// indented by 4 spaces (`QJsonDocument::Indented`). Keys keep their order;
/// klogg's `QJsonObject` sorts them.
pub fn format_json(text: &str) -> Option<String> {
    let start = text.find(['{', '['])?;
    let value: serde_json::Value = serde_json::Deserializer::from_str(&text[start..])
        .into_iter()
        .next()?
        .ok()?;
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut ser = serde_json::Serializer::with_formatter(&mut out, formatter);
    serde::Serialize::serialize(&value, &mut ser).ok()?;
    String::from_utf8(out).ok()
}

/// klogg: "Format xml": from the first `<`, indented by 2 spaces
/// (`QDomDocument::toString(2)`).
pub fn format_xml(text: &str) -> Option<String> {
    use quick_xml::events::Event;
    let start = text.find('<')?;
    let mut reader = quick_xml::Reader::from_str(&text[start..]);
    reader.config_mut().trim_text(true);
    let mut writer = quick_xml::Writer::new_with_indent(Vec::new(), b' ', 2);
    loop {
        match reader.read_event().ok()? {
            Event::Eof => break,
            event => writer.write_event(event).ok()?,
        }
    }
    non_empty(String::from_utf8(writer.into_inner()).ok()?)
}

/// The read-outs next to the text (klogg: the scratchpad's form).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Readouts {
    pub crc32_hex: String,
    pub crc32_dec: String,
    /// A Windows FILETIME (100 ns ticks since 1601) as UTC.
    pub file_time: String,
    pub dec_to_hex: String,
    pub hex_to_dec: String,
    /// Unix seconds as UTC.
    pub time: String,
}

impl Readouts {
    pub fn of(text: &str) -> Self {
        let crc = crc32fast::hash(text.as_bytes());
        let number = text.trim().parse::<i64>().ok();
        Self {
            crc32_hex: format!("0x{}", hex8(u64::from(crc))),
            crc32_dec: crc.to_string(),
            file_time: number
                .map(|ticks| ticks / FILE_TIME_TICKS - SECONDS_TO_EPOCH)
                .and_then(iso_utc)
                .unwrap_or_default(),
            dec_to_hex: number.map(|n| hex8(n as u64)).unwrap_or_default(),
            hex_to_dec: i64::from_str_radix(text.trim().trim_start_matches("0x"), 16)
                .map(|n| n.to_string())
                .unwrap_or_default(),
            time: number.and_then(iso_utc).unwrap_or_default(),
        }
    }
}

/// klogg: `windowsTickToUnixSeconds`.
const FILE_TIME_TICKS: i64 = 10_000_000;
const SECONDS_TO_EPOCH: i64 = 11_644_473_600;

/// klogg: `formatHex`, lower case, at least 8 digits.
fn hex8(value: u64) -> String {
    format!("{value:08x}")
}

/// Unix seconds as `YYYY-MM-DDTHH:MM:SSZ` (Qt::ISODate in UTC).
pub fn iso_utc(secs: i64) -> Option<String> {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (0..=9999).contains(&year).then(|| {
        format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
            rem / 3600,
            rem % 3600 / 60,
            rem % 60
        )
    })
}

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_and_hex() {
        assert_eq!(to_base64("hello").as_deref(), Some("aGVsbG8="));
        assert_eq!(from_base64("aGVsbG8=").as_deref(), Some("hello"));
        assert_eq!(from_base64("aGVs\nbG8").as_deref(), Some("hello"));
        assert_eq!(from_base64("***"), None);
        assert_eq!(to_hex("hi").as_deref(), Some("6869"));
        assert_eq!(from_hex("68 69").as_deref(), Some("hi"));
        assert_eq!(from_hex("zz"), None);
        assert_eq!(to_base64(""), None);
    }

    #[test]
    fn url_json_xml() {
        assert_eq!(decode_url("a%20b%2Fc").as_deref(), Some("a b/c"));
        assert_eq!(
            format_json(r#"payload={"b":1,"a":[1,2]} trailing"#).as_deref(),
            Some("{\n    \"b\": 1,\n    \"a\": [\n        1,\n        2\n    ]\n}")
        );
        assert_eq!(format_json("no json"), None);
        assert_eq!(
            format_xml("msg=<a><b x=\"1\">t</b></a>").as_deref(),
            Some("<a>\n  <b x=\"1\">t</b>\n</a>")
        );
        assert_eq!(format_xml("<a><b></a>"), None);
    }

    #[test]
    fn readouts() {
        let r = Readouts::of("123456789");
        assert_eq!(r.crc32_hex, "0xcbf43926", "CRC32 check value");
        assert_eq!(r.crc32_dec, "3421780262");
        assert_eq!(r.dec_to_hex, "075bcd15");
        assert_eq!(r.time, "1973-11-29T21:33:09Z");
        assert_eq!(Readouts::of("ff").hex_to_dec, "255");
        assert_eq!(Readouts::of("0x10").hex_to_dec, "16");
        // 2026-11-10T00:00:00Z as a FILETIME.
        let ticks = (1_794_268_800i64 + SECONDS_TO_EPOCH) * FILE_TIME_TICKS;
        assert_eq!(
            Readouts::of(&ticks.to_string()).file_time,
            "2026-11-10T00:00:00Z"
        );
        let r = Readouts::of("text");
        assert_eq!((r.time.as_str(), r.dec_to_hex.as_str()), ("", ""));
        assert_eq!(iso_utc(0).as_deref(), Some("1970-01-01T00:00:00Z"));
        assert_eq!(iso_utc(-1).as_deref(), Some("1969-12-31T23:59:59Z"));
        assert_eq!(
            iso_utc(951_782_400).as_deref(),
            Some("2000-02-29T00:00:00Z")
        );
        assert_eq!(iso_utc(i64::MAX), None);
    }
}
