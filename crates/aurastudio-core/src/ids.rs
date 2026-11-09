//! Stable 128-bit ids (documents, sheets, scenes, palettes): written as a
//! UUID, kept across renames and moves. `core` has no randomness: the app
//! makes them (from the system's random source) and hands them down.

use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(pub u128);

impl Id {
    #[cfg(test)]
    pub const NIL: Id = Id(0);

    #[cfg(test)]
    pub fn is_nil(self) -> bool { self.0 == 0 }

    /// From 16 random bytes, as a version 4 UUID.
    #[cfg(test)]
    pub fn from_random(b: [u8; 16]) -> Id {
        let mut v = u128::from_be_bytes(b);
        v = (v & !(0xf << 76)) | (0x4 << 76);
        v = (v & !(0x3 << 62)) | (0x2 << 62);
        Id(v)
    }

    pub fn to_bytes(self) -> [u8; 16] { self.0.to_be_bytes() }
}

impl fmt::Display for Id {
    /// `8-4-4-4-12` lowercase hex.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let h = format!("{:032x}", self.0);
        write!(f, "{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
    }
}

impl FromStr for Id {
    type Err = ();
    /// With or without dashes, any case.
    fn from_str(s: &str) -> Result<Id, ()> {
        let hex: String = s.chars().filter(|c| *c != '-').collect();
        if hex.len() != 32 { return Err(()); }
        u128::from_str_radix(&hex, 16).map(Id).map_err(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trip() {
        let id = Id::from_random([0xab; 16]);
        let s = id.to_string();
        assert_eq!(s.len(), 36);
        assert_eq!(&s[14..15], "4", "version 4");
        assert_eq!(s.parse::<Id>(), Ok(id));
        assert_eq!(s.replace('-', "").to_uppercase().parse::<Id>(), Ok(id));
        assert!("nope".parse::<Id>().is_err());
        assert!(Id::NIL.is_nil());
    }
}
