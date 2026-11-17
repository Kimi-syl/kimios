//! TCP/IP networking: smoltcp on top of the virtio-net driver.
//!
//! `init` brings up the interface and a DHCPv4 client; `poll` (driven from the
//! timer tick and from socket calls) processes ingress/egress and applies DHCP
//! leases. On top of that this module offers DNS resolution and a small pool of
//! TCP sockets, exposed to user space through the `net` syscall.
//!
//! Syscalls run with interrupts masked, so blocking helpers drive `poll_state`
//! themselves and busy-wait on the always-running system counter.

use alloc::vec::Vec;
use spin::Mutex;

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{dhcpv4, dns, tcp};
use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetAddress, IpAddress, IpCidr, IpEndpoint, IpListenEndpoint, Ipv4Address, Ipv4Cidr,
};

use crate::{println, timer, virtio_net};

const TCP_SLOTS: usize = 4;
const TCP_BUF: usize = 4096;

/// smoltcp physical device backed by virtio-net.
pub struct NetDevice;

pub struct NetRxToken {
    packet: Vec<u8>,
}

pub struct NetTxToken;

impl Device for NetDevice {
    type RxToken<'a> = NetRxToken;
    type TxToken<'a> = NetTxToken;

    fn receive(&mut self, _timestamp: Instant) -> Option<(NetRxToken, NetTxToken)> {
        let mut buffer = [0u8; 2048];
        let length = virtio_net::rx_pop(&mut buffer)?;
        Some((
            NetRxToken { packet: buffer[..length].to_vec() },
            NetTxToken,
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<NetTxToken> {
        if virtio_net::ready() {
            Some(NetTxToken)
        } else {
            None
        }
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1500;
        caps.max_burst_size = Some(1);
        caps
    }
}

impl RxToken for NetRxToken {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.packet)
    }
}

impl TxToken for NetTxToken {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut buffer = alloc::vec![0u8; len];
        let result = f(&mut buffer);
        virtio_net::tx_send(&buffer);
        result
    }
}

struct NetState {
    iface: Interface,
    sockets: SocketSet<'static>,
    dhcp: SocketHandle,
    dns: SocketHandle,
    tcp: [Option<SocketHandle>; TCP_SLOTS],
    ip: Option<Ipv4Cidr>,
    gateway: Option<Ipv4Address>,
    dns_servers: Vec<Ipv4Address>,
}

static NET: Mutex<Option<NetState>> = Mutex::new(None);

fn now() -> Instant {
    Instant::from_millis(timer::uptime_ms() as i64)
}

fn busy_wait(ms: u64) {
    let until = timer::uptime_ms() + ms;
    while timer::uptime_ms() < until {
        core::hint::spin_loop();
    }
}

/// Initialise virtio-net and the smoltcp interface. Returns false if absent.
pub fn init() -> bool {
    let Some(version) = virtio_net::init() else {
        return false;
    };
    let mac = virtio_net::mac();
    let mut config = Config::new(EthernetAddress(mac).into());
    config.random_seed = timer::uptime_ms();
    let mut device = NetDevice;
    let mut iface = Interface::new(config, &mut device, now());
    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::new(IpAddress::v4(0, 0, 0, 0), 0));
    });

    let mut sockets = SocketSet::new(Vec::new());
    let dhcp = sockets.add(dhcpv4::Socket::new());
    let dns = sockets.add(dns::Socket::new(
        &[IpAddress::v4(1, 1, 1, 1), IpAddress::v4(8, 8, 8, 8)],
        Vec::new(),
    ));

    *NET.lock() = Some(NetState {
        iface,
        sockets,
        dhcp,
        dns,
        tcp: [None; TCP_SLOTS],
        ip: None,
        gateway: None,
        dns_servers: Vec::new(),
    });

    println!(
        "virtio-net  : {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} mmio v{} (DHCP starting)",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], version
    );
    true
}

pub fn present() -> bool {
    NET.lock().is_some()
}

enum DhcpOutcome {
    Configured(Ipv4Cidr, Option<Ipv4Address>, Vec<Ipv4Address>),
    Lost,
    None,
}

