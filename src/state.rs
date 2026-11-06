//! The full state of the buds, as reported by `EXTENDED_STATUS_UPDATED`.
//!
//! Field offsets follow GalaxyBudsClient's `ExtendedStatusUpdateDecoder` for
//! `Models.Buds2`; later fields are gated on the revision byte the buds report.

use serde::{Deserialize, Serialize};

use crate::proto::{ids, Equalizer, HoldModes, NoiseMode, TouchAction, TouchFlags};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Placement {
    InCase,
    Ear,
    Outside,
    Charging,
    Unknown,
}

impl Placement {
    fn decode(nibble: u8) -> Self {
        match nibble {
            0 => Placement::InCase,
            1 => Placement::Ear,
            2 => Placement::Outside,
            _ => Placement::Unknown,
        }
    }
}

/// Everything the TUI, the CLI and the bar widget can show or change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct BudsState {
    pub connected: bool,
    pub address: String,
    pub model: String,
    /// Protocol revision of the buds' firmware; gates several fields below.
    pub revision: u8,
    pub battery_left: u8,
    pub battery_right: u8,
    pub battery_case: u8,
    pub charging_left: bool,
    pub charging_right: bool,
    pub charging_case: bool,
    pub placement_left: Placement,
    pub placement_right: Placement,
    pub wearing_left: bool,
    pub wearing_right: bool,
    pub coupled: bool,
    pub noise_mode: NoiseMode,
    pub ambient_volume: u8,
    pub ambient_custom_on: bool,
    pub ambient_custom_left: u8,
    pub ambient_custom_right: u8,
    pub ambient_custom_tone: u8,
    pub extra_high_ambient: bool,
    pub equalizer: Equalizer,
    pub touch_left: Option<TouchAction>,
    pub touch_right: Option<TouchAction>,
    pub touch_flags: TouchFlags,
    pub hold_modes: HoldModes,
    pub sidetone: bool,
    pub call_path_on_buds: bool,
    pub spatial_audio: bool,
    pub game_mode: bool,
    pub seamless_connection: bool,
    pub noise_controls_one_earbud: bool,
    pub voice_wake_up: bool,
    pub firmware: Option<String>,
    /// Unix seconds of the last successful decode.
    pub updated_at: u64,
}

impl Default for BudsState {
    fn default() -> Self {
        BudsState {
            connected: false,
            address: String::new(),
            model: "Buds2".into(),
            revision: 0,
            battery_left: 0,
            battery_right: 0,
            battery_case: 0,
            charging_left: false,
            charging_right: false,
            charging_case: false,
            placement_left: Placement::Unknown,
            placement_right: Placement::Unknown,
            wearing_left: false,
            wearing_right: false,
            coupled: false,
            noise_mode: NoiseMode::Off,
            ambient_volume: 0,
            ambient_custom_on: false,
            ambient_custom_left: 0,
            ambient_custom_right: 0,
            ambient_custom_tone: 0,
            extra_high_ambient: false,
            equalizer: Equalizer::Normal,
            touch_left: None,
            touch_right: None,
            touch_flags: TouchFlags::default(),
            hold_modes: HoldModes::default(),
            sidetone: false,
            call_path_on_buds: false,
            spatial_audio: false,
            game_mode: false,
            seamless_connection: false,
            noise_controls_one_earbud: false,
            voice_wake_up: false,
            firmware: None,
            updated_at: 0,
        }
    }
}

const SW_MONTH: [&str; 12] = ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L"];
const SW_YEAR: [&str; 12] = ["O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z"];
const SW_REL: [&str; 36] = [
    "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "A", "B", "C", "D", "E", "F", "G", "H", "I",
    "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z",
];
/// Build prefix of the Buds2 firmware (SM-R177).
const BUILD_PREFIX: &str = "177";

