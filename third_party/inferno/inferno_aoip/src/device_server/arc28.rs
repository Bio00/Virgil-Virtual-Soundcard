//! Virgil patch: ARC 2.8, the control protocol of current Dante devices
//! (Dante Virtual Soundcard 4.5: 2.8.15). A controller picks the generation
//! from `arcp_vers` on mDNS and then uses start codes 0x28xx.
//!
//! Layouts: answers of a Dante Virtual Soundcard 4.5.2 and the notes and
//! tests of dstack (github.com/zbynekdrlik/dstack, MIT OR Apache-2.0) and
//! netaudio (github.com/chris-ritsen/network-audio-controller, Unlicense).
//!
//! The flag for segmented flows (0x1000 of the 0x1000 answer's first word)
//! is set: Dante Controller then uses the 2.8 lists and flow commands.

use crate::byte_utils::{align_wpos, make_u16, read_0term_str_from_buffer, write_0term_str_to_bytebuffer};
use crate::device_info::DeviceInfo;
use crate::protocol::proto_arc::CommonChannelsDescriptor;
use crate::protocol::req_resp::HEADER_LENGTH;
use binary_serde::recursive_array::RecursiveArray as _;
use binary_serde::BinarySerde;
use bytebuffer::ByteBuffer;

// Versions announced, the ones of Dante Virtual Soundcard 4.5.2.
pub const ARCP_VERSION: &str = "2.8.15";
const ARCP_START_CODE: u16 = 0x280f;
pub const ROUTER_VERSION: &str = "4.4.0";
const ROUTER_VERSION_CODE: u16 = 0x0404;
pub const SERVER_VERSION: &str = "4.2.0";
/// Dante software version in Dante Controller's Device Info: major, minor, patch.
pub const DANTE_SOFTWARE_VERSION: [u8; 4] = [4, 5, 0, 2];

/// Start code some 2.8 answers carry whatever the request's was.
pub const START_CODE_2801: u16 = 0x2801;
pub const STATUS_MORE: u16 = 0x8112;

pub const CAPABILITIES: u16 = 0x1503;
pub const TX_CHANNELS: u16 = 0x2400;
pub const TX_NAMES_SET: u16 = 0x2438;
pub const RX_CHANNELS: u16 = 0x3400;
pub const RX_NAMES_SET: u16 = 0x3401;
pub const RX_SUBSCRIBE: u16 = 0x3410;
pub const TX_FLOWS: u16 = 0x2600;
pub const TX_FLOW_CREATE: u16 = 0x2601;
pub const TX_FLOW_DELETE: u16 = 0x2602;
pub const RX_FLOWS: u16 = 0x3600;
const FLOW_MULTICAST: u32 = 2;
const FLOW_UNICAST: u32 = 0x11;

const MEDIA_AUDIO: u16 = 3;
/// Selector of the first page; later pages may be asked by media type.
const AUDIO_SELECTOR: u16 = 1;
const NAME_FIELD: usize = 32;
/// Records of a page: an RX record with its strings has at most 248 bytes
/// (56 + three of at most 64), so a page also ends at SOFT_LIMIT.
const PAGE_SLOTS: usize = 16;
const SOFT_LIMIT: usize = 800;

pub fn is_arc28(start_code: u16) -> bool {
  start_code >> 8 == 0x28
}

/// What is added to the 2.7 answer to 0x1000; `body_len` is its length.
pub fn media_record(body_len: usize, tx: u16, rx: u16, tx_flows: u16, rx_flows: u16, per_flow: u16) -> Vec<u8> {
  let mut out = Vec::new();
  let mut word = |v: u16| out.extend_from_slice(&v.to_be_bytes());
  word((HEADER_LENGTH + body_len + 4) as u16);
  word(0);
  word(0x1414);
  word(MEDIA_AUDIO);
  let tx_flows = if tx == 0 { 0 } else { tx_flows };
  let rx_flows = if rx == 0 { 0 } else { rx_flows };
  for (t, r) in [(1, 1), (tx, rx), (tx, rx), (tx_flows, rx_flows), (tx_flows, rx_flows), (tx, rx), (per_flow, per_flow)] {
    word(t);
    word(r);
  }
  out.extend_from_slice(&[0; 8]);
  out
}

