//! Virgil patch: device settings Dante Controller can change (Device Config
//! tab): device latency, sample rate, reboot and identify.
//!
//! Inferno answered Dante Controller's device-settings queries with zeros,
//! so it showed "0.0 msec" and "This device does not support ... configuration".
//! The layouts here follow captures of Dante hardware (kept as comments in
//! arc_server.rs and info_mcast_server.rs) and netaudio's encoders
//! (github.com/chris-ritsen/network-audio-controller). Packets are built and
//! parsed by plain functions so they can be tested without a network.
//!
//! A change is not applied here: it goes to the host application through
//! the hook (Virgil saves it in its configuration and restarts the engine).

use std::sync::Mutex;

/// A change Dante Controller asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteSetting {
  /// Device (receive) latency, nanoseconds.
  RxLatencyNs(u32),
  /// Sample rate, Hz (one of SUPPORTED_SAMPLE_RATES).
  SampleRate(u32),
  /// "Reboot" in Device Config.
  Reboot,
  /// "Identify" (the eye icon).
  Identify,
}

pub type RemoteSettingHook = fn(RemoteSetting);
static HOOK: Mutex<Option<RemoteSettingHook>> = Mutex::new(None);

/// Install (or clear) the function that receives changes from Dante Controller.
pub fn set_remote_setting_hook(hook: Option<RemoteSettingHook>) {
  *HOOK.lock().unwrap_or_else(|e| e.into_inner()) = hook;
}

/// Hand a change to the host. False when nobody listens (then the device
/// must not claim it accepted it).
pub(crate) fn notify(setting: RemoteSetting) -> bool {
  let hook = *HOOK.lock().unwrap_or_else(|e| e.into_inner());
  match hook {
    Some(h) => {
      h(setting);
      true
    }
    None => false,
  }
}

/// Receives a device name set in Dante Controller ("Rename Device").
pub type DeviceNameHook = fn(&str);
static NAME_HOOK: Mutex<Option<DeviceNameHook>> = Mutex::new(None);

pub fn set_device_name_hook(hook: Option<DeviceNameHook>) {
  *NAME_HOOK.lock().unwrap_or_else(|e| e.into_inner()) = hook;
}

pub(crate) fn notify_device_name(name: &str) -> bool {
  let hook = *NAME_HOOK.lock().unwrap_or_else(|e| e.into_inner());
  match hook {
    Some(h) => {
      h(name);
      true
    }
    None => false,
  }
}

/// Name from an ARC opcode 0x1001 (set device name): the content after the
/// header is the name, NUL-terminated. Dante names: 1-31 letters, digits and
/// '-', not starting or ending with '-'.
pub fn parse_device_name_set(content: &[u8]) -> Option<String> {
  let end = content.iter().position(|&b| b == 0).unwrap_or(content.len());
  let name = std::str::from_utf8(&content[..end]).ok()?;
  let ok = (1..=31).contains(&name.len())
    && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    && !name.starts_with('-')
    && !name.ends_with('-');
  ok.then(|| name.to_owned())
}

