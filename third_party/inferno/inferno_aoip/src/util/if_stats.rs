//! Virgil patch: traffic counters of the network interface that has a given
//! address, for the utilization and error figures Dante Controller shows.

use std::net::Ipv4Addr;
use std::time::Instant;

/// Counters as the system reports them; they wrap, so only differences count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
  pub tx_bytes: u32,
  pub rx_bytes: u32,
  pub tx_errors: u32,
  pub rx_errors: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Utilization {
  pub tx_rate: u32,
  pub rx_rate: u32,
  pub tx_errors: u32,
  pub rx_errors: u32,
}

/// Bytes per second over `seconds`, errors since `first`.
pub fn utilization(first: &Counters, prev: &Counters, now: &Counters, seconds: f64) -> Utilization {
  let rate = |a: u32, b: u32| {
    if seconds <= 0.0 {
      0
    } else {
      (b.wrapping_sub(a) as f64 / seconds).min(u32::MAX as f64) as u32
    }
  };
  Utilization {
    tx_rate: rate(prev.tx_bytes, now.tx_bytes),
    rx_rate: rate(prev.rx_bytes, now.rx_bytes),
    tx_errors: now.tx_errors.wrapping_sub(first.tx_errors),
    rx_errors: now.rx_errors.wrapping_sub(first.rx_errors),
  }
}

pub struct Monitor {
  ip: Ipv4Addr,
  first: Option<Counters>,
  prev: Option<(Instant, Counters)>,
}

impl Monitor {
  pub fn new(ip: Ipv4Addr) -> Self {
    Self { ip, first: None, prev: None }
  }
  /// None on the first call, and where the system's counters are not read.
  pub fn update(&mut self) -> Option<Utilization> {
    let now = read_counters(self.ip)?;
    let t = Instant::now();
    let first = *self.first.get_or_insert(now);
    let prev = self.prev.replace((t, now));
    prev.map(|(pt, p)| utilization(&first, &p, &now, t.duration_since(pt).as_secs_f64()))
  }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn read_counters(ip: Ipv4Addr) -> Option<Counters> {
  use std::ffi::CStr;
  let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
  // SAFETY: getifaddrs fills a list that stays valid until freeifaddrs; every
  // pointer is checked, and addresses are read as the type their family says.
  unsafe {
    if libc::getifaddrs(&mut list) != 0 {
      return None;
    }
    let find = |wanted: &dyn Fn(&libc::ifaddrs) -> bool| {
      let mut p = list;
      while let Some(ifa) = p.as_ref() {
        if wanted(ifa) {
          return Some(ifa);
        }
        p = ifa.ifa_next;
      }
      None
    };
    let result = find(&|ifa| {
      !ifa.ifa_addr.is_null()
        && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET
        && (*(ifa.ifa_addr as *const libc::sockaddr_in)).sin_addr.s_addr == u32::from_ne_bytes(ip.octets())
    })
    .and_then(|with_ip| {
      let name = CStr::from_ptr(with_ip.ifa_name);
      #[cfg(target_os = "macos")]
      {
        find(&|ifa| {
          !ifa.ifa_addr.is_null()
            && (*ifa.ifa_addr).sa_family as i32 == libc::AF_LINK
            && !ifa.ifa_data.is_null()
            && CStr::from_ptr(ifa.ifa_name) == name
        })
        .map(|ifa| {
          let d = std::ptr::read_unaligned(ifa.ifa_data as *const libc::if_data);
          Counters {
            tx_bytes: d.ifi_obytes,
            rx_bytes: d.ifi_ibytes,
            tx_errors: d.ifi_oerrors,
            rx_errors: d.ifi_ierrors,
          }
        })
      }
      #[cfg(target_os = "linux")]
      {
        let name = name.to_str().ok()?.split(':').next()?.to_owned();
        let get = |what: &str| -> Option<u32> {
          let s = std::fs::read_to_string(format!("/sys/class/net/{name}/statistics/{what}")).ok()?;
          Some(s.trim().parse::<u64>().ok()? as u32)
        };
        Some(Counters {
          tx_bytes: get("tx_bytes")?,
          rx_bytes: get("rx_bytes")?,
          tx_errors: get("tx_errors")?,
          rx_errors: get("rx_errors")?,
        })
      }
    });
    libc::freeifaddrs(list);
    result
  }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn read_counters(_ip: Ipv4Addr) -> Option<Counters> {
  None
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn rates_survive_counter_wrap_and_errors_count_from_the_start() {
    let first = Counters { tx_errors: 5, ..Default::default() };
    let prev = Counters { tx_bytes: u32::MAX - 99, rx_bytes: 1000, tx_errors: 6, rx_errors: 0 };
    let now = Counters { tx_bytes: 900, rx_bytes: 3000, tx_errors: 7, rx_errors: 1 };
    assert_eq!(
      utilization(&first, &prev, &now, 2.0),
      Utilization { tx_rate: 500, rx_rate: 1000, tx_errors: 2, rx_errors: 1 }
    );
    assert_eq!(utilization(&first, &prev, &now, 0.0).tx_rate, 0);
  }

  #[cfg(any(target_os = "macos", target_os = "linux"))]
  #[test]
  fn reads_the_loopback_interface_only_for_its_address() {
    assert!(read_counters(Ipv4Addr::LOCALHOST).is_some());
    assert!(read_counters(Ipv4Addr::new(203, 0, 113, 1)).is_none());
  }
}