/// Answer to 0x1503 (Dante Controller's QueryCapabilities, asked of 2.8.15
/// devices): one record, audio, without optional capabilities, as Dante
/// Virtual Soundcard answers.
pub fn capabilities() -> [u8; 30] {
  let mut out = [0u8; 30];
  out[6..14].copy_from_slice(&[1, 1, 0, 20, 0x0a, 0x0a, 0, MEDIA_AUDIO as u8]);
  out
}

/// Answer to 0x1003: names and protocol versions.
pub fn device_names(friendly: &str, factory: &str, board: &str, revision: &str) -> Vec<u8> {
  const NAMES_AT: u16 = 28;
  const VERSIONS_AT: u16 = 40;
  const NAME_AT: u16 = 60;
  const FACTORY_AT: u16 = NAME_AT + NAME_FIELD as u16;
  const BOARD_AT: u16 = FACTORY_AT + NAME_FIELD as u16;
  let revision_at = BOARD_AT + board.len() as u16 + 1;
  let head: [u16; 25] = [
    NAMES_AT, NAMES_AT, VERSIONS_AT, BOARD_AT, revision_at, 0, 0, 0, 0,
    0x0500, NAME_AT, FACTORY_AT, NAME_AT, 0, 0,
    0x0a0a, 0, ROUTER_VERSION_CODE, 0, ARCP_START_CODE, 0x0204, 0x1200, 0x1004, 0, 0,
  ];
  let mut out: Vec<u8> = head.iter().flat_map(|w| w.to_be_bytes()).collect();
  for name in [friendly, factory] {
    let bytes = &name.as_bytes()[..name.len().min(NAME_FIELD - 1)];
    out.extend_from_slice(bytes);
    out.resize(out.len() + NAME_FIELD - bytes.len(), 0);
  }
  for s in [board, revision] {
    out.extend_from_slice(s.as_bytes());
    out.push(0);
  }
  out
}

/// Subscription of a receive channel as listed.
pub struct ListedSubscription {
  pub tx_channel: String,
  pub tx_host: String,
  pub status: u32,
}

