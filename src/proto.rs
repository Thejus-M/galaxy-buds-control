//! Galaxy Buds2 SPP message layer: ids, encoders and the state decoder.
//!
//! Framing and CRC come from `galaxy_buds_rs`; the Buds2 payload layouts and the
//! messages that crate does not model are implemented here, mirrored from
//! GalaxyBudsClient's encoders and `ExtendedStatusUpdateDecoder`.

use galaxy_buds_rs::message::Payload;
use serde::{Deserialize, Serialize};

/// Message ids used by this crate (see GalaxyBudsClient `SppMessageEnums.cs`).
pub mod ids {
    pub const STATUS_UPDATED: u8 = 96;
    pub const EXTENDED_STATUS_UPDATED: u8 = 97;
    pub const VERSION_INFO: u8 = 99;
    pub const SET_CALL_PATH_CONTROL: u8 = 110;
    pub const SET_ANC_WITH_ONE_EARBUD: u8 = 111;
    pub const NOISE_CONTROLS_UPDATE: u8 = 119;
    pub const NOISE_CONTROLS: u8 = 120;
    pub const SET_TOUCH_AND_HOLD_NOISE_CONTROLS: u8 = 121;
    pub const CUSTOMIZE_AMBIENT_SOUND: u8 = 130;
    pub const AMBIENT_VOLUME: u8 = 132;
    pub const EQUALIZER: u8 = 134;
    pub const GAME_MODE: u8 = 135;
    pub const MANAGER_INFO: u8 = 136;
    pub const SET_SIDETONE: u8 = 139;
    pub const LOCK_TOUCHPAD: u8 = 144;
    pub const TOUCH_UPDATED: u8 = 145;
    pub const SET_TOUCHPAD_OPTION: u8 = 146;
    pub const SET_TOUCHPAD_OTHER_OPTION: u8 = 147;
    pub const EXTRA_HIGH_AMBIENT: u8 = 150;
    pub const FIND_MY_EARBUDS_START: u8 = 160;
    pub const FIND_MY_EARBUDS_STOP: u8 = 161;
    pub const SET_SPATIAL_AUDIO: u8 = 124;
}

/// A request with no payload; the buds answer with the matching `*_UPDATED`.
pub struct Request(pub u8);

impl Payload for Request {
    fn get_id(&self) -> u8 {
        self.0
    }
}

/// A message with an explicit payload.
pub struct Raw(pub u8, pub Vec<u8>);

impl Payload for Raw {
    fn get_id(&self) -> u8 {
        self.0
    }
    fn get_data(&self) -> Vec<u8> {
        self.1.clone()
    }
}

impl Raw {
    pub fn byte(id: u8, value: u8) -> Self {
        Raw(id, vec![value])
    }

    pub fn flag(id: u8, on: bool) -> Self {
        Raw(id, vec![u8::from(on)])
    }
}

/// The host handshake the Android app sends (`ManagerInfoEncoder`).
pub fn manager_info() -> Raw {
    Raw(ids::MANAGER_INFO, vec![1, 1, 34])
}

/// Noise control mode. Buds2 has no adaptive mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NoiseMode {
    Off,
    Anc,
    Ambient,
}

impl NoiseMode {
    pub fn encode(self) -> u8 {
        match self {
            NoiseMode::Off => 0,
            NoiseMode::Anc => 1,
            NoiseMode::Ambient => 2,
        }
    }

    pub fn decode(b: u8) -> Self {
        match b {
            1 => NoiseMode::Anc,
            2 => NoiseMode::Ambient,
            _ => NoiseMode::Off,
        }
    }

    /// Order used when cycling from a keybind or a bar click.
    pub fn next(self) -> Self {
        match self {
            NoiseMode::Off => NoiseMode::Anc,
            NoiseMode::Anc => NoiseMode::Ambient,
            NoiseMode::Ambient => NoiseMode::Off,
        }
    }

    pub fn all() -> [NoiseMode; 3] {
        [NoiseMode::Off, NoiseMode::Anc, NoiseMode::Ambient]
    }
}

/// Touch-and-hold action, `StandardTouchMap` in GalaxyBudsClient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TouchAction {
    VoiceAssistant,
    NoiseControl,
    Volume,
    Spotify,
    /// "Other" is side-specific: 5 on the left earbud, 6 on the right.
    Other,
}

