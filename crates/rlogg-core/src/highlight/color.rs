//! RGBA colours as stored in highlighter settings.
//!
//! klogg stores `QColor`s as `#AARRGGBB` strings (`QColor::HexArgb`); rlogg
//! reads and writes the same form, and also reads `#RRGGBB`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Parses `#RRGGBB` or `#AARRGGBB` (case-insensitive).
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.trim().strip_prefix('#')?;
        if !hex.is_ascii() {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        match hex.len() {
            6 => Some(Self::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(Self {
                a: byte(0)?,
                r: byte(2)?,
                g: byte(4)?,
                b: byte(6)?,
            }),
            _ => None,
        }
    }

    /// `#aarrggbb`, as Qt's `QColor::name(QColor::HexArgb)`.
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}{:02x}", self.a, self.r, self.g, self.b)
    }

    /// Qt's `QColor::darker(factor)`: divides the HSV value by
    /// `factor / 100`. A factor below 100 makes the colour lighter.
    pub fn darker(self, factor: u32) -> Self {
        if factor == 0 {
            return self;
        }
        if factor < 100 {
            return self.lighter(10_000 / factor);
        }
        let (h, s, v) = self.hsv();
        self.with_hsv(h, s, v * 100. / factor as f32)
    }

    /// Qt's `QColor::lighter(factor)`: multiplies the HSV value by
    /// `factor / 100`; past full brightness, the saturation drops instead.
    pub fn lighter(self, factor: u32) -> Self {
        if factor == 0 {
            return self;
        }
        if factor < 100 {
            return self.darker(10_000 / factor);
        }
        let (h, mut s, mut v) = self.hsv();
        v = v * factor as f32 / 100.;
        if v > 255. {
            s = (s - (v - 255.)).max(0.);
            v = 255.;
        }
        self.with_hsv(h, s, v)
    }

    /// Hue in degrees, saturation and value in 0..=255.
    fn hsv(self) -> (f32, f32, f32) {
        let (r, g, b) = (f32::from(self.r), f32::from(self.g), f32::from(self.b));
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let s = if max > 0. { delta * 255. / max } else { 0. };
        let h = if delta == 0. {
            0.
        } else if max == r {
            60. * ((g - b) / delta).rem_euclid(6.)
        } else if max == g {
            60. * ((b - r) / delta + 2.)
        } else {
            60. * ((r - g) / delta + 4.)
        };
        (h, s, max)
    }

    fn with_hsv(self, h: f32, s: f32, v: f32) -> Self {
        let (s, v) = (s.clamp(0., 255.) / 255., v.clamp(0., 255.));
        let c = v * s;
        let x = c * (1. - ((h / 60.).rem_euclid(2.) - 1.).abs());
        let m = v - c;
        let (r, g, b) = match (h / 60.) as u32 % 6 {
            0 => (c, x, 0.),
            1 => (x, c, 0.),
            2 => (0., c, x),
            3 => (0., x, c),
            4 => (x, 0., c),
            _ => (c, 0., x),
        };
        let channel = |f: f32| (f + m).round().clamp(0., 255.) as u8;
        Self {
            r: channel(r),
            g: channel(g),
            b: channel(b),
            a: self.a,
        }
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Self::parse(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid colour {text:?}")))
    }
}

/// Serde helpers for an optional colour: `""` (klogg: an invalid `QColor`)
/// means none.
pub(crate) mod optional {
    use super::Color;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(c: &Option<Color>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&c.map(Color::to_hex).unwrap_or_default())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Color>, D::Error> {
        let text = String::deserialize(d)?;
        if text.trim().is_empty() {
            return Ok(None);
        }
        Color::parse(&text)
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid colour {text:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_format() {
        assert_eq!(Color::parse("#001e80"), Some(Color::rgb(0, 0x1e, 0x80)));
        assert_eq!(
            Color::parse("#80A1B7FF"),
            Some(Color {
                a: 0x80,
                r: 0xa1,
                g: 0xb7,
                b: 0xff
            })
        );
        assert_eq!(Color::rgb(0xa1, 0xb7, 0xff).to_hex(), "#ffa1b7ff");
        assert_eq!(Color::parse("a1b7ff"), None);
        assert_eq!(Color::parse("#a1b7f"), None);
        assert_eq!(Color::parse("#zzzzzz"), None);
        assert_eq!(Color::parse("#ééé"), None);
    }

    #[test]
    fn darker_and_lighter_follow_qt() {
        let c = Color::rgb(200, 100, 50);
        // Value 200 -> 100; hue and saturation kept.
        assert_eq!(c.darker(200), Color::rgb(100, 50, 25));
        assert_eq!(c.darker(100), c);
        assert_eq!(Color::rgb(100, 50, 25).lighter(200), c);
        // Lighter past full value lowers the saturation.
        let white_ish = Color::rgb(200, 200, 200).lighter(200);
        assert_eq!(white_ish, Color::rgb(255, 255, 255));
        // A factor below 100 inverts.
        assert_eq!(c.darker(50), c.lighter(200));
        assert_eq!(Color::rgb(0, 0, 0).lighter(150), Color::rgb(0, 0, 0));
    }
}