/// Answer to 0x2400 (TX, `subscription` is None) or 0x3400 (RX): result code
/// and content.
pub fn channels_page(
  self_info: &DeviceInfo,
  request: &[u8],
  subscription: Option<&dyn Fn(usize) -> Option<ListedSubscription>>,
) -> (u16, Vec<u8>) {
  let channels = if subscription.is_some() { &self_info.rx_channels } else { &self_info.tx_channels };
  let word = |at: usize| request.get(at..at + 2).map(|b| make_u16(b[0], b[1]));
  let (media, first) = match (word(8), word(10)) {
    (Some(m), Some(f)) if f > 0 => (m, f as usize - 1),
    _ => return (0x22, vec![]),
  };
  if media != AUDIO_SELECTOR && media != MEDIA_AUDIO {
    return (1, vec![0; 8]);
  }
  let end = match word(12).unwrap_or(0) {
    0 => channels.len(),
    last => channels.len().min(last as usize),
  };
  let slots = channels.len().min(PAGE_SLOTS);
  // positions in `out` are packet offsets
  let mut out = ByteBuffer::new();
  out.write_bytes(&vec![0u8; HEADER_LENGTH + 6]);
  out.write_u8(slots as u8);
  out.write_u8(0);
  let table = out.get_wpos();
  out.write_bytes(&vec![0u8; slots * 2]);
  align_wpos(&mut out, 4);
  let format = out.get_wpos() as u16;
  if first < end {
    let descr = CommonChannelsDescriptor::new(self_info);
    out.write_bytes(descr.binary_serialize_to_array(binary_serde::Endianness::Big).as_slice());
  }
  let mut count = 0;
  let mut more = false;
  for index in first..end {
    if count >= slots || out.get_wpos() >= SOFT_LIMIT {
      more = true;
      break;
    }
    let ch = &channels[index];
    let sub = subscription.and_then(|f| f(index));
    let (tx_channel, tx_host, status) = match &sub {
      Some(s) => (
        write_0term_str_to_bytebuffer(&mut out, &s.tx_channel),
        write_0term_str_to_bytebuffer(&mut out, &s.tx_host),
        s.status,
      ),
      None => (0, 0, 0),
    };
    let name = write_0term_str_to_bytebuffer(&mut out, &ch.friendly_name.read().unwrap_or_else(|e| e.into_inner()));
    let default_name = write_0term_str_to_bytebuffer(&mut out, &ch.factory_name);
    align_wpos(&mut out, 4);
    let record = out.get_wpos();
    let id = (index + 1) as u16;
    out.write_u16(if subscription.is_some() { 0x141c } else { 0x1414 });
    for w in [id, 0, MEDIA_AUDIO, id, 0] {
      out.write_u16(w);
    }
    out.write_u32(if subscription.is_some() { 6 } else { 7 });
    out.write_u32(0);
    out.write_u16(name);
    out.write_u16(format);
    out.write_bytes(&[0; 6]);
    out.write_u16(default_name);
    out.write_bytes(&[0; 8]);
    if subscription.is_some() {
      out.write_u32(0x0608_0000);
      out.write_u16(tx_channel);
      out.write_u16(tx_host);
      out.write_u32(status.rotate_left(16)); // 2.7's word, halves swapped
      out.write_u32(0x0202_0000);
    }
    let end_pos = out.get_wpos();
    out.set_wpos(table + count * 2);
    out.write_u16(record as u16);
    out.set_wpos(end_pos);
    count += 1;
  }
  let mut content = out.as_bytes()[HEADER_LENGTH..].to_vec();
  content[7] = count as u8;
  (if more { STATUS_MORE } else { 1 }, content)
}

/// Records of a 2.8 write (0x2438, 0x3401: 6 bytes, 0x3410: 8 bytes) as
/// words; None when malformed.
fn write_records(content: &[u8], size: u8) -> Option<Vec<Vec<u16>>> {
  const PREFIX: usize = 10;
  if content.len() < PREFIX || content[6] != size || content[7] != 0 {
    return None;
  }
  let table = content[PREFIX..].get(..content[9] as usize * size as usize)?;
  Some(
    table
      .chunks_exact(size.into())
      .map(|r| r.chunks_exact(2).map(|b| make_u16(b[0], b[1])).collect())
      .collect(),
  )
}

fn string_at(content: &[u8], offset: u16) -> Option<String> {
  let at = (offset as usize).checked_sub(HEADER_LENGTH)?;
  if at == content.len() {
    return Some(String::new()); // netaudio's reset to the default name
  }
  read_0term_str_from_buffer(content, at).ok().map(|s| s.to_owned())
}

/// Renames of 0x2438 / 0x3401: channel ids and names, empty = the default.
pub fn parse_renames(content: &[u8]) -> Option<Vec<(u16, String)>> {
  write_records(content, 6)?
    .into_iter()
    .map(|r| match (r[1], r[2]) {
      (MEDIA_AUDIO, 0) => Some((r[0], String::new())),
      (MEDIA_AUDIO, at) => Some((r[0], string_at(content, at)?)),
      _ => None,
    })
    .collect()
}

/// Subscriptions of 0x3410: channel ids and (TX channel, TX device), None =
/// unsubscribe. Other media are passed over.
pub fn parse_subscriptions(content: &[u8]) -> Option<Vec<(u16, Option<(String, String)>)>> {
  let mut res = vec![];
  for r in write_records(content, 8)? {
    if r[1] != MEDIA_AUDIO {
      continue;
    }
    let names = if r[2] == 0 || r[3] == 0 {
      None
    } else {
      // Dante names are short (an mDNS label is at most 63 bytes)
      let names = (string_at(content, r[2])?, string_at(content, r[3])?);
      if [&names.0, &names.1].iter().any(|n| n.is_empty() || n.len() > 63) {
        return None;
      }
      Some(names)
    };
    res.push((r[0], names));
  }
  Some(res)
}