impl TouchAction {
    pub fn encode(self, side: Side) -> u8 {
        match self {
            TouchAction::VoiceAssistant => 1,
            TouchAction::NoiseControl => 2,
            TouchAction::Volume => 3,
            TouchAction::Spotify => 4,
            TouchAction::Other => match side {
                Side::Left => 5,
                Side::Right => 6,
            },
        }
    }

    pub fn decode(b: u8) -> Option<Self> {
        Some(match b {
            1 => TouchAction::VoiceAssistant,
            2 => TouchAction::NoiseControl,
            3 => TouchAction::Volume,
            4 => TouchAction::Spotify,
            5 | 6 => TouchAction::Other,
            _ => return None,
        })
    }

    pub fn all() -> [TouchAction; 5] {
        [
            TouchAction::VoiceAssistant,
            TouchAction::NoiseControl,
            TouchAction::Volume,
            TouchAction::Spotify,
            TouchAction::Other,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Side {
    Left,
    Right,
}

/// Equalizer preset. On the wire 0 means "no EQ" and the presets are
/// `EqPresets + 1` (see `SetEqualizerEncoder` in GalaxyBudsClient).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Equalizer {
    Normal,
    BassBoost,
    Soft,
    Dynamic,
    Clear,
    TrebleBoost,
}

impl Equalizer {
    pub fn encode(self) -> u8 {
        match self {
            Equalizer::Normal => 0,
            Equalizer::BassBoost => 1,
            Equalizer::Soft => 2,
            Equalizer::Dynamic => 3,
            Equalizer::Clear => 4,
            Equalizer::TrebleBoost => 5,
        }
    }

    pub fn decode(b: u8) -> Self {
        match b {
            1 => Equalizer::BassBoost,
            2 => Equalizer::Soft,
            3 => Equalizer::Dynamic,
            4 => Equalizer::Clear,
            5 => Equalizer::TrebleBoost,
            _ => Equalizer::Normal,
        }
    }

    pub fn all() -> [Equalizer; 6] {
        [
            Equalizer::Normal,
            Equalizer::BassBoost,
            Equalizer::Soft,
            Equalizer::Dynamic,
            Equalizer::Clear,
            Equalizer::TrebleBoost,
        ]
    }
}

/// Which modes the long-press gesture cycles through, per earbud pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HoldModes {
    pub off: bool,
    pub ambient: bool,
    pub anc: bool,
}

impl HoldModes {
    /// Payload bit layout of `SET_TOUCH_AND_HOLD_NOISE_CONTROLS` for Buds2:
    /// right earbud in bits 0-2, left earbud in bits 4-6.
    pub fn encode(self) -> u8 {
        let mut b = 0u8;
        if self.off {
            b |= 1 << 0 | 1 << 4;
        }
        if self.ambient {
            b |= 1 << 1 | 1 << 5;
        }
        if self.anc {
            b |= 1 << 2 | 1 << 6;
        }
        b
    }

    pub fn decode(b: u8) -> Self {
        HoldModes {
            off: b & (1 << 0) != 0,
            ambient: b & (1 << 1) != 0,
            anc: b & (1 << 2) != 0,
        }
    }
}

/// Which touch gestures are enabled (`msg.Payload[10]` bit flags on Buds2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TouchFlags {
    pub touch_hold: bool,
    pub triple_tap: bool,
    pub double_tap: bool,
    pub single_tap: bool,
    pub double_tap_for_call: bool,
    pub touch_hold_for_call: bool,
    /// The buds report the lock inverted: bit 7 clear means locked.
    pub locked: bool,
}

impl TouchFlags {
    pub fn decode(b: u8) -> Self {
        TouchFlags {
            touch_hold: b & (1 << 0) != 0,
            triple_tap: b & (1 << 1) != 0,
            double_tap: b & (1 << 2) != 0,
            single_tap: b & (1 << 3) != 0,
            double_tap_for_call: b & (1 << 4) != 0,
            touch_hold_for_call: b & (1 << 5) != 0,
            locked: b & (1 << 7) == 0,
        }
    }