pub(crate) fn hook_installed() -> bool {
  HOOK.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

pub const SUPPORTED_SAMPLE_RATES: [u32; 4] = [44100, 48000, 88200, 96000];
/// Device latency range offered to Dante Controller (Virgil's latency_us limits).
pub const MIN_LATENCY_NS: u32 = 500_000;
pub const MAX_LATENCY_NS: u32 = 200_000_000;
/// What Dante Controller shows as the default latency.
pub const DEFAULT_LATENCY_NS: u32 = 4_000_000;

// ARC (control port) device settings property ids.
pub const PROP_SAMPLE_RATE: u16 = 0x8020;
pub const PROP_DEFAULT_LATENCY_NS: u16 = 0x8204;
pub const PROP_CONFIGURED_LATENCY_NS: u16 = 0x8205;
pub const PROP_UNICAST_FRAMES_PER_PACKET: u16 = 0x0211;
pub const PROP_RX_LATENCY_NS: u16 = 0x8301;
pub const PROP_MAX_LATENCY_NS: u16 = 0x8302;
pub const PROP_RX_FRAMES_PER_PACKET: u16 = 0x0310;
pub const PROP_MIN_LATENCY_NS: u16 = 0x8306;

/// Bytes before the content of an ARC packet: start code, length, sequence,
/// opcode and result code / reserved word. Value pointers in device-settings
/// packets count from the start of the packet.
const ARC_HEADER: usize = 10;

/// Answer to ARC opcode 0x1100 (device settings query): property directory
/// then the u32 values it points to.
pub fn device_settings_response(sample_rate: u32, latency_ns: u32) -> Vec<u8> {
  let settings: [(u16, u32); 6] = [
    (PROP_SAMPLE_RATE, sample_rate),
    (PROP_DEFAULT_LATENCY_NS, DEFAULT_LATENCY_NS),
    (PROP_CONFIGURED_LATENCY_NS, latency_ns),
    (PROP_RX_LATENCY_NS, latency_ns),
    (PROP_MAX_LATENCY_NS, MAX_LATENCY_NS),
    (PROP_MIN_LATENCY_NS, MIN_LATENCY_NS),
  ];
  let n = settings.len();
  let mut body = vec![n as u8, n as u8];
  let first_value = ARC_HEADER + 2 + n * 4;
  for (i, (prop, _)) in settings.iter().enumerate() {
    body.extend_from_slice(&prop.to_be_bytes());
    body.extend_from_slice(&((first_value + i * 4) as u16).to_be_bytes());
  }
  for (_, v) in settings {
    body.extend_from_slice(&v.to_be_bytes());
  }
  body
}

/// Answer to ARC opcode 0x1102 (which properties exist; 3 = writable,
/// 1 = read-only). Captured from Dante hardware with few channels.
pub const PROPERTY_DIRECTORY: [u8; 94] = [
  0x00, 0x17, 0x80, 0x20, 0x00, 0x01, 0x80, 0x21, 0x00, 0x03, 0x00, 0x22, 0x00, 0x03, 0x00, 0x23, 0x00, 0x03,
  0x00, 0x24, 0x00, 0x01, 0x02, 0x01, 0x00, 0x03, 0x82, 0x04, 0x00, 0x03, 0x82, 0x05, 0x00, 0x03, 0x02, 0x0a,
  0x00, 0x01, 0x02, 0x0b, 0x00, 0x01, 0x02, 0x10, 0x00, 0x03, 0x02, 0x11, 0x00, 0x03, 0x02, 0x12, 0x00, 0x03,
  0x02, 0x13, 0x00, 0x01, 0x02, 0x14, 0x00, 0x01, 0x83, 0x01, 0x00, 0x03, 0x83, 0x06, 0x00, 0x01, 0x83, 0x02,
  0x00, 0x01, 0x03, 0x10, 0x00, 0x01, 0x03, 0x11, 0x00, 0x01, 0x03, 0x03, 0x00, 0x03, 0x83, 0xf0, 0x00, 0x01,
  0x06, 0x01, 0x00, 0x01,
];

/// Latency from an ARC opcode 0x1101 (device settings write). `content` is
/// the packet after its 10-byte header: count, a byte, then (property, value
/// or pointer) pairs; latency values are u32 at the pointers. The receive
/// latency is preferred, then the configured (unicast) one.
pub fn parse_latency_set(content: &[u8]) -> Option<u32> {
  let n = *content.first()? as usize;
  let read_u16 = |at: usize| content.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
  let mut found: Option<(u16, u32)> = None;
  for i in 0..n {
    let at = 2 + i * 4;
    let prop = read_u16(at)?;
    if prop != PROP_RX_LATENCY_NS && prop != PROP_CONFIGURED_LATENCY_NS {
      continue;
    }
    let ptr = read_u16(at + 2)? as usize;
    let off = ptr.checked_sub(ARC_HEADER)?;
    let b = content.get(off..off + 4)?;
    let v = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    if found.map_or(true, |(p, _)| p != PROP_RX_LATENCY_NS) {
      found = Some((prop, v));
    }
  }
  found.map(|(_, v)| v)
}

/// Answer to an accepted latency write: the applied values, as Dante devices
/// echo them.
pub fn latency_applied_response(latency_ns: u32) -> Vec<u8> {
  let first_value = (ARC_HEADER + 2 + 4 * 4) as u16;
  let mut body = vec![4u8, 4u8];
  body.extend_from_slice(&PROP_CONFIGURED_LATENCY_NS.to_be_bytes());
  body.extend_from_slice(&first_value.to_be_bytes());
  body.extend_from_slice(&PROP_UNICAST_FRAMES_PER_PACKET.to_be_bytes());
  body.extend_from_slice(&4u16.to_be_bytes());
  body.extend_from_slice(&PROP_RX_LATENCY_NS.to_be_bytes());
  body.extend_from_slice(&(first_value + 4).to_be_bytes());
  body.extend_from_slice(&PROP_RX_FRAMES_PER_PACKET.to_be_bytes());
  body.extend_from_slice(&4u16.to_be_bytes());
  body.extend_from_slice(&latency_ns.to_be_bytes());
  body.extend_from_slice(&latency_ns.to_be_bytes());
  body
}

// Settings (ConMon, UDP 8700) requests: "Audinate" at 16..24, 0x07 at 24,
// a group byte at 25, the message type at 26..28.
const GROUP_SYSTEM: u8 = 0x3a;
const GROUP_IDENTITY: u8 = 0x31;
const GROUP_AUDIO_CONFIG: u8 = 0x27;
pub const TYPE_SAMPLE_RATE: u16 = 0x0081;
pub const TYPE_ENCODING: u16 = 0x0083;
const TYPE_SYSTEM_RESET: u16 = 0x0090;
const TYPE_IDENTIFY: u16 = 0x0063;

/// A settings (port 8700) request this module understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsRequest {
  SetSampleRate(u32),
  SetEncoding(u32),
  Reboot,
  FactoryReset,
  Identify,
}

