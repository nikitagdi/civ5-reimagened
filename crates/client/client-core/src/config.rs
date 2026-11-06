//! `NetConfig`: every connection threshold in one place (net-client 4.7, "all thresholds live in `NetConfig`"),
//! and the server address rule NC-D16 as a pure function.

/// Thresholds and identity of one `ClientCore`. `Debug` is safe to log: the password is not here.
#[derive(Debug, Clone)]
pub struct NetConfig {
    pub build: String,
    pub nick: String,
    /// The mod API the client speaks and the mods it has built in, for `ModHello` (T35). Empty: no `ModHello` is
    /// sent (to the server that is the same as an empty list).
    pub mod_api: (u16, u16, u16),
    pub built_in: Vec<protocol::BuiltMod>,
    /// Seed of the jitter generator (reconnect pauses); tests fix it.
    pub jitter_seed: u64,
    /// Application `Ping` period. Visible tab only.
    pub ping_interval_ms: u64,
    /// No bytes for this long with a ping unanswered: `Poor`.
    pub poor_after_ms: u64,
    /// No bytes for this long: `Lost`, the socket is closed and reconnecting starts.
    pub lost_after_ms: u64,
    /// From `Opened` to `LoggedIn`.
    pub handshake_timeout_ms: u64,
    /// Hidden for longer than this: on return `Ping` at once and `probe_wait_ms` to answer.
    pub probe_after_hidden_ms: u64,
    /// Time to answer the ping sent on return (after a long hiding, after `Online`).
    pub probe_wait_ms: u64,
    /// First pause of the ordinary backoff.
    pub backoff_first_ms: u64,
    /// Longest pause of the ordinary backoff.
    pub backoff_max_ms: u64,
    /// Jitter of the backoff, percent each way.
    pub backoff_jitter_pct: u64,
    /// After this long of stable play the backoff and the fast-failure counter reset.
    pub stable_after_ms: u64,
    /// Silence at which fresh probe sockets start (NC-D17).
    pub probe_start_ms: u64,
    /// A new probe socket this often.
    pub probe_every_ms: u64,
    /// A probe socket that has not opened by then is closed.
    pub probe_attempt_ms: u64,
    /// Probe sockets alive at once.
    pub probe_max_live: usize,
    /// While the last byte is newer than this, the short probe cadence applies; later the backoff.
    pub outage_window_ms: u64,
    /// While offline, one attempt this often (`onLine` can lie).
    pub offline_check_ms: u64,
    /// Waiting for a keyframe after `NeedKeyframe` (NC2).
    pub keyframe_wait_ms: u64,
    /// Orders in flight (NC3).
    pub max_inflight: usize,
    /// Guard of the overlay (NC6).
    pub order_stuck_ms: u64,
    /// Queue of unprocessed messages in a hidden tab, bytes (platform-web).
    pub hidden_backlog_bytes: u64,
}

impl NetConfig {
    /// The values of net-client 4.7.
    pub fn new(build: impl Into<String>, nick: impl Into<String>) -> Self {
        NetConfig {
            build: build.into(),
            nick: nick.into(),
            mod_api: (1, 0, 0),
            built_in: Vec::new(),
            jitter_seed: 0x9E37_79B9_7F4A_7C15,
            ping_interval_ms: 2_000,
            poor_after_ms: 4_000,
            lost_after_ms: 10_000,
            handshake_timeout_ms: 10_000,
            probe_after_hidden_ms: 5_000,
            probe_wait_ms: 3_000,
            backoff_first_ms: 500,
            backoff_max_ms: 10_000,
            backoff_jitter_pct: 20,
            stable_after_ms: 10_000,
            probe_start_ms: 4_000,
            probe_every_ms: 500,
            probe_attempt_ms: 1_000,
            probe_max_live: 2,
            outage_window_ms: 30_000,
            offline_check_ms: 5_000,
            keyframe_wait_ms: 5_000,
            max_inflight: 32,
            order_stuck_ms: 15_000,
            hidden_backlog_bytes: 8 << 20,
        }
    }
}

// ---------------------------------------------------------------- server address (NC-D16)

/// Who asks for an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddrContext<'a> {
    /// A browser page, with its own URL (`https://host[:port]/…`).
    Page { url: &'a str },
    /// A desktop client or a bot: `--server` is set by the user.
    Desktop,
}

/// The address to connect to and, when the `?server=` parameter was dropped, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAddr {
    /// `ws://` or `wss://` URL of the socket.
    pub url: String,
    /// A line for the log when the parameter was ignored.
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AddrError {
    #[error("Plain http is allowed only on local and LAN addresses")]
    PlainHttp,
    #[error("The page address is not understood: {0}")]
    BadPage(String),
    #[error("The server address is not understood: {0}")]
    BadServer(String),
    #[error("No server address")]
    NoServer,
    #[error("A public server needs wss://")]
    PublicNeedsWss,
}

