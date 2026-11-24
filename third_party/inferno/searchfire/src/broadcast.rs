//! # mDNS Broadcasting
//!
//! This module provides a way to respond to mDNS queries on the network.
//!
//! In other words, this module provides an _mDNS server_.
//!
//! # Example
//!
//! ```rust, no_run
//! use searchlight::{
//!     broadcast::{BroadcasterBuilder, ServiceBuilder},
//!     discovery::{DiscoveryBuilder, DiscoveryEvent},
//!     net::IpVersion,
//! };
//! use std::{
//!     net::{IpAddr, Ipv4Addr},
//!     str::FromStr,
//! };
//!
//! let (found_tx, found_rx) = std::sync::mpsc::sync_channel(0);
//!
//! let broadcaster = BroadcasterBuilder::new()
//!     .loopback()
//!     .add_service(
//!         ServiceBuilder::new("_searchlight._udp.local.", "HELLO-WORLD", 1234)
//!             .unwrap()
//!             .add_ip_address(IpAddr::V4(Ipv4Addr::from_str("192.168.1.69").unwrap()))
//!             .add_txt_truncated("key=value")
//!             .add_txt_truncated("key2=value2")
//!             .build()
//!             .unwrap(),
//!     )
//!     .build(IpVersion::V4)
//!     .unwrap()
//!     .run_in_background();
//!
//! let discovery = DiscoveryBuilder::new()
//!     .loopback()
//!     .service("_searchlight._udp.local.")
//!     .unwrap()
//!     .build(IpVersion::V4)
//!     .unwrap()
//!     .run_in_background(move |event| {
//!         if let DiscoveryEvent::ResponderFound(responder) = event {
//!             found_tx.try_send(responder).ok();
//!         }
//!     });
//!
//! println!("Waiting for discovery to find responder...");
//!
//! println!("{:#?}", found_rx.recv().unwrap());
//!
//! println!("Shutting down...");
//!
//! broadcaster.shutdown().unwrap();
//! discovery.shutdown().unwrap();
//!
//! println!("Done!");
//! ```

use crate::{
	errors::MultiIpIoError,
	socket::{AsyncMdnsSocket, MdnsSocket, MdnsSocketRecv},
};
use std::{
	collections::BTreeSet,
	sync::{Arc, RwLock},
};
use trust_dns_client::{
	op::Message as DnsMessage,
	serialize::binary::{BinDecodable, BinEncodable, BinEncoder},
};

/// Errors that can occur while broadcasting or initializing a broadcaster.
pub mod errors;

mod builder;
pub use builder::BroadcasterBuilder;

mod service;
use service::ServiceDnsResponse;
pub use service::{IntoServiceTxt, Service, ServiceBuilder};

mod handle;
pub use handle::BroadcasterHandle;
use handle::*;

pub(crate) struct BroadcasterConfig {
	services: BTreeSet<ServiceDnsResponse>,
	/// Virgil patch: unsolicited packets waiting to be sent (announcements of
	/// added services, goodbyes of removed ones). The original only answered
	/// queries, so peers kept stale records (old TXT, removed names) in their
	/// caches until the TTL ran out.
	outbox: Vec<Outgoing>,
	wake: Arc<tokio::sync::Notify>,
}

/// Virgil patch: one unsolicited mDNS packet.
pub(crate) struct Outgoing {
	due: tokio::time::Instant,
	packet: Vec<u8>,
	/// The service id a goodbye withdraws: re-adding that service before
	/// the goodbye went out cancels it (a remove + add of the same name).
	goodbye_of: Option<trust_dns_client::rr::Name>,
}

/// RFC 6762 section 8.3: announce at least twice, one second apart.
const ANNOUNCE_REPEAT: std::time::Duration = std::time::Duration::from_secs(1);

fn encode(message: &DnsMessage) -> Option<Vec<u8>> {
	let mut buf = Vec::with_capacity(512);
	message.emit(&mut BinEncoder::new(&mut buf)).ok()?;
	Some(buf)
}

impl BroadcasterConfig {
	pub(crate) fn new(services: BTreeSet<ServiceDnsResponse>) -> Self {
		let mut config = Self { services: BTreeSet::new(), outbox: Vec::new(), wake: Arc::new(tokio::sync::Notify::new()) };
		for service in services {
			config.add(service);
		}
		config
	}