/// Answer to 0x2438: the channels and the names they now have.
pub fn renames_echo(names: &[(u16, String)]) -> Vec<u8> {
  let mut out = vec![0, 0, 0, 0, 0, 0, 6, 0, names.len() as u8, names.len() as u8];
  let mut at = HEADER_LENGTH + out.len() + names.len() * 6;
  for (id, name) in names {
    for w in [*id, MEDIA_AUDIO, at as u16] {
      out.extend_from_slice(&w.to_be_bytes());
    }
    at += name.len() + 1;
  }
  for (_, name) in names {
    out.extend_from_slice(name.as_bytes());
    out.push(0);
  }
  out
}

/// A transmit flow as listed.
pub struct ListedTxFlow {
  pub id: u16,
  pub name: String,
  pub rx_host: Option<String>,
  pub rx_flow: Option<String>,
  pub destination: Option<(std::net::Ipv4Addr, u16)>,
  /// Channel ids (0: none) of the flow's slots.
  pub channels: Vec<u16>,
  pub fpp: u16,
}

/// A receive flow as listed.
pub struct ListedRxFlow {
  pub id: u16,
  pub port: u16,
  pub latency_ns: u32,
  /// Receive channel indices each slot of the flow feeds.
  pub slots: Vec<Vec<usize>>,
}

/// One page of records; `write` writes one at an aligned position. Flow ids
/// start at 1, `first` is the first one wanted.
fn flow_page<T>(
  self_info: &DeviceInfo,
  slots: usize,
  flows: impl Iterator<Item = T>,
  write: impl Fn(&mut ByteBuffer, u16, &T) -> u16,
) -> (u16, Vec<u8>) {
  let mut out = ByteBuffer::new();
  out.write_bytes(&vec![0u8; HEADER_LENGTH + 6]);
  out.write_u8(slots as u8);
  out.write_u8(0);
  let table = out.get_wpos();
  out.write_bytes(&vec![0u8; slots * 2]);
  align_wpos(&mut out, 4);
  let format = out.get_wpos() as u16;
  out.write_u32(self_info.sample_rate);
  out.write_u32(self_info.bits_per_sample.into());
  let (mut count, mut more) = (0, false);
  for flow in flows {
    if count >= slots || out.get_wpos() >= SOFT_LIMIT {
      more = true;
      break;
    }
    let record = write(&mut out, format, &flow);
    let end = out.get_wpos();
    out.set_wpos(table + 2 * count);
    out.write_u16(record);
    out.set_wpos(end);
    count += 1;
  }
  let mut content = out.as_bytes()[HEADER_LENGTH..].to_vec();
  content[7] = count as u8;
  (if more { STATUS_MORE } else { 1 }, content)
}

/// First flow id a page request asks for.
fn first_flow(request: &[u8]) -> Option<u16> {
  let word = |at: usize| request.get(at..at + 2).map(|b| make_u16(b[0], b[1]));
  match (word(8), word(10)) {
    (Some(AUDIO_SELECTOR | MEDIA_AUDIO), Some(f)) if f > 0 => Some(f),
    _ => None,
  }
}

fn flow_head(out: &mut ByteBuffer, words: usize, id: u16, flags: u32, name: u16, format: u16, latency_ns: u32) {
  out.write_u8(0x14);
  out.write_u8(words as u8);
  for w in [id, 0, MEDIA_AUDIO, id, 0] {
    out.write_u16(w);
  }
  out.write_u32(flags);
  out.write_u32(0);
  out.write_u16(name);
  out.write_u16(format);
  out.write_u32(latency_ns);
  out.write_bytes(&[0; 12]);
}