/// Decode the three-byte packed software version at `at` into the form the
/// phone app shows, e.g. `R177XXU0AYF1`.
fn software_version(p: &[u8], at: usize) -> String {
    let variant = if p[at] == 0 { "E" } else { "U" };
    let year = SW_YEAR
        .get(((p[at + 1] & 0xf0) >> 4) as usize)
        .copied()
        .unwrap_or("?");
    let month = SW_MONTH
        .get((p[at + 1] & 0x0f) as usize)
        .copied()
        .unwrap_or("?");
    let release = SW_REL.get(p[at + 2] as usize).copied().unwrap_or("?");
    format!("R{BUILD_PREFIX}XX{variant}0A{year}{month}{release}")
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl BudsState {
    /// Apply one decoded frame. Returns true when the state changed.
    pub fn apply(&mut self, id: u8, payload: &[u8]) -> bool {
        let before = self.clone();

        match id {
            ids::EXTENDED_STATUS_UPDATED => self.apply_extended(payload),
            ids::STATUS_UPDATED => self.apply_status(payload),
            ids::NOISE_CONTROLS_UPDATE => {
                if let Some(&b) = payload.first() {
                    self.noise_mode = NoiseMode::decode(b);
                }
            }
            ids::AMBIENT_VOLUME => {
                if let Some(&b) = payload.first() {
                    self.ambient_volume = b;
                }
            }
            ids::TOUCH_UPDATED => {
                if let Some(&b) = payload.first() {
                    self.touch_flags = TouchFlags::decode(b);
                }
            }
            ids::VERSION_INFO => self.apply_version(payload),
            _ => {}
        }

        if *self != before {
            self.updated_at = now();
            true
        } else {
            false
        }
    }

    /// `VERSION_INFO`: hardware revisions, then a packed software version per
    /// side (`DebugModeVersionDecoder` in GalaxyBudsClient). We keep the left
    /// earbud's version, which is what the phone app shows.
    /// An earbud sitting in the case reports zeros, and so does the reply to an
    /// explicit request, so take whichever side actually carries a version.
    fn apply_version(&mut self, p: &[u8]) {
        if p.len() < 8 {
            return;
        }
        for at in [2, 5] {
            if p[at..at + 3].iter().any(|&b| b != 0) {
                self.firmware = Some(software_version(p, at));
                return;
            }
        }
    }

    fn apply_status(&mut self, p: &[u8]) {
        if p.len() < 7 {
            return;
        }
        // p[0] is this message's own revision counter, not the protocol
        // revision that gates features: a Buds2 reporting 11 in its extended
        // status sends 1 here. Overwriting would make firmware-gated controls
        // appear and disappear, so only the extended status sets `revision`.
        self.battery_left = p[1];
        self.battery_right = p[2];
        self.coupled = p[3] != 0;
        self.placement_left = Placement::decode((p[5] & 0xf0) >> 4);
        self.placement_right = Placement::decode(p[5] & 0x0f);
        self.wearing_left = self.placement_left == Placement::Ear;
        self.wearing_right = self.placement_right == Placement::Ear;
        self.battery_case = p[6];
        if p.len() > 7 {
            self.apply_charging(p[7]);
        }
    }

    fn apply_charging(&mut self, b: u8) {
        self.charging_left = b & (1 << 4) != 0;
        self.charging_right = b & (1 << 2) != 0;
        self.charging_case = b & (1 << 0) != 0;
        if self.charging_left {
            self.placement_left = Placement::Charging;
        }
        if self.charging_right {
            self.placement_right = Placement::Charging;
        }
    }

    fn apply_extended(&mut self, p: &[u8]) {
        if p.len() < 22 {
            return;
        }
        let rev = p[0];
        self.revision = rev;
        self.battery_left = p[2];
        self.battery_right = p[3];
        self.coupled = p[4] != 0;
        self.placement_left = Placement::decode((p[6] & 0xf0) >> 4);
        self.placement_right = Placement::decode(p[6] & 0x0f);
        self.wearing_left = self.placement_left == Placement::Ear;
        self.wearing_right = self.placement_right == Placement::Ear;
        self.battery_case = p[7];
        self.equalizer = Equalizer::decode(p[9]);
        self.touch_flags = TouchFlags::decode(p[10]);
        self.touch_left = TouchAction::decode((p[11] & 0xf0) >> 4);
        self.touch_right = TouchAction::decode(p[11] & 0x0f);
        self.noise_mode = NoiseMode::decode(p[12]);
        self.voice_wake_up = p[13] == 1;
        self.seamless_connection = p[19] == 0;
        self.hold_modes = HoldModes::decode(p[21]);

        if p.len() > 23 {
            self.ambient_volume = p[23];
        }
        if p.len() > 26 {
            self.extra_high_ambient = p[26] == 1;
        }
        if rev >= 3 && p.len() > 28 {
            self.noise_controls_one_earbud = p[28] == 1;
        }
        if rev >= 5 && p.len() > 31 {
            self.ambient_custom_on = p[29] == 1;
            self.ambient_custom_left = (p[30] & 0xf0) >> 4;
            self.ambient_custom_right = p[30] & 0x0f;
            self.ambient_custom_tone = p[31];
        }
        if rev >= 6 && p.len() > 33 {
            self.sidetone = p[33] == 1;
        }
        if rev >= 7 && p.len() > 34 {
            self.call_path_on_buds = p[34] == 0;
        }
        if rev >= 8 && p.len() > 35 {
            self.spatial_audio = p[35] == 1;
        }
        if rev >= 10 && p.len() > 36 {
            self.apply_charging(p[36]);
        }
    }

    /// Lower of the two earbud batteries — what the bar widget shows.
    pub fn battery_min(&self) -> u8 {
        self.battery_left.min(self.battery_right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `EXTENDED_STATUS_UPDATED` payload from a Buds2 on firmware
    /// revision 11, with the left earbud in the case and the right one worn.
    const EXTENDED: [u8; 39] = [
        0x0b, 0x03, 0x00, 0x51, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xbe, 0x22, 0x00, 0x00, 0x00,
        0x00, 0x3a, 0x01, 0x00, 0x00, 0x03, 0x35, 0x00, 0x02, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00,
        0x11, 0x02, 0x01, 0x00, 0x00, 0x01, 0x80, 0x00, 0x00,
    ];

    fn decoded() -> BudsState {
        let mut state = BudsState::default();
        assert!(state.apply(ids::EXTENDED_STATUS_UPDATED, &EXTENDED));
        state
    }

    #[test]
    fn extended_status_decodes_every_field_we_expose() {
        let s = decoded();

        assert_eq!(s.revision, 11);
        assert_eq!(s.battery_left, 0);
        assert_eq!(s.battery_right, 81);
        assert_eq!(s.placement_left, Placement::InCase);
        assert_eq!(s.placement_right, Placement::Ear);
        assert!(!s.wearing_left && s.wearing_right);

        assert_eq!(s.noise_mode, NoiseMode::Off);
        assert_eq!(s.ambient_volume, 2);
        assert_eq!(s.equalizer, Equalizer::Normal);
        assert_eq!(s.touch_left, Some(TouchAction::NoiseControl));
        assert_eq!(s.touch_right, Some(TouchAction::NoiseControl));
        assert_eq!(
            s.hold_modes,
            HoldModes {
                off: true,
                anc: true,
                ambient: false
            }
        );
        assert!(!s.touch_flags.locked);
        assert!(s.seamless_connection);
        assert!(!s.sidetone);
        assert!(s.call_path_on_buds);
        assert!(s.spatial_audio);
        assert!(!s.charging_left && !s.charging_right && !s.charging_case);
    }

    #[test]
    fn status_update_patches_battery_and_placement() {
        let mut s = decoded();
        // its own revision counter, batteries, coupled, main, placement, case, charging
        let update = [0x01, 0x50, 0x4f, 0x01, 0x00, 0x11, 0x55, 0x00];
        assert!(s.apply(ids::STATUS_UPDATED, &update));

        // The protocol revision stays where the extended status put it.
        assert_eq!(s.revision, 11);
        assert_eq!(s.battery_left, 0x50);
        assert_eq!(s.battery_right, 0x4f);
        assert_eq!(s.battery_case, 0x55);
        assert_eq!(s.placement_left, Placement::Ear);
        assert_eq!(s.placement_right, Placement::Ear);
        // Settings from the extended update survive a status update.
        assert_eq!(s.ambient_volume, 2);
    }

    #[test]
    fn repeated_identical_frames_report_no_change() {
        let mut s = decoded();
        assert!(!s.apply(ids::EXTENDED_STATUS_UPDATED, &EXTENDED));
    }

    #[test]
    fn noise_control_push_updates_the_mode() {
        let mut s = decoded();
        assert!(s.apply(ids::NOISE_CONTROLS_UPDATE, &[1]));
        assert_eq!(s.noise_mode, NoiseMode::Anc);
    }

    #[test]
    fn version_info_formats_like_the_phone_app() {
        let mut s = BudsState::default();
        // hw L, hw R, then three packed bytes per side.
        assert!(s.apply(ids::VERSION_INFO, &[0, 0, 0x01, 0xa5, 0x01, 0x01, 0xa5, 0x01, 0, 0]));
        assert_eq!(s.firmware.as_deref(), Some("R177XXU0AYF1"));
    }

    #[test]
    fn version_info_falls_back_to_the_bud_that_is_awake() {
        let mut s = BudsState::default();
        // Left earbud in the case: its three version bytes read as zeros.
        assert!(s.apply(ids::VERSION_INFO, &[0, 0, 0, 0, 0, 0x01, 0xa5, 0x01, 0, 0]));
        assert_eq!(s.firmware.as_deref(), Some("R177XXU0AYF1"));
    }

    #[test]
    fn battery_min_ignores_nothing_and_reports_the_weaker_bud() {
        let mut s = BudsState::default();
        s.battery_left = 70;
        s.battery_right = 55;
        assert_eq!(s.battery_min(), 55);
    }
}