	/// Adds (or replaces) a service and queues its announcement.
	pub(crate) fn add(&mut self, service: ServiceDnsResponse) -> Option<ServiceDnsResponse> {
		let id = service.service_id().clone();
		self.outbox.retain(|o| o.goodbye_of.as_ref() != Some(&id));
		if let Some(packet) = encode(&service.dns_response) {
			let now = tokio::time::Instant::now();
			self.outbox.push(Outgoing { due: now, packet: packet.clone(), goodbye_of: None });
			self.outbox.push(Outgoing { due: now + ANNOUNCE_REPEAT, packet, goodbye_of: None });
		}
		self.wake.notify_one();
		self.services.replace(service)
	}

	/// Queues a goodbye (every record with TTL 0) for a removed service.
	pub(crate) fn goodbye(&mut self, service: &ServiceDnsResponse) {
		let mut message = service.dns_response.clone();
		for record in message.answers_mut().iter_mut() {
			record.set_ttl(0);
		}
		for record in message.additionals_mut().iter_mut() {
			record.set_ttl(0);
		}
		if let Some(packet) = encode(&message) {
			self.outbox.push(Outgoing {
				due: tokio::time::Instant::now(),
				packet,
				goodbye_of: Some(service.service_id().clone()),
			});
		}
		self.wake.notify_one();
	}

	/// Removes the services `keep` rejects, queueing a goodbye for each.
	pub(crate) fn remove_where(&mut self, mut keep: impl FnMut(&ServiceDnsResponse) -> bool) -> bool {
		let (kept, removed): (BTreeSet<_>, Vec<_>) =
			std::mem::take(&mut self.services).into_iter().fold((BTreeSet::new(), Vec::new()), |(mut k, mut r), s| {
				if keep(&s) {
					k.insert(s);
				} else {
					r.push(s);
				}
				(k, r)
			});
		self.services = kept;
		for service in &removed {
			self.goodbye(service);
		}
		!removed.is_empty()
	}

	/// Queues goodbyes for every service, for a shutdown.
	pub(crate) fn goodbye_all(&mut self) {
		self.outbox.clear();
		let services = std::mem::take(&mut self.services);
		for service in &services {
			self.goodbye(service);
		}
	}

	/// Takes the packets due by `now`; returns them and the next due time.
	fn take_due(&mut self, now: tokio::time::Instant) -> (Vec<Vec<u8>>, Option<tokio::time::Instant>) {
		let mut due = Vec::new();
		let mut next = None::<tokio::time::Instant>;
		self.outbox.retain_mut(|o| {
			if o.due <= now {
				due.push(std::mem::take(&mut o.packet));
				false
			} else {
				next = Some(next.map_or(o.due, |n| n.min(o.due)));
				true
			}
		});
		(due, next)
	}
}

/// A built mDNS broadcaster (server) instance, ready to be started.
///
/// You can choose to run broadcasting on the current thread, or in the background, using [`Broadcaster::run`] or [`Broadcaster::run_in_background`].
///
/// A `Broadcaster` can be built using [`BroadcasterBuilder`].
pub struct Broadcaster {
	socket: MdnsSocket,
	config: Arc<RwLock<BroadcasterConfig>>,
}
impl Broadcaster {
	/// Run broadcasting on a new thread; in the background.
	///
	/// Returns a [`BroadcasterHandle`] that can be used to cleanly shut down the background thread.
	pub fn run_in_background(self) -> BroadcasterHandle {
		let Broadcaster { socket, config } = self;

		let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

		let config_ref = config.clone();
		let thread = std::thread::spawn(move || {
			tokio::runtime::Builder::new_current_thread()
				.thread_name("Searchlight mDNS Broadcaster (Tokio)")
				.enable_all()
				.build()
				.unwrap()
				.block_on(async move {
					let socket = socket.into_async().await?;
					Self::impl_run(&socket, socket.recv(vec![0; 4096]), config_ref, Some(shutdown_rx)).await;
					Ok(())
				})
		});

		BroadcasterHandle(BroadcasterHandleDrop(Some(BroadcasterHandleInner { config, thread, shutdown_tx })))
	}