fn write_tx_flow(out: &mut ByteBuffer, format: u16, f: &ListedTxFlow) -> u16 {
  let mut text = |s: Option<&str>| s.map_or(0, |s| write_0term_str_to_bytebuffer(out, s));
  let name = text(Some(&f.name));
  let (host, rx_flow) = (text(f.rx_host.as_deref()), text(f.rx_flow.as_deref()));
  align_wpos(out, 4);
  let destination = match f.destination {
    Some((ip, port)) => {
      let at = out.get_wpos() as u16;
      out.write_u16(0x0802);
      out.write_u16(port);
      out.write_bytes(&ip.octets());
      at
    }
    None => 0,
  };
  let at = out.get_wpos();
  let channels = &f.channels[..f.channels.len().min(64)];
  let chan_words = 3 + channels.len();
  // Segments: their words, then the words left in the record
  let mut left = 20 + 8 + 4 + chan_words + 2;
  let flags = if f.rx_host.is_none() && f.rx_flow.is_none() { FLOW_MULTICAST } else { FLOW_UNICAST };
  flow_head(out, left, f.id, flags, name, format, 0);
  let mut segment = |out: &mut ByteBuffer, words: usize, before: usize| {
    left -= before;
    out.write_u8(words as u8);
    out.write_u8(left as u8);
  };
  segment(out, 8, 20);
  for w in [0, host, rx_flow, 1, 0, 0, 0] {
    out.write_u16(w);
  }
  segment(out, 4, 8);
  out.write_u16(if destination == 0 { 0x0100 } else { 0x0101 });
  out.write_u16(destination);
  out.write_u16(0);
  segment(out, chan_words, 4);
  out.write_u16(channels.len() as u16);
  for c in channels {
    out.write_u16(*c);
  }
  out.write_u16(0);
  segment(out, 2, chan_words);
  out.write_u16(f.fpp);
  at as u16
}

/// Answer to 0x2600.
pub fn tx_flows_page(self_info: &DeviceInfo, request: &[u8], slots: usize, flows: Vec<ListedTxFlow>) -> (u16, Vec<u8>) {
  match first_flow(request) {
    Some(first) => flow_page(self_info, slots, flows.into_iter().filter(|f| f.id >= first), write_tx_flow),
    None => (0x22, vec![]),
  }
}

/// Answer to 0x2601: the new flow.
pub fn created_flow(self_info: &DeviceInfo, slots: usize, flow: ListedTxFlow) -> Vec<u8> {
  flow_page(self_info, slots, std::iter::once(flow), write_tx_flow).1
}

/// Multicast flow 0x2601 asks for: id and channel ids; None when malformed or
/// not a single multicast flow of 1-8 different channels.
pub fn parse_create(content: &[u8]) -> Option<(u16, Vec<u16>)> {
  let word = |at: usize| content.get(at..at + 2).map(|b| make_u16(b[0], b[1]));
  if *content.get(7)? != 1 {
    return None;
  }
  let record = (word(8)? as usize).checked_sub(HEADER_LENGTH)?;
  let end = record + 2 * *content.get(record + 1)? as usize;
  let mut segments = vec![];
  let mut at = record;
  while at < end {
    let words = *content.get(at)? as usize;
    if words == 0 || at + 2 * words > end || at + 2 * words > content.len() {
      return None;
    }
    segments.push((at, 2 * words));
    at += 2 * words;
  }
  let (head, head_len) = *segments.first()?;
  let (chans, chans_len) = *segments.get(3)?;
  if head_len < 16 || chans_len < 4 {
    return None;
  }
  let flags = (word(head + 12)? as u32) << 16 | word(head + 14)? as u32;
  let id = word(head + 8)?;
  let n = word(chans + 2)? as usize;
  if flags & FLOW_MULTICAST == 0 || id == 0 || n == 0 || n > 8 || 4 + 2 * n > chans_len {
    return None;
  }
  let channels = (0..n).map(|i| word(chans + 4 + 2 * i)).collect::<Option<Vec<_>>>()?;
  let distinct: std::collections::BTreeSet<_> = channels.iter().collect();
  (distinct.len() == n && !channels.contains(&0)).then_some((id, channels))
}

/// Flow id 0x2602 deletes.
pub fn parse_delete(content: &[u8]) -> Option<u16> {
  let b = content.get(12..14)?;
  Some(make_u16(b[0], b[1])).filter(|id| *id > 0)
}