pub fn parse_settings_request(buf: &[u8]) -> Option<SettingsRequest> {
  if buf.len() < 28 || buf.get(16..24)? != b"Audinate" || buf[24] != 0x07 {
    return None;
  }
  let group = buf[25];
  let kind = u16::from_be_bytes([buf[26], buf[27]]);
  let u32_at = |at: usize| buf.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
  match (group, kind) {
    (GROUP_AUDIO_CONFIG, TYPE_SAMPLE_RATE) if u32_at(32)? == 1 => Some(SettingsRequest::SetSampleRate(u32_at(36)?)),
    (GROUP_AUDIO_CONFIG, TYPE_ENCODING) if u32_at(32)? == 1 => Some(SettingsRequest::SetEncoding(u32_at(36)?)),
    (GROUP_SYSTEM, TYPE_SYSTEM_RESET) => {
      let mode = buf.get(34..36).map(|b| u16::from_be_bytes([b[0], b[1]]))?;
      Some(if mode == 0 { SettingsRequest::Reboot } else { SettingsRequest::FactoryReset })
    }
    (GROUP_IDENTITY, TYPE_IDENTIFY) => Some(SettingsRequest::Identify),
    _ => None,
  }
}

/// Content of the sample rate (opcode ...0080) or encoding (...0082) status:
/// count, current, pending, a constant, the supported values.
pub fn audio_capability_content(current: u32, pending: u32, supported: &[u32]) -> Vec<u8> {
  let mut c = Vec::with_capacity(16 + supported.len() * 4);
  c.extend_from_slice(&0x0018u16.to_be_bytes());
  c.extend_from_slice(&(supported.len() as u16).to_be_bytes());
  c.extend_from_slice(&current.to_be_bytes());
  c.extend_from_slice(&pending.to_be_bytes());
  c.extend_from_slice(&0x0002_0000u32.to_be_bytes());
  for v in supported {
    c.extend_from_slice(&v.to_be_bytes());
  }
  c
}

#[cfg(test)]
mod tests {
  use super::*;

