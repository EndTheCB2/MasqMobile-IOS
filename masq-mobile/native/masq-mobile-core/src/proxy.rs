const MAX_HEADER_BYTES: usize = 16 * 1024;

#[derive(Debug, Eq, PartialEq)]
pub struct ConnectTarget {
    pub host: String,
    pub port: u16,
}

pub fn parse_connect_request(header: &[u8]) -> Result<ConnectTarget, &'static str> {
    if header.len() > MAX_HEADER_BYTES {
        return Err("proxy header too large");
    }
    let text = std::str::from_utf8(header).map_err(|_| "proxy header is not UTF-8")?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().ok_or("missing request line")?;
    let mut fields = request_line.split_ascii_whitespace();
    if fields.next() != Some("CONNECT") {
        return Err("only CONNECT is accepted");
    }
    let authority = fields.next().ok_or("missing CONNECT authority")?;
    if fields.next() != Some("HTTP/1.1") || fields.next().is_some() {
        return Err("CONNECT requires HTTP/1.1");
    }
    if lines.any(|line| {
        line.to_ascii_lowercase()
            .starts_with("proxy-authorization:")
    }) {
        return Err("proxy authentication is not accepted");
    }

    let (host, port) = split_authority(authority)?;
    if port != 443 {
        return Err("mobile MASQ accepts HTTPS port 443 only");
    }
    Ok(ConnectTarget {
        host: host.to_owned(),
        port,
    })
}

fn split_authority(authority: &str) -> Result<(&str, u16), &'static str> {
    let (host, port) = if authority.starts_with('[') {
        let closing = authority.find(']').ok_or("invalid IPv6 authority")?;
        let host = &authority[1..closing];
        let port = authority
            .get(closing + 1..)
            .and_then(|tail| tail.strip_prefix(':'))
            .ok_or("missing port")?;
        (host, port)
    } else {
        authority.rsplit_once(':').ok_or("missing port")?
    };
    if host.is_empty() {
        return Err("missing host");
    }
    let port = port.parse::<u16>().map_err(|_| "invalid port")?;
    Ok((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_an_https_connect_tunnel() {
        let parsed = parse_connect_request(
            b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n",
        )
        .unwrap();
        assert_eq!(
            parsed,
            ConnectTarget {
                host: "example.com".to_owned(),
                port: 443
            }
        );
    }

    #[test]
    fn rejects_plain_http_and_proxy_credentials() {
        assert!(parse_connect_request(b"GET http://example.com/ HTTP/1.1\r\n\r\n").is_err());
        assert!(parse_connect_request(
            b"CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Basic x\r\n\r\n"
        )
        .is_err());
    }

    #[test]
    fn rejects_non_https_ports() {
        assert!(parse_connect_request(b"CONNECT example.com:80 HTTP/1.1\r\n\r\n").is_err());
    }
}