/// A parsed `scheme://host[:port]/path`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Url {
    scheme: String,
    host: String,
    port: Option<u16>,
    path: String,
}

impl Url {
    fn parse(s: &str) -> Option<Url> {
        let (scheme, rest) = s.split_once("://")?;
        let scheme = scheme.to_ascii_lowercase();
        if scheme.is_empty() || !scheme.bytes().all(|b| b.is_ascii_alphabetic()) {
            return None;
        }
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(end);
        if authority.contains('@') || authority.is_empty() {
            return None;
        }
        let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
            let (h, after) = inner.split_once(']')?;
            let port = match after {
                "" => None,
                p => Some(p.strip_prefix(':')?.parse().ok()?),
            };
            (format!("[{}]", h.to_ascii_lowercase()), port)
        } else if let Some((h, p)) = authority.rsplit_once(':') {
            (h.to_ascii_lowercase(), Some(p.parse().ok()?))
        } else {
            (authority.to_ascii_lowercase(), None)
        };
        if host.is_empty() {
            return None;
        }
        let path = if tail.starts_with('/') {
            tail.split(['?', '#']).next().unwrap_or("/").to_string()
        } else {
            "/".to_string()
        };
        Some(Url {
            scheme,
            host,
            port,
            path,
        })
    }

    fn authority(&self) -> String {
        match self.port {
            Some(p) => format!("{}:{p}", self.host),
            None => self.host.clone(),
        }
    }

    fn secure(&self) -> bool {
        matches!(self.scheme.as_str(), "https" | "wss")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostClass {
    /// `localhost`, `127.0.0.1`, `[::1]`, `*.localhost`, `*.test`.
    Local,
    /// 10/8, 172.16/12, 192.168/16, 169.254/16, fc00::/7.
    Private,
    Public,
}

fn classify(host: &str) -> HostClass {
    if host == "localhost"
        || host == "127.0.0.1"
        || host == "[::1]"
        || host.ends_with(".localhost")
        || host.ends_with(".test")
    {
        return HostClass::Local;
    }
    if let Some(v6) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        // fc00::/7 is fc.. and fd.. in the first group.
        let first = v6.split(':').next().unwrap_or("");
        let is_ula = first.len() == 4 && (first.starts_with("fc") || first.starts_with("fd"));
        return if is_ula {
            HostClass::Private
        } else {
            HostClass::Public
        };
    }
    let octets: Vec<u8> = host.split('.').filter_map(|p| p.parse().ok()).collect();
    if octets.len() == 4 && host.split('.').count() == 4 {
        let [a, b, _, _] = [octets[0], octets[1], octets[2], octets[3]];
        let private = a == 10
            || (a == 172 && (16..=31).contains(&b))
            || (a == 192 && b == 168)
            || (a == 169 && b == 254);
        if private {
            return HostClass::Private;
        }
        if a == 127 {
            return HostClass::Local;
        }
    }
    HostClass::Public
}

/// The socket URL of the page itself: `wss://` for `https://`, `ws://` for `http://` (local and LAN only).
fn page_addr(page: &Url) -> Result<String, AddrError> {
    let ws = match page.scheme.as_str() {
        "https" => "wss",
        "http" if classify(&page.host) != HostClass::Public => "ws",
        "http" => return Err(AddrError::PlainHttp),
        other => return Err(AddrError::BadPage(format!("scheme {other}"))),
    };
    Ok(format!("{ws}://{}/ws", page.authority()))
}

