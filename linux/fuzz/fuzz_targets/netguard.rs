//! The worker's egress address checks: never panic, never call an address
//! public that the standard library's `is_global` (the IANA special-purpose
//! registries) says is not, treat IPv6 forms of an IPv4 address like that
//! address, and refuse whatever falls in one of the host's own networks.
#![no_main]
// `IpAddr::is_global` as the oracle (cargo-fuzz needs nightly anyway).
#![feature(ip)]

use std::net::IpAddr;

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use tunemesh_worker::netguard::{
    Net, canonical, is_public, refusal, resolve_checked_with, routes_v4, routes_v6,
};

#[derive(Arbitrary, Debug)]
struct Input {
    ip: IpAddr,
    nets: Vec<(IpAddr, u8)>,
    answers: Vec<IpAddr>,
    host: String,
    port: u16,
    table: String,
}

fuzz_target!(|i: Input| {
    let ip = i.ip;
    let c = canonical(ip);
    assert_eq!(canonical(c), c);
    if is_public(ip) {
        assert!(c.is_global(), "{ip} is public, but {c} is not global");
    }
    if c != ip {
        assert_eq!(is_public(ip), is_public(c), "{ip} and {c}");
    }

    let own: Vec<Net> = i.nets.iter().map(|&(a, p)| Net::new(a, p)).collect();
    for (n, &(a, _)) in own.iter().zip(&i.nets) {
        assert!(n.contains(a), "{n:?} does not contain its own address {a}");
        if !is_public(a) {
            continue;
        }
        assert!(refusal(a, &own).is_some());
    }
    match refusal(ip, &own) {
        None => assert!(is_public(ip) && !own.iter().any(|n| n.contains(ip))),
        Some(_) => assert!(!is_public(ip) || own.iter().any(|n| n.contains(ip))),
    }

    let answers = i.answers.clone();
    if let Ok(addrs) = resolve_checked_with(&i.host, i.port, &own, |_, _| Ok(answers.clone())) {
        assert!(!addrs.is_empty());
        for a in addrs {
            assert_eq!(a.port(), i.port);
            assert!(refusal(a.ip(), &own).is_none(), "{} let through", a.ip());
        }
    }

    for n in routes_v4(&i.table).into_iter().chain(routes_v6(&i.table)) {
        assert!(n.contains(n.addr));
    }
});