	/// Run broadcasting on the current thread.
	///
	/// This will start a new Tokio runtime on the current thread and block until a fatal error occurs.
	pub fn run(self) -> Result<(), MultiIpIoError> {
		let Broadcaster { socket, config } = self;

		tokio::runtime::Builder::new_current_thread()
			.thread_name("Searchlight mDNS Broadcaster (Tokio)")
			.enable_all()
			.build()
			.unwrap()
			.block_on(async move {
				let socket = socket.into_async().await?;
				Self::impl_run(&socket, socket.recv(vec![0; 4096]), config, None).await;
				Ok(())
			})
	}
}
impl Broadcaster {
	async fn impl_run(
		tx: &AsyncMdnsSocket,
		mut rx: MdnsSocketRecv<'_>,
		config: Arc<RwLock<BroadcasterConfig>>,
		shutdown_rx: Option<tokio::sync::oneshot::Receiver<()>>,
	) {
		if let Some(shutdown_rx) = shutdown_rx {
			tokio::select! {
				biased;
				_ = Self::recv_loop(tx, &mut rx, &config) => (),
				_ = shutdown_rx => (),
			}
			// Virgil patch: send the goodbyes queued by the shutdown.
			let (packets, _) = config.write().unwrap().take_due(tokio::time::Instant::now() + std::time::Duration::from_secs(3600));
			Self::send_unsolicited(tx, packets).await;
		} else {
			Self::recv_loop(tx, &mut rx, &config).await
		}
	}

	/// Virgil patch: multicasts queued announcements / goodbyes, pausing a
	/// little every few packets so a burst (every channel of a device) is
	/// not dropped by receivers.
	async fn send_unsolicited(tx: &AsyncMdnsSocket, packets: Vec<Vec<u8>>) {
		for (i, packet) in packets.iter().enumerate() {
			if i > 0 && i % 16 == 0 {
				tokio::time::sleep(std::time::Duration::from_millis(2)).await;
			}
			if let Err(err) = tx.send_multicast(packet).await {
				log::warn!("Failed to send unsolicited mDNS packet: {err}");
			}
		}
	}

	#[allow(clippy::await_holding_lock)]
	// It's fine to hold the lock in this case because we're using the current-thread runtime.
	// The future just won't be Send.
	async fn recv_loop(tx: &AsyncMdnsSocket, rx: &mut MdnsSocketRecv<'_>, config: &RwLock<BroadcasterConfig>) {
		let mut send_buf = vec![0u8; 4096];
		let wake = config.read().unwrap().wake.clone();
		loop {
			// Virgil patch: send what is due, then wait for a query, the next
			// due packet or a newly queued one.
			let (packets, next_due) = config.write().unwrap().take_due(tokio::time::Instant::now());
			Self::send_unsolicited(tx, packets).await;
			let sleep_until = next_due.unwrap_or_else(|| tokio::time::Instant::now() + std::time::Duration::from_secs(3600));
			let recv = tokio::select! {
				recv = rx.recv_multicast() => recv,
				_ = tokio::time::sleep_until(sleep_until) => continue,
				_ = wake.notified() => continue,
			};
			let ((count, addr), packet) = match recv {
				Ok(recv) => recv,
				Err(err) => {
					log::warn!("Failed to receive on mDNS socket: {err}");
					continue;
				}
			};
			if count == 0 {
				continue;
			}

			let message = match DnsMessage::from_bytes(packet) {
				Ok(message) if !message.truncated() => message,
				_ => continue,
			};

			for query in message.queries() {
				for service in config.read().unwrap().services.iter().filter(|service| {
					if service.service_type() == query.name() {
						return true;
					}
					if service.service_id() == query.name() {
						return true;
					}

					if let Some(subtype_suffix) = &service.service_subtype_suffix {
						if query.name().to_utf8().ends_with(subtype_suffix) {
							return true;
						}
					}

					false
				}) {
					send_buf.clear();

					if service.dns_response.emit(&mut BinEncoder::new(&mut send_buf)).is_ok() {
						if query.mdns_unicast_response() {
							// Send unicast packet
							if let Err(err) = tx.send_to(&send_buf, addr).await {
								log::warn!("Failed to send unicast mDNS response to {addr}: {err}");
							}
						} else {
							// Send multicast packet
							if let Err(err) = tx.send_multicast(&send_buf).await {
								log::warn!("Failed to send multicast mDNS response (requested by {addr}): {err}");
							}
						}
					}
				}
			}
		}
	}
}