/// NC-D16: where the password may go. The page's own address, or the address a developer chose with
/// `?server=` / `--server`. Everything else is ignored with a warning.
///
/// - browser, no parameter: the page address (`http://` only on local and LAN pages);
/// - browser, with parameter: local hosts are taken as they are (`ws://` too); LAN hosts only when the
///   page itself is on a local address or on that same LAN address; a public host only when it is the
///   page's own address over `wss://`; otherwise the parameter is dropped with a warning;
/// - desktop: `--server` is required; local and LAN hosts are taken, a public host only over `wss://`.
pub fn resolve_server(
    ctx: AddrContext<'_>,
    server_param: Option<&str>,
) -> Result<ServerAddr, AddrError> {
    match ctx {
        AddrContext::Desktop => {
            let raw = server_param.ok_or(AddrError::NoServer)?;
            let u = Url::parse(raw).ok_or_else(|| AddrError::BadServer(redact(raw)))?;
            if !matches!(u.scheme.as_str(), "ws" | "wss") {
                return Err(AddrError::BadServer(redact(raw)));
            }
            if classify(&u.host) == HostClass::Public && !u.secure() {
                return Err(AddrError::PublicNeedsWss);
            }
            Ok(ServerAddr {
                url: normalize(&u),
                warning: None,
            })
        }
        AddrContext::Page { url } => {
            let page = Url::parse(url).ok_or_else(|| AddrError::BadPage(redact(url)))?;
            let own = page_addr(&page);
            let Some(raw) = server_param else {
                return own.map(|url| ServerAddr { url, warning: None });
            };
            let accepted = Url::parse(raw)
                .filter(|u| matches!(u.scheme.as_str(), "ws" | "wss"))
                .filter(|u| match classify(&u.host) {
                    HostClass::Local => true,
                    HostClass::Private => {
                        classify(&page.host) == HostClass::Local || page.host == u.host
                    }
                    HostClass::Public => {
                        u.secure() && page.secure() && u.authority() == page.authority()
                    }
                });
            match accepted {
                Some(u) => Ok(ServerAddr {
                    url: normalize(&u),
                    warning: None,
                }),
                None => own.map(|url| ServerAddr {
                    url,
                    warning: Some(
                        "the server parameter is ignored: the password goes only to the page address"
                            .to_string(),
                    ),
                }),
            }
        }
    }
}

fn normalize(u: &Url) -> String {
    format!("{}://{}{}", u.scheme, u.authority(), u.path)
}

/// Errors never echo input that could carry user info or a password.
fn redact(s: &str) -> String {
    if s.contains('@') {
        "(not shown)".to_string()
    } else {
        s.chars().take(64).collect()
    }
}