  // A settings packet as netaudio builds it: ffff, length, id, 0, device id,
  // "Audinate", 0x07, group, tail.
  fn settings_packet(group: u8, tail: &[u8]) -> Vec<u8> {
    let mut p = vec![0xff, 0xff, 0, 0, 0x12, 0x34, 0, 0];
    p.extend_from_slice(&[0x52, 0x54, 0, 0, 0, 0, 0, 0]);
    p.extend_from_slice(b"Audinate");
    p.push(0x07);
    p.push(group);
    p.extend_from_slice(tail);
    let len = p.len() as u16;
    p[2..4].copy_from_slice(&len.to_be_bytes());
    p
  }

  #[test]
  fn parses_sample_rate_and_encoding_writes() {
    let mut tail = vec![0x00, 0x81, 0, 0, 0, 100, 0, 0, 0, 1];
    tail.extend_from_slice(&96000u32.to_be_bytes());
    assert_eq!(parse_settings_request(&settings_packet(0x27, &tail)), Some(SettingsRequest::SetSampleRate(96000)));
    let mut tail = vec![0x00, 0x83, 0, 0, 0, 100, 0, 0, 0, 1];
    tail.extend_from_slice(&24u32.to_be_bytes());
    assert_eq!(parse_settings_request(&settings_packet(0x27, &tail)), Some(SettingsRequest::SetEncoding(24)));
  }

  #[test]
  fn probes_are_not_writes() {
    // Sample rate probe: group 0x3a, mode 0.
    let tail = [0x00, 0x81, 0, 0, 0, 100, 0, 0, 0, 0, 0, 0, 0, 0];
    assert_eq!(parse_settings_request(&settings_packet(0x3a, &tail)), None);
  }

  #[test]
  fn parses_reboot_factory_reset_identify() {
    let reboot = [0x00, 0x90, 0, 0, 0, 100, 0, 1, 0, 0];
    assert_eq!(parse_settings_request(&settings_packet(0x3a, &reboot)), Some(SettingsRequest::Reboot));
    let factory = [0x00, 0x90, 0, 0, 0, 100, 0, 1, 0, 1];
    assert_eq!(parse_settings_request(&settings_packet(0x3a, &factory)), Some(SettingsRequest::FactoryReset));
    let identify = [0x00, 0x63, 0x00, 0x00, 0x00, 0x64];
    assert_eq!(parse_settings_request(&settings_packet(0x31, &identify)), Some(SettingsRequest::Identify));
  }

  #[test]
  fn rejects_short_or_foreign_packets() {
    assert_eq!(parse_settings_request(&[0u8; 10]), None);
    let mut p = settings_packet(0x3a, &[0x00, 0x90, 0, 0, 0, 100, 0, 1, 0, 0]);
    p[16] = b'X';
    assert_eq!(parse_settings_request(&p), None);
    // Truncated before the mode.
    let p = settings_packet(0x3a, &[0x00, 0x90, 0, 0]);
    assert_eq!(parse_settings_request(&p), None);
  }

  #[test]
  fn parses_dante_controller_latency_write() {
    // netaudio's build_set_latency(1.0 ms): 0x0000 at offset 8, then this.
    let preamble = [
      0x05, 0x04, 0x82, 0x05, 0x00, 0x20, 0x02, 0x11, 0x00, 0x04, 0x83, 0x01, 0x00, 0x24, 0x03, 0x10, 0x00, 0x04,
      0x83, 0x02, 0x83, 0x06,
    ];
    let mut content = preamble.to_vec();
    content.extend_from_slice(&1_000_000u32.to_be_bytes());
    content.extend_from_slice(&1_000_000u32.to_be_bytes());
    assert_eq!(parse_latency_set(&content), Some(1_000_000));
  }

  #[test]
  fn latency_write_prefers_receive_latency_and_survives_garbage() {
    // 8205 -> 2 ms, 8301 -> 5 ms.
    let mut c = vec![2u8, 4, 0x82, 0x05, 0x00, 0x14, 0x83, 0x01, 0x00, 0x18];
    c.extend_from_slice(&2_000_000u32.to_be_bytes());
    c.extend_from_slice(&5_000_000u32.to_be_bytes());
    assert_eq!(parse_latency_set(&c), Some(5_000_000));
    assert_eq!(parse_latency_set(&[]), None);
    assert_eq!(parse_latency_set(&[9, 4, 0x83, 0x01, 0xff, 0xff]), None);
    assert_eq!(parse_latency_set(&[1, 4, 0x83, 0x01, 0x00, 0x02]), None); // pointer into the header
  }