/// Answer to 0x3600.
pub fn rx_flows_page(self_info: &DeviceInfo, request: &[u8], slots: usize, flows: Vec<ListedRxFlow>) -> (u16, Vec<u8>) {
  let Some(first) = first_flow(request) else { return (0x22, vec![]) };
  flow_page(self_info, slots, flows.into_iter().filter(|f| f.id >= first), |out, format, f| {
    let name = write_0term_str_to_bytebuffer(out, &f.id.to_string());
    align_wpos(out, 4);
    let at = out.get_wpos();
    flow_head(out, 0x22, f.id, 1, name, format, f.latency_ns);
    out.write_u16(0x0a0e);
    out.write_bytes(&[0; 10]);
    out.write_u16(f.slots.len() as u16);
    out.write_u16(at as u16 + 76);
    out.write_bytes(&[0; 4]);
    out.write_u16(0x0400);
    out.write_u16(0x0101);
    out.write_u16(at as u16 + 68);
    out.write_u16(0);
    out.write_u16(0x0802);
    out.write_u16(f.port);
    out.write_bytes(&self_info.ip_address.octets());
    // Which slot feeds each receive channel, from the first one fed
    let fed: Vec<(usize, u8)> =
      f.slots.iter().enumerate().flat_map(|(s, chs)| chs.iter().map(move |c| (*c, s as u8 + 1))).collect();
    let first = fed.iter().map(|x| x.0).min().unwrap_or(0);
    let last = fed.iter().map(|x| x.0).max().unwrap_or(0);
    let mut map = vec![0u8; (last - first + 2) / 2 * 2];
    for (c, s) in fed {
      map[c - first] = s;
    }
    out.write_u16(first as u16 + 1);
    out.write_u16(map.len() as u16);
    out.write_u16(0);
    out.write_bytes(&map);
    at as u16
  })
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::device_info::Channel;
  use std::sync::RwLock;

  fn device(n: usize) -> DeviceInfo {
    let channels = || {
      (1..=n)
        .map(|i| Channel { factory_name: format!("{i:02}"), friendly_name: RwLock::new(format!("Ch {i}")).into() })
        .collect::<Vec<_>>()
    };
    DeviceInfo {
      ip_address: std::net::Ipv4Addr::new(192, 168, 1, 1),
      netmask: std::net::Ipv4Addr::new(255, 255, 255, 0),
      gateway: std::net::Ipv4Addr::new(192, 168, 1, 1),
      mac_address: netdev::mac::MacAddr::from_hex_format("00:11:22:33:44:55"),
      link_speed: 1000,
      board_name: "B".into(),
      manufacturer: "M".into(),
      model_name: "N".into(),
      model_number: "1".into(),
      factory_device_id: [0; 8],
      process_id: 1,
      vendor_string: "V".into(),
      friendly_hostname: "dev".into(),
      factory_hostname: "fac".into(),
      rx_channels: channels(),
      tx_channels: channels(),
      bits_per_sample: 24,
      pcm_type: 0x0e,
      latency_ns: 1_000_000,
      sample_rate: 48000,
      arc_port: 4440,
      cmc_port: 8800,
      flows_control_port: 4455,
      info_request_port: 8700,
    }
  }
  fn w(b: &[u8], packet_offset: usize) -> u16 {
    let at = packet_offset - HEADER_LENGTH;
    make_u16(b[at], b[at + 1])
  }
  fn text(b: &[u8], packet_offset: u16) -> &str {
    read_0term_str_from_buffer(b, packet_offset as usize - HEADER_LENGTH).unwrap()
  }
  fn page(first: u16) -> Vec<u8> {
    let mut r = vec![0, 0, 0, 0, 0, 0, 0, 1, 0, 1];
    r.extend_from_slice(&first.to_be_bytes());
    r.extend_from_slice(&[0, 0]);
    r
  }

  /// The record of Dante Virtual Soundcard 4.5.2 with 8x8 channels.
  #[test]
  fn media_record_as_dvs() {
    let r = media_record(34, 8, 8, 4, 4, 8);
    assert_eq!(w(&r, HEADER_LENGTH), 48);
    assert_eq!(
      hex::encode(&r[4..]),
      "14140003000100010008000800080008000400040004000400080008000800080000000000000000"
    );
    assert_eq!(r.len(), 4 + 40);
  }

  #[test]
  fn names_point_to_strings() {
    let n = device_names("virgil", "factory", "board", ":705");
    assert_eq!(text(&n, 60), "virgil");
    assert_eq!(text(&n, 92), "factory");
    assert_eq!(text(&n, w(&n, 16)), "board");
    assert_eq!(text(&n, w(&n, 18)), ":705");
    assert_eq!(w(&n, 44), 0x0404);
    assert_eq!(w(&n, 48), 0x280f);
    // the names of long devices are cut, not overlapping
    let n = device_names(&"x".repeat(60), "f", "b", "r");
    assert_eq!(text(&n, 60).len(), 31);
    assert_eq!(text(&n, 92), "f");
  }

  #[test]
  fn pages_list_every_channel_once() {
    for rx in [false, true] {
      for n in [0, 1, 8, 64, 128] {
        let d = device(n);
        let sub = |i: usize| {
          (i % 2 == 0).then(|| ListedSubscription {
            tx_channel: "x".repeat(63),
            tx_host: "Y000".into(),
            status: 0x01010009,
          })
        };
        let (mut next, mut pages) = (1u16, 0);
        loop {
          let (code, body) = channels_page(&d, &page(next), if rx { Some(&sub) } else { None });
          assert!(body.len() + HEADER_LENGTH < 1400);
          let count = body[7] as u16;
          for k in 0..count {
            let rec = w(&body, 18 + 2 * k as usize) as usize;
            assert_eq!(rec % 4, 0);
            assert_eq!(w(&body, rec + 2), next + k);
            assert_eq!(text(&body, w(&body, rec + 20)), format!("Ch {}", next + k));
            assert_eq!(w(&body, rec), if rx { 0x141c } else { 0x1414 });
            if rx && (next + k) % 2 == 1 {
              assert_eq!(text(&body, w(&body, rec + 46)), "Y000");
              assert_eq!((w(&body, rec + 48), w(&body, rec + 50)), (9, 0x0101));
            }
          }
          next += count;
          pages += 1;
          assert!(pages < 100);
          if code != STATUS_MORE {
            break;
          }
          assert!(count > 0);
        }
        assert_eq!(next as usize, n + 1);
      }
    }
  }

  #[test]
  fn malformed_requests() {
    let d = device(2);
    for len in 0..12 {
      assert_eq!(channels_page(&d, &vec![0; len], None).0, 0x22);
    }
    assert_eq!(channels_page(&d, &page(500), None).1[7], 0);
    for len in 0..40 {
      for fill in [0u8, 1, 3, 6, 8, 255] {
        let mut c = vec![fill; len];
        parse_renames(&c);
        parse_subscriptions(&c);
        if len > 9 {
          c[7] = 0;
          for size in [6, 8] {
            c[6] = size;
            parse_renames(&c);
            parse_subscriptions(&c);
          }
        }
      }
    }
  }

  #[test]
  fn writes() {
    // channel 2 -> "Vox", channel 3 -> default
    let mut c = vec![0, 0, 0, 0, 0, 0, 6, 0, 2, 2, 0, 2, 0, 3, 0, 32, 0, 3, 0, 3, 0, 0];
    c.extend_from_slice(b"Vox\0");
    let names = parse_renames(&c).unwrap();
    assert_eq!(names, vec![(2, "Vox".to_owned()), (3, String::new())]);
    let echo = renames_echo(&names);
    assert_eq!(text(&echo, w(&echo, 24)), "Vox");
    assert_eq!(text(&echo, w(&echo, 30)), "");

    let mut c = vec![0, 0, 0, 0, 0, 0, 8, 0, 2, 2, 0, 1, 0, 3, 0, 36, 0, 39, 0, 2, 0, 3, 0, 0, 0, 0];
    c.extend_from_slice(b"L1\0desk\0");
    assert_eq!(
      parse_subscriptions(&c).unwrap(),
      vec![(1, Some(("L1".to_owned(), "desk".to_owned()))), (2, None)]
    );
  }

  fn tx_flow(id: u16) -> ListedTxFlow {
    ListedTxFlow {
      id,
      name: format!("{id}_1"),
      rx_host: Some("desk".into()),
      rx_flow: Some("f".into()),
      destination: Some((std::net::Ipv4Addr::new(10, 0, 0, 9), 14339)),
      channels: vec![1, 0],
      fpp: 16,
    }
  }

  /// Segment layout of a 2.8.9 card's unicast flow of 2 slots
  /// (dstack issue 223): 14 27, 08 13, 04 0b, 05 07, 02 02.
  #[test]
  fn tx_flow_record() {
    let d = device(2);
    let (code, b) = tx_flows_page(&d, &page(1), 16, vec![tx_flow(1)]);
    assert_eq!((code, b[6], b[7]), (1, 16, 1));
    let at = w(&b, 18) as usize;
    for (off, v) in [(0, 0x1427), (40, 0x0813), (56, 0x040b), (64, 0x0507), (74, 0x0202)] {
      assert_eq!(w(&b, at + off), v, "segment at {off}");
    }
    assert_eq!(w(&b, at + 14), 0x11);
    assert_eq!(text(&b, w(&b, at + 20)), "1_1");
    assert_eq!(text(&b, w(&b, at + 44)), "desk");
    assert_eq!(text(&b, w(&b, at + 46)), "f");
    let dst = w(&b, at + 60) as usize;
    assert_eq!((w(&b, dst), w(&b, dst + 2)), (0x0802, 14339));
    assert_eq!((w(&b, at + 66), w(&b, at + 68), w(&b, at + 70)), (2, 1, 0));
    assert_eq!(w(&b, at + 76), 16);
    let format = w(&b, at + 22) as usize;
    assert_eq!(w(&b, format + 6), 24);
  }

  #[test]
  fn flow_pages_and_rx() {
    let d = device(2);
    let (mut next, mut seen) = (1, 0);
    loop {
      let (code, b) = tx_flows_page(&d, &page(next), 16, (1..=32).map(tx_flow).collect());
      assert!(b.len() < 1400 && b[7] > 0);
      seen += b[7] as u16;
      next += b[7] as u16;
      if code == 1 {
        break;
      }
    }
    assert_eq!(seen, 32);
    let rx = ListedRxFlow { id: 1, port: 5000, latency_ns: 1_000_000, slots: vec![vec![2], vec![4]] };
    let (_, b) = rx_flows_page(&d, &page(1), 16, vec![rx]);
    let at = w(&b, 18) as usize;
    assert_eq!(w(&b, at), 0x1422);
    assert_eq!(w(&b, at + 68), 0x0802);
    assert_eq!((w(&b, at + 76), w(&b, at + 78)), (3, 4));
    assert_eq!(&b[at + 82 - HEADER_LENGTH..at + 86 - HEADER_LENGTH], &[1, 0, 2, 0]);
  }

  #[test]
  fn create_request() {
    let mut f = tx_flow(3);
    (f.rx_host, f.rx_flow, f.destination) = (None, None, None);
    f.channels = vec![2, 1];
    let mut req = created_flow(&device(2), 1, f);
    assert_eq!(parse_create(&req), Some((3, vec![2, 1])));
    for len in 0..req.len() {
      parse_create(&req[..len]);
    }
    for i in 0..req.len() {
      for v in [0, 1, 0x7f, 0xff] {
        let old = std::mem::replace(&mut req[i], v);
        parse_create(&req);
        req[i] = old;
      }
    }
    let mut unicast = created_flow(&device(2), 1, tx_flow(3));
    assert_eq!(parse_create(&unicast), None);
    unicast.truncate(13);
    assert_eq!(parse_delete(&unicast), None);
    assert_eq!(parse_delete(&[0, 0, 0, 0, 0, 0, 0, 1, 0, 3, 0, 0, 0, 5]), Some(5));
  }
}