/// One round of ingress/egress plus DHCP, on an already-locked state.
fn poll_state(state: &mut NetState) {
    let mut device = NetDevice;
    state.iface.poll(now(), &mut device, &mut state.sockets);

    let outcome = {
        let socket = state.sockets.get_mut::<dhcpv4::Socket>(state.dhcp);
        match socket.poll() {
            Some(dhcpv4::Event::Configured(config)) => DhcpOutcome::Configured(
                config.address,
                config.router,
                config.dns_servers.iter().copied().collect(),
            ),
            Some(dhcpv4::Event::Deconfigured) => DhcpOutcome::Lost,
            None => DhcpOutcome::None,
        }
    };

    match outcome {
        DhcpOutcome::Configured(address, router, servers) => {
            state.iface.update_ip_addrs(|addrs| {
                addrs.clear();
                let _ = addrs.push(IpCidr::Ipv4(address));
            });
            if let Some(router) = router {
                let _ = state.iface.routes_mut().add_default_ipv4_route(router);
            }
            // Prefer public resolvers. QEMU's 10.0.2.3 forwarder relies on the
            // host's resolver config, which is absent on some hosts (Android),
            // and smoltcp waits 10 s before falling back to the next server.
            // Public DNS is reachable via slirp UDP forwarding everywhere, so
            // list it first; keep the DHCP-provided servers as fallbacks.
            let mut dns_v4: Vec<Ipv4Address> = Vec::new();
            dns_v4.push(Ipv4Address::new(1, 1, 1, 1));
            dns_v4.push(Ipv4Address::new(8, 8, 8, 8));
            for server in &servers {
                if !dns_v4.contains(server) {
                    dns_v4.push(*server);
                }
            }
            let as_ip: Vec<IpAddress> = dns_v4.iter().map(|&s| IpAddress::Ipv4(s)).collect();
            state.sockets.get_mut::<dns::Socket>(state.dns).update_servers(&as_ip);
            state.ip = Some(address);
            state.gateway = router;
            state.dns_servers = dns_v4;
            println!("[net] DHCP lease: {}", address);
        }
        DhcpOutcome::Lost => {
            state.ip = None;
        }
        DhcpOutcome::None => {}
    }
}

/// Process one round of ingress/egress and apply any DHCP lease.
pub fn poll() {
    let mut guard = NET.lock();
    if let Some(state) = guard.as_mut() {
        poll_state(state);
    }
}

/// Poll until a DHCP lease is acquired or `timeout_ms` elapses.
pub fn wait_for_ip(timeout_ms: u64) -> bool {
    let mut guard = NET.lock();
    let Some(state) = guard.as_mut() else {
        return false;
    };
    let start = timer::uptime_ms();
    loop {
        poll_state(state);
        if state.ip.is_some() {
            return true;
        }
        if timer::uptime_ms() - start > timeout_ms {
            return false;
        }
        busy_wait(5);
    }
}

pub fn ipv4() -> Option<Ipv4Address> {
    NET.lock().as_ref().and_then(|s| s.ip.map(|cidr| cidr.address()))
}

/// Resolve a hostname to an IPv4 address via DNS.
pub fn resolve(name: &str, timeout_ms: u64) -> Option<Ipv4Address> {
    let mut guard = NET.lock();
    let state = guard.as_mut()?;
    if state.ip.is_none() {
        return None;
    }
    let query = {
        let socket = state.sockets.get_mut::<dns::Socket>(state.dns);
        let cx = state.iface.context();
        socket.start_query(cx, name, smoltcp::wire::DnsQueryType::A).ok()?
    };
    let start = timer::uptime_ms();
    loop {
        poll_state(state);
        let result = state.sockets.get_mut::<dns::Socket>(state.dns).get_query_result(query);
        match result {
            Ok(addrs) => {
                for address in addrs {
                    if let IpAddress::Ipv4(v4) = address {
                        return Some(v4);
                    }
                }
                return None;
            }
            Err(dns::GetQueryResultError::Pending) => {}
            Err(_) => return None,
        }
        if timer::uptime_ms() - start > timeout_ms {
            state.sockets.get_mut::<dns::Socket>(state.dns).cancel_query(query);
            return None;
        }
        busy_wait(2);
    }
}

