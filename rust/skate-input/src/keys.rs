//! Key names the skate controls are written with, and their Windows
//! virtual-key codes.

/// Every key name the controls accept, with its virtual-key code (mouse
/// buttons included, as Windows numbers them).
pub const KEYS: &[(&str, u32)] = &[
    ("Mouse1", 0x01),
    ("Mouse2", 0x02),
    ("Mouse3", 0x04),
    ("Mouse4", 0x05),
    ("Mouse5", 0x06),
    ("Backspace", 0x08),
    ("Tab", 0x09),
    ("Enter", 0x0D),
    ("CapsLock", 0x14),
    ("Escape", 0x1B),
    ("Space", 0x20),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("End", 0x23),
    ("Home", 0x24),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("Insert", 0x2D),
    ("Delete", 0x2E),
    ("0", 0x30),
    ("1", 0x31),
    ("2", 0x32),
    ("3", 0x33),
    ("4", 0x34),
    ("5", 0x35),
    ("6", 0x36),
    ("7", 0x37),
    ("8", 0x38),
    ("9", 0x39),
    ("A", 0x41),
    ("B", 0x42),
    ("C", 0x43),
    ("D", 0x44),
    ("E", 0x45),
    ("F", 0x46),
    ("G", 0x47),
    ("H", 0x48),
    ("I", 0x49),
    ("J", 0x4A),
    ("K", 0x4B),
    ("L", 0x4C),
    ("M", 0x4D),
    ("N", 0x4E),
    ("O", 0x4F),
    ("P", 0x50),
    ("Q", 0x51),
    ("R", 0x52),
    ("S", 0x53),
    ("T", 0x54),
    ("U", 0x55),
    ("V", 0x56),
    ("W", 0x57),
    ("X", 0x58),
    ("Y", 0x59),
    ("Z", 0x5A),
    ("Num0", 0x60),
    ("Num1", 0x61),
    ("Num2", 0x62),
    ("Num3", 0x63),
    ("Num4", 0x64),
    ("Num5", 0x65),
    ("Num6", 0x66),
    ("Num7", 0x67),
    ("Num8", 0x68),
    ("Num9", 0x69),
    ("NumMultiply", 0x6A),
    ("NumPlus", 0x6B),
    ("NumMinus", 0x6D),
    ("NumPeriod", 0x6E),
    ("NumDivide", 0x6F),
    ("F1", 0x70),
    ("F2", 0x71),
    ("F3", 0x72),
    ("F4", 0x73),
    ("F5", 0x74),
    ("F6", 0x75),
    ("F7", 0x76),
    ("F8", 0x77),
    ("F9", 0x78),
    ("F10", 0x79),
    ("F11", 0x7A),
    ("F12", 0x7B),
    ("LShift", 0xA0),
    ("RShift", 0xA1),
    ("LCtrl", 0xA2),
    ("RCtrl", 0xA3),
    ("LAlt", 0xA4),
    ("RAlt", 0xA5),
    ("Semicolon", 0xBA),
    ("Equals", 0xBB),
    ("Comma", 0xBC),
    ("Minus", 0xBD),
    ("Period", 0xBE),
    ("Slash", 0xBF),
    ("Grave", 0xC0),
    ("LBracket", 0xDB),
    ("Backslash", 0xDC),
    ("RBracket", 0xDD),
    ("Quote", 0xDE),
];

/// The canonical spelling of a key name, ignoring case.
pub fn canonical(name: &str) -> Option<&'static str> {
    let name = name.trim();
    KEYS.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(key, _)| *key)
}

/// A key's Windows virtual-key code.
pub fn virtual_key(name: &str) -> Option<u32> {
    let name = name.trim();
    KEYS.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, vk)| *vk)
}

/// The key name of a Windows virtual-key code.
pub fn from_virtual_key(vk: u32) -> Option<&'static str> {
    KEYS.iter().find(|(_, code)| *code == vk).map(|(key, _)| *key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_round_trip() {
        for (i, (name, vk)) in KEYS.iter().enumerate() {
            assert!(KEYS[i + 1..].iter().all(|(other, code)| other != name && code != vk), "{name}");
            assert_eq!(from_virtual_key(*vk), Some(*name));
            assert_eq!(canonical(&name.to_lowercase()), Some(*name));
        }
        assert_eq!(virtual_key("space"), Some(0x20));
        assert_eq!(canonical("Hyper"), None);
    }
}
