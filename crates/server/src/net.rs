//! Keeping requests made on a person's say-so off the home network.
//!
//! The server sits inside a home network and fetches addresses people give
//! it: store pages, ntfy topics, webhooks, AI services. Store pages are
//! always public. The others may be on the home network (an Ollama or ntfy
//! of one's own), which is for the administrator to decide; other people on
//! the server can only reach public addresses unless the administrator
//! allows it with `APT_MEMBERS_LOCAL_NETWORK=1`.

use crate::service::{user_problem, Problem};
use crate::{db, App};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

/// True for an address on the public internet: not this machine, not the
/// home network, not a link-local or otherwise special range.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            let inner = |high: u16, low: u16| Ipv4Addr::new((high >> 8) as u8, high as u8, (low >> 8) as u8, low as u8);
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            // Addresses that are an IPv4 address in disguise: NAT64 (64:ff9b::/96) and 6to4 (2002::/16).
            if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                return is_public_v4(inner(s[6], s[7]));
            }
            if s[0] == 0x2002 {
                return is_public_v4(inner(s[1], s[2]));
            }
            // Otherwise only ordinary global addresses (2000::/3) count. That leaves out this
            // machine, link-local, unique-local and site-local addresses, and multicast.
            // Teredo (2001::/32) and the documentation range (2001:db8::/32) are not real destinations.
            (s[0] & 0xe000) == 0x2000 && !(s[0] == 0x2001 && (s[1] == 0 || s[1] == 0x0db8))
        }
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        // Carrier-grade NAT (100.64.0.0/10) and the reserved top of the range.
        || (a == 100 && (64..128).contains(&b))
        || a >= 240)
}

/// Looks names up as usual, then refuses the answers that are not public.
struct PublicOnly;

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            let found: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            let public: Vec<SocketAddr> = found.into_iter().filter(|addr| is_public(addr.ip())).collect();
            if public.is_empty() {
                return Err(format!("{host} is not an address on the public internet").into());
            }
            Ok(Box::new(public.into_iter()) as Addrs)
        })
    }
}

/// True when the address names a machine by number that is not public,
/// such as `http://192.168.1.5:8080/x`. Names are checked when they are looked up.
pub fn is_private_literal(address: &str) -> bool {
    match url::Url::parse(address.trim()).ok().as_ref().and_then(|u| u.host()) {
        Some(url::Host::Ipv4(ip)) => !is_public(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => !is_public(IpAddr::V6(ip)),
        _ => false,
    }
}

/// A client that only ever connects to public addresses: names that resolve
/// to the home network are refused, and so are redirects to a numbered
/// private address.
pub fn public_only(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    // No proxy: a proxy would look the name up again, where this check cannot see.
    builder.no_proxy().dns_resolver(Arc::new(PublicOnly)).redirect(no_private_redirects())
}

fn no_private_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if is_private_literal(attempt.url().as_str()) {
            attempt.error("the address redirects to a machine that is not on the public internet")
        } else if attempt.previous().len() >= 10 {
            attempt.error("too many redirects")
        } else {
            attempt.follow()
        }
    })
}

/// True when every address the name resolves to is public. Used before the
/// server's browser, which looks names up by itself, is sent somewhere.
pub async fn resolves_public(address: &str) -> bool {
    let Ok(url) = url::Url::parse(address) else {
        return false;
    };
    match url.host() {
        Some(url::Host::Domain(name)) => match tokio::net::lookup_host((name, 0)).await {
            Ok(found) => {
                let all: Vec<SocketAddr> = found.collect();
                !all.is_empty() && all.iter().all(|addr| is_public(addr.ip()))
            }
            Err(_) => false,
        },
        Some(_) => !is_private_literal(address),
        None => false,
    }
}

const HOME_NETWORK_ONLY_ADMIN: &str =
    "That address is on the home network (or this machine). Only the administrator can use such an address here.";

/// The client for requests to addresses this person chose (an AI service,
/// an ntfy topic, a webhook). The administrator may reach the home network;
/// anyone else gets a client that only reaches public addresses, and an
/// address that plainly names a private machine is refused outright.
pub async fn client_for(app: &App, user_id: i64, addresses: &[&str]) -> Result<reqwest::Client, Problem> {
    if app.cfg.members_local_network || db::is_admin(&app.db, user_id).await? {
        return Ok(app.http.clone());
    }
    if addresses.iter().any(|address| is_private_literal(address)) {
        return Err(user_problem(HOME_NETWORK_ONLY_ADMIN));
    }
    Ok(app.public_http.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_network_and_special_addresses_are_not_public() {
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.9",
            "192.168.1.20",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fd12:3456::1",
            "::ffff:192.168.1.1",
            "fec0::1",
            "64:ff9b::c0a8:101",
            "64:ff9b:1::1",
            "2002:c0a8:0101::1",
            "2001:db8::1",
            "2001::1",
            "::192.168.1.1",
            "ff02::1",
        ] {
            assert!(!is_public(private.parse().unwrap()), "{private}");
        }
        for public in ["93.184.216.34", "8.8.8.8", "172.32.0.1", "100.128.0.1", "2606:4700:4700::1111", "64:ff9b::808:808"] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
        assert!(is_private_literal("http://192.168.1.5:8080/topic"));
        assert!(is_private_literal("http://[::1]:11434/v1"));
        assert!(!is_private_literal("https://ntfy.sh/topic"));
        assert!(!is_private_literal("http://localhost:11434"), "a name: refused when it is looked up");
    }

    #[tokio::test]
    async fn the_public_only_client_refuses_this_machine_by_name_and_by_redirect() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let client = public_only(reqwest::Client::builder()).build().unwrap();
        // A name that resolves to this machine never gets a connection.
        let refused = client.get("http://localhost:9/").send().await.unwrap_err();
        assert!(crate::fetch::root_cause(&refused).contains("not an address on the public internet"), "{refused:?}");
        assert!(!resolves_public("http://localhost:9/").await);

        // A server that sends visitors on to a private address is not followed there.
        // (Reached here with an ordinary resolver, because the test server is itself local.)
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0u8; 2048];
            let _ = socket.read(&mut buffer).await;
            let _ = socket
                .write_all(
                    b"HTTP/1.1 302 Found\r\nlocation: http://192.168.1.1/admin\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await;
        });
        let follows = reqwest::Client::builder().redirect(no_private_redirects()).build().unwrap();
        let stopped = follows.get(&address).send().await.unwrap_err();
        assert!(stopped.is_redirect(), "{stopped:?}");
    }

    #[tokio::test]
    async fn only_the_administrator_reaches_the_home_network() {
        let app = crate::test_app().await;
        let admin = db::create_user(&app.db, "sam", "h", true).await.unwrap();
        let member = db::create_user(&app.db, "alex", "h", false).await.unwrap();
        let local = "http://192.168.1.5:11434/v1";
        assert!(client_for(&app, admin, &[local]).await.is_ok());
        assert!(matches!(client_for(&app, member, &[local]).await, Err(Problem::User(_))));
        assert!(client_for(&app, member, &["https://ntfy.sh/topic", ""]).await.is_ok());
        // By name: the member's client is the one that refuses private answers when it connects.
        let client = client_for(&app, member, &["http://localhost:9/topic"]).await.unwrap();
        assert!(client.get("http://localhost:9/topic").send().await.is_err());
    }
}