    /// `LOCK_TOUCHPAD` payload for the advanced (Buds2) form: the same bit
    /// layout, with bit 7 set when the touchpad is unlocked.
    pub fn encode(self) -> u8 {
        let mut b = 0u8;
        if self.touch_hold {
            b |= 1 << 0;
        }
        if self.triple_tap {
            b |= 1 << 1;
        }
        if self.double_tap {
            b |= 1 << 2;
        }
        if self.single_tap {
            b |= 1 << 3;
        }
        if self.double_tap_for_call {
            b |= 1 << 4;
        }
        if self.touch_hold_for_call {
            b |= 1 << 5;
        }
        if !self.locked {
            b |= 1 << 7;
        }
        b
    }

    /// All gestures on, matching the phone app's default.
    pub fn all_on(locked: bool) -> Self {
        TouchFlags {
            touch_hold: true,
            triple_tap: true,
            double_tap: true,
            single_tap: true,
            double_tap_for_call: true,
            touch_hold_for_call: true,
            locked,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use galaxy_buds_rs::message::{Message, BOM, EOM};
    use galaxy_buds_rs::model::Model;

    #[test]
    fn frames_carry_a_valid_header_and_crc() {
        let bytes = Raw::byte(ids::NOISE_CONTROLS, 1).to_byte_array();

        assert_eq!(bytes[0], BOM);
        assert_eq!(bytes[bytes.len() - 1], EOM);
        // size = id + payload + crc
        assert_eq!(u16::from_le_bytes([bytes[1], bytes[2]]) & 0x03ff, 4);
        assert_eq!(bytes[3], ids::NOISE_CONTROLS);
        assert_eq!(bytes[4], 1);
        assert!(Message::new(bytes, Model::Buds2).check_crc());
    }

    #[test]
    fn manager_info_matches_the_android_handshake() {
        let msg = manager_info();
        assert_eq!(msg.get_id(), ids::MANAGER_INFO);
        assert_eq!(msg.get_data(), vec![1, 1, 34]);
    }

    #[test]
    fn noise_modes_use_the_firmware_numbering() {
        assert_eq!(NoiseMode::Off.encode(), 0);
        assert_eq!(NoiseMode::Anc.encode(), 1);
        assert_eq!(NoiseMode::Ambient.encode(), 2);
        for mode in NoiseMode::all() {
            assert_eq!(NoiseMode::decode(mode.encode()), mode);
        }
        assert_eq!(NoiseMode::Off.next(), NoiseMode::Anc);
        assert_eq!(NoiseMode::Ambient.next(), NoiseMode::Off);
    }

    #[test]
    fn equalizer_reserves_zero_for_no_preset() {
        assert_eq!(Equalizer::Normal.encode(), 0);
        for eq in Equalizer::all() {
            assert_eq!(Equalizer::decode(eq.encode()), eq);
        }
    }

    #[test]
    fn touch_other_is_side_specific() {
        assert_eq!(TouchAction::Other.encode(Side::Left), 5);
        assert_eq!(TouchAction::Other.encode(Side::Right), 6);
        assert_eq!(TouchAction::decode(2), Some(TouchAction::NoiseControl));
        assert_eq!(TouchAction::decode(9), None);
    }

    #[test]
    fn hold_modes_round_trip_both_earbuds() {
        // 0x35: right = off + anc, left = off + ambient (as read from the buds).
        let modes = HoldModes::decode(0x35);
        assert!(modes.off && modes.anc && !modes.ambient);

        let both = HoldModes {
            off: true,
            anc: true,
            ambient: false,
        };
        // Each enabled mode is set for the right earbud and mirrored to the left.
        assert_eq!(both.encode(), 0b0101_0101);
        assert_eq!(HoldModes::decode(both.encode()), both);
    }

    #[test]
    fn touch_flags_report_the_lock_inverted() {
        let flags = TouchFlags::decode(0xbe);
        assert!(!flags.locked, "bit 7 set means unlocked");
        assert!(flags.triple_tap && flags.double_tap && flags.single_tap);
        assert!(!flags.touch_hold);

        let locked = TouchFlags::decode(0x3e);
        assert!(locked.locked);
        // Encoding keeps every gesture bit and flips the lock bit back.
        assert_eq!(TouchFlags::decode(flags.encode()), flags);
    }
}