/// The HTTP base of a socket URL for the catalog: `wss://h/ws` is `https://h`, `ws://h:8480/ws` is
/// `http://h:8480`.
pub fn http_base(ws_url: &str) -> Option<String> {
    let u = Url::parse(ws_url)?;
    let scheme = match u.scheme.as_str() {
        "wss" => "https",
        "ws" => "http",
        _ => return None,
    };
    Some(format!("{scheme}://{}", u.authority()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(url: &str, param: Option<&str>) -> Result<ServerAddr, AddrError> {
        resolve_server(AddrContext::Page { url }, param)
    }

    fn ok(r: Result<ServerAddr, AddrError>) -> String {
        r.unwrap().url
    }

    #[test]
    fn defaults_follow_the_page() {
        assert_eq!(
            ok(page("https://play.example.com/", None)),
            "wss://play.example.com/ws"
        );
        assert_eq!(
            ok(page("https://play.example.com:8443/index.html?x=1", None)),
            "wss://play.example.com:8443/ws"
        );
        assert_eq!(
            ok(page("http://localhost:8080/", None)),
            "ws://localhost:8080/ws"
        );
        assert_eq!(
            ok(page("http://192.168.1.10:8480/", None)),
            "ws://192.168.1.10:8480/ws"
        );
        assert_eq!(ok(page("http://[::1]:8080/", None)), "ws://[::1]:8080/ws");
        assert_eq!(
            ok(page("http://dev.localhost/", None)),
            "ws://dev.localhost/ws"
        );
    }

    #[test]
    fn plain_http_on_a_public_address_never_starts() {
        for p in [
            "http://play.example.com/",
            "http://8.8.8.8/",
            "http://172.32.0.1/",
        ] {
            assert_eq!(page(p, None), Err(AddrError::PlainHttp), "{p}");
            // The parameter cannot rescue it: a public page falls back to its own address.
            assert_eq!(
                page(p, Some("ws://127.0.0.1:1/ws")).map(|a| a.url),
                Ok("ws://127.0.0.1:1/ws".to_string()),
                "a local server may be chosen from any page, {p}"
            );
        }
        assert_eq!(
            page("http://play.example.com/", Some("ws://8.8.4.4/ws")),
            Err(AddrError::PlainHttp)
        );
    }

    #[test]
    fn public_https_page_accepts_only_its_own_address() {
        let p = "https://play.example.com/";
        // Own address over wss: same as the default.
        assert_eq!(
            ok(page(p, Some("wss://play.example.com/ws"))),
            "wss://play.example.com/ws"
        );
        // A foreign public host, a LAN host, plain ws: ignored with a warning.
        for param in [
            "wss://evil.example.org/ws",
            "ws://evil.example.org/ws",
            "ws://192.168.1.10:8480/ws",
            "wss://192.168.1.10:8480/ws",
            "ws://10.0.0.5/ws",
            "ws://play.example.com/ws",
            "garbage",
            "https://play.example.com/ws",
        ] {
            let a = page(p, Some(param)).unwrap();
            assert_eq!(a.url, "wss://play.example.com/ws", "{param}");
            assert!(a.warning.is_some(), "{param}");
        }
        // A local server is allowed from any page: this is the developer's machine.
        assert_eq!(
            ok(page(p, Some("ws://localhost:8480/ws"))),
            "ws://localhost:8480/ws"
        );
    }

    #[test]
    fn localhost_page_accepts_a_lan_server() {
        let p = "http://localhost:8080/";
        assert_eq!(
            ok(page(p, Some("ws://192.168.1.10:8480/ws"))),
            "ws://192.168.1.10:8480/ws"
        );
        assert_eq!(
            ok(page(p, Some("ws://10.1.2.3:1/ws"))),
            "ws://10.1.2.3:1/ws"
        );
        assert_eq!(
            ok(page(p, Some("ws://[fd12::1]:1/ws"))),
            "ws://[fd12::1]:1/ws"
        );
        assert_eq!(
            ok(page(p, Some("ws://169.254.1.1:1/ws"))),
            "ws://169.254.1.1:1/ws"
        );
        assert_eq!(
            ok(page(p, Some("ws://172.16.0.1:1/ws"))),
            "ws://172.16.0.1:1/ws"
        );
        // The ranges end where they should.
        for bad in [
            "ws://172.32.0.1/ws",
            "ws://192.169.0.1/ws",
            "ws://11.0.0.1/ws",
            "ws://[fe80::1]/ws",
        ] {
            let a = page(p, Some(bad)).unwrap();
            assert_eq!(a.url, "ws://localhost:8080/ws", "{bad}");
            assert!(a.warning.is_some(), "{bad}");
        }
        // A public server from a plain-http local page: ignored.
        assert!(
            page(p, Some("wss://play.example.com/ws"))
                .unwrap()
                .warning
                .is_some()
        );
    }

    #[test]
    fn lan_page_accepts_a_server_on_the_same_lan_address_only() {
        let p = "http://192.168.1.10:8480/";
        assert_eq!(
            ok(page(p, Some("ws://192.168.1.10:8480/ws"))),
            "ws://192.168.1.10:8480/ws"
        );
        // Another LAN host: the page was not opened by the developer on his machine.
        let a = page(p, Some("ws://192.168.1.99:8480/ws")).unwrap();
        assert_eq!(a.url, "ws://192.168.1.10:8480/ws");
        assert!(a.warning.is_some());
    }

    #[test]
    fn desktop_table() {
        let d = |p: Option<&str>| resolve_server(AddrContext::Desktop, p);
        assert_eq!(d(None), Err(AddrError::NoServer));
        assert_eq!(
            d(Some("ws://192.168.1.10:8480/ws")).unwrap().url,
            "ws://192.168.1.10:8480/ws"
        );
        assert_eq!(
            d(Some("ws://127.0.0.1:8080/ws")).unwrap().url,
            "ws://127.0.0.1:8080/ws"
        );
        assert_eq!(
            d(Some("wss://play.example.com/ws")).unwrap().url,
            "wss://play.example.com/ws"
        );
        assert_eq!(
            d(Some("ws://play.example.com/ws")),
            Err(AddrError::PublicNeedsWss)
        );
        assert!(matches!(
            d(Some("http://x/ws")),
            Err(AddrError::BadServer(_))
        ));
        assert!(matches!(d(Some("nonsense")), Err(AddrError::BadServer(_))));
    }

    #[test]
    fn user_info_in_the_url_is_refused() {
        assert!(matches!(
            resolve_server(AddrContext::Desktop, Some("wss://user:pw@host/ws")),
            Err(AddrError::BadServer(_))
        ));
    }

    #[test]
    fn http_base_of_a_socket_url() {
        assert_eq!(http_base("wss://h/ws").as_deref(), Some("https://h"));
        assert_eq!(
            http_base("ws://192.168.1.10:8480/ws").as_deref(),
            Some("http://192.168.1.10:8480")
        );
        assert_eq!(http_base("http://h/ws"), None);
    }

    #[test]
    fn config_has_the_documented_values() {
        let c = NetConfig::new("b", "n");
        assert_eq!(
            (
                c.ping_interval_ms,
                c.poor_after_ms,
                c.lost_after_ms,
                c.handshake_timeout_ms
            ),
            (2_000, 4_000, 10_000, 10_000)
        );
        assert_eq!(
            (
                c.probe_start_ms,
                c.probe_every_ms,
                c.probe_attempt_ms,
                c.probe_max_live,
                c.outage_window_ms
            ),
            (4_000, 500, 1_000, 2, 30_000)
        );
        assert_eq!(
            (c.keyframe_wait_ms, c.max_inflight, c.order_stuck_ms),
            (5_000, 32, 15_000)
        );
        assert_eq!(c.hidden_backlog_bytes, 8 << 20);
        assert_eq!((c.backoff_first_ms, c.backoff_max_ms), (500, 10_000));
    }
}