/// Open a TCP connection; returns a slot index on success.
pub fn tcp_connect(ip: Ipv4Address, port: u16, timeout_ms: u64) -> Option<usize> {
    let mut guard = NET.lock();
    let state = guard.as_mut()?;
    if state.ip.is_none() {
        return None;
    }
    let slot = state.tcp.iter().position(|s| s.is_none())?;
    let rx = smoltcp::storage::RingBuffer::new(alloc::vec![0u8; TCP_BUF]);
    let tx = smoltcp::storage::RingBuffer::new(alloc::vec![0u8; TCP_BUF]);
    let socket = tcp::Socket::new(rx, tx);
    let handle = state.sockets.add(socket);
    state.tcp[slot] = Some(handle);

    let remote = IpEndpoint::new(IpAddress::Ipv4(ip), port);
    let local_port = 49152 + (timer::uptime_ms() % 16000) as u16;
    {
        let sock = state.sockets.get_mut::<tcp::Socket>(handle);
        let cx = state.iface.context();
        let local = IpListenEndpoint { addr: None, port: local_port };
        if sock.connect(cx, remote, local).is_err() {
            state.sockets.remove(handle);
            state.tcp[slot] = None;
            return None;
        }
    }

    let start = timer::uptime_ms();
    loop {
        poll_state(state);
        let status = state.sockets.get_mut::<tcp::Socket>(handle).state();
        if status == tcp::State::Established {
            return Some(slot);
        }
        if status == tcp::State::Closed || timer::uptime_ms() - start > timeout_ms {
            state.sockets.get_mut::<tcp::Socket>(handle).abort();
            state.sockets.remove(handle);
            state.tcp[slot] = None;
            return None;
        }
        busy_wait(2);
    }
}

/// Send bytes on an open TCP slot; returns the number accepted.
pub fn tcp_send(slot: usize, data: &[u8]) -> usize {
    let mut guard = NET.lock();
    let Some(state) = guard.as_mut() else {
        return 0;
    };
    let Some(handle) = state.tcp.get(slot).copied().flatten() else {
        return 0;
    };
    let mut sent = 0;
    let start = timer::uptime_ms();
    while sent < data.len() {
        poll_state(state);
        let sock = state.sockets.get_mut::<tcp::Socket>(handle);
        if sock.can_send() {
            match sock.send_slice(&data[sent..]) {
                Ok(n) => sent += n,
                Err(_) => break,
            }
        }
        if timer::uptime_ms() - start > 5000 {
            break;
        }
        busy_wait(1);
    }
    let flush_start = timer::uptime_ms();
    while timer::uptime_ms() - flush_start < 2000 {
        poll_state(state);
        if state.sockets.get_mut::<tcp::Socket>(handle).send_queue() == 0 {
            break;
        }
        busy_wait(1);
    }
    sent
}

/// Receive bytes from a TCP slot; returns 0 on timeout or close.
pub fn tcp_recv(slot: usize, out: &mut [u8], timeout_ms: u64) -> usize {
    let mut guard = NET.lock();
    let Some(state) = guard.as_mut() else {
        return 0;
    };
    let Some(handle) = state.tcp.get(slot).copied().flatten() else {
        return 0;
    };
    let start = timer::uptime_ms();
    loop {
        poll_state(state);
        let sock = state.sockets.get_mut::<tcp::Socket>(handle);
        if sock.recv_queue() > 0 {
            match sock.recv_slice(out) {
                Ok(n) if n > 0 => return n,
                _ => {}
            }
        }
        if !sock.is_active() {
            return 0;
        }
        if timer::uptime_ms() - start > timeout_ms {
            return 0;
        }
        busy_wait(2);
    }
}

/// Close a TCP slot and release it.
pub fn tcp_close(slot: usize) {
    let mut guard = NET.lock();
    let Some(state) = guard.as_mut() else {
        return;
    };
    if let Some(Some(handle)) = state.tcp.get(slot).copied() {
        state.sockets.get_mut::<tcp::Socket>(handle).close();
        let start = timer::uptime_ms();
        while timer::uptime_ms() - start < 500 {
            poll_state(state);
            busy_wait(1);
        }
        state.sockets.remove(handle);
        state.tcp[slot] = None;
    }
}

/// Print a short status summary (used by the shell `net` command).
pub fn print_status() {
    let guard = NET.lock();
    let Some(state) = guard.as_ref() else {
        println!("net: no interface");
        return;
    };
    let mac = virtio_net::mac();
    println!(
        "net: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );
    match state.ip {
        Some(ip) => println!("net: IP {}", ip),
        None => println!("net: no DHCP lease yet"),
    }
    if let Some(gateway) = state.gateway {
        println!("net: gateway {}", gateway);
    }
    for server in &state.dns_servers {
        println!("net: DNS {}", server);
    }
}