  #[test]
  fn device_settings_response_points_at_its_values() {
    let body = device_settings_response(48000, 1_000_000);
    assert_eq!(body[0], 6);
    // The packet the device sends: 10-byte header + body.
    let mut pkt = vec![0u8; 10];
    pkt.extend_from_slice(&body);
    let value_of = |prop: u16| {
      (0..6).find_map(|i| {
        let at = 12 + i * 4;
        (u16::from_be_bytes([pkt[at], pkt[at + 1]]) == prop).then(|| {
          let p = u16::from_be_bytes([pkt[at + 2], pkt[at + 3]]) as usize;
          u32::from_be_bytes([pkt[p], pkt[p + 1], pkt[p + 2], pkt[p + 3]])
        })
      })
    };
    assert_eq!(value_of(PROP_SAMPLE_RATE), Some(48000));
    assert_eq!(value_of(PROP_CONFIGURED_LATENCY_NS), Some(1_000_000));
    assert_eq!(value_of(PROP_RX_LATENCY_NS), Some(1_000_000));
    assert_eq!(value_of(PROP_MIN_LATENCY_NS), Some(MIN_LATENCY_NS));
    assert_eq!(value_of(PROP_MAX_LATENCY_NS), Some(MAX_LATENCY_NS));
  }

  #[test]
  fn latency_applied_round_trips_through_the_parser() {
    assert_eq!(parse_latency_set(&latency_applied_response(3_000_000)), Some(3_000_000));
  }

  #[test]
  fn property_directory_is_consistent() {
    let n = u16::from_be_bytes([PROPERTY_DIRECTORY[0], PROPERTY_DIRECTORY[1]]) as usize;
    assert_eq!(PROPERTY_DIRECTORY.len(), 2 + n * 4);
    let writable = |prop: u16| {
      (0..n).any(|i| {
        let at = 2 + i * 4;
        u16::from_be_bytes([PROPERTY_DIRECTORY[at], PROPERTY_DIRECTORY[at + 1]]) == prop
          && PROPERTY_DIRECTORY[at + 3] == 3
      })
    };
    assert!(writable(PROP_CONFIGURED_LATENCY_NS) && writable(PROP_RX_LATENCY_NS));
  }

  #[test]
  fn audio_capability_layout() {
    let c = audio_capability_content(48000, 96000, &SUPPORTED_SAMPLE_RATES);
    assert_eq!(&c[0..4], &[0x00, 0x18, 0x00, 0x04]);
    assert_eq!(&c[4..8], &48000u32.to_be_bytes());
    assert_eq!(&c[8..12], &96000u32.to_be_bytes());
    assert_eq!(&c[16..20], &44100u32.to_be_bytes());
    assert_eq!(c.len(), 16 + 16);
  }

  #[test]
  fn parses_device_name() {
    assert_eq!(parse_device_name_set(b"Studio-A\0"), Some("Studio-A".to_owned()));
    assert_eq!(parse_device_name_set(b"NoNul"), Some("NoNul".to_owned()));
    assert_eq!(parse_device_name_set(b"\0"), None);
    assert_eq!(parse_device_name_set(b"-bad\0"), None);
    assert_eq!(parse_device_name_set(b"has space\0"), None);
    assert_eq!(parse_device_name_set(&[b'a'; 32]), None);
    assert_eq!(parse_device_name_set(&[0xff, 0xfe, 0]), None);
  }

  #[test]
  fn hook_receives_settings() {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEEN: AtomicU32 = AtomicU32::new(0);
    fn h(s: RemoteSetting) {
      if let RemoteSetting::SampleRate(r) = s {
        SEEN.store(r, Ordering::SeqCst);
      }
    }
    set_remote_setting_hook(Some(h));
    assert!(hook_installed());
    assert!(notify(RemoteSetting::SampleRate(88200)));
    assert_eq!(SEEN.load(Ordering::SeqCst), 88200);
    set_remote_setting_hook(None);
    assert!(!notify(RemoteSetting::Reboot));
  }
}
