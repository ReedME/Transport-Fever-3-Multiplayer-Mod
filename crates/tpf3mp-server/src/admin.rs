//! The admin endpoint: Prometheus metrics at `/metrics`, a health check at
//! `/healthz`, `POST /announce`, which tells everyone connected the
//! request's body (up to 280 bytes of UTF-8), and players' diagnostics:
//! `/diagnostics` lists the sessions and runs with some,
//! `/diagnostics/<code>` gives one session's or run's (`?source=hook` one
//! source's), and `/rooms` the rooms open now with their members (one
//! room by its invite, or those of a player or a name), over plain HTTP.
//! It has no authentication, so it must only listen on a private address:
//! loopback, or a VPN interface.

use std::{sync::Arc, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
};
use tracing::warn;

use tpf3mp_proto::{ChatText, Invite};

use crate::{ServerStats, directory::RoomFilter};

/// Largest request head the endpoint reads.
const MAX_REQUEST: usize = 8 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Requests answered at once; more wait in the listen backlog. A scraper
/// needs one.
const MAX_CONNECTIONS: usize = 16;
/// Pause after a failed accept, such as when the process is out of file
/// descriptors, so the loop neither spins nor gives up.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// Serves the admin endpoint for as long as the server runs. A failed
/// accept is logged and retried: the endpoint must not vanish silently.
pub async fn serve_admin(listener: TcpListener, stats: ServerStats) {
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let Ok(slot) = Arc::clone(&slots).acquire_owned().await else {
            return;
        };
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(error) => {
                warn!(%error, "the admin endpoint cannot accept a connection");
                tokio::time::sleep(ACCEPT_BACKOFF).await;
                continue;
            }
        };
        let stats = stats.clone();
        tokio::spawn(async move {
            let _ = tokio::time::timeout(REQUEST_TIMEOUT, answer(stream, &stats)).await;
            drop(slot);
        });
    }
}

async fn answer(mut stream: TcpStream, stats: &ServerStats) -> std::io::Result<()> {
    let mut head = Vec::with_capacity(512);
    let mut chunk = [0; 512];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk).await?;
        if read == 0 || head.len() + read > MAX_REQUEST {
            return Ok(());
        }
        head.extend_from_slice(&chunk[..read]);
    }
    let end = head
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map_or(head.len(), |at| at + 4);
    let request_line = head.split(|byte| *byte == b'\r').next().unwrap_or_default();
    let (status, content_type, body) = match request_line {
        b"POST /announce HTTP/1.1" | b"POST /announce HTTP/1.0" => {
            match read_body(&mut stream, &head[..end], head[end..].to_vec()).await? {
                Some(text) => match ChatText::new(text.trim()) {
                    Ok(text) if !text.as_str().is_empty() => {
                        let told = stats.announce(text);
                        ("200 OK", "text/plain", format!("told {told} connections\n"))
                    }
                    _ => (
                        "400 Bad Request",
                        "text/plain",
                        "the notice must be 1 to 280 bytes of printable text\n".to_owned(),
                    ),
                },
                None => (
                    "400 Bad Request",
                    "text/plain",
                    "send the notice as the body, with a Content-Length\n".to_owned(),
                ),
            }
        }
        b"GET /metrics HTTP/1.1" | b"GET /metrics HTTP/1.0" => (
            "200 OK",
            "text/plain; version=0.0.4",
            stats.render_metrics(),
        ),
        b"GET /healthz HTTP/1.1" | b"GET /healthz HTTP/1.0" => {
            ("200 OK", "text/plain", "ok\n".to_owned())
        }
        line if line.starts_with(b"GET /diagnostics") => diagnostics(line, stats),
        line if line.starts_with(b"GET /rooms") => rooms(line, stats),
        _ => ("404 Not Found", "text/plain", "not found\n".to_owned()),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

/// `/diagnostics`, the sessions with diagnostics, the latest first, each
/// with its player's ID and name (`?name=ann` keeps those whose name holds
/// it, in any case; `?player=p-…` one player's); or `/diagnostics/<code>`,
/// one session's or run's, one JSON object a line, headed by a line saying
/// who it is, and `/diagnostics/<code>?source=hook` one source's of them.
fn diagnostics(request_line: &[u8], stats: &ServerStats) -> (&'static str, &'static str, String) {
    let not_found = || ("404 Not Found", "text/plain", "not found\n".to_owned());
    let failed = |error: std::io::Error| {
        warn!(%error, "cannot read the players' diagnostics");
        (
            "500 Internal Server Error",
            "text/plain",
            "cannot read the diagnostics\n".to_owned(),
        )
    };
    let line = String::from_utf8_lossy(request_line);
    let Some(path) = line
        .strip_suffix(" HTTP/1.1")
        .or_else(|| line.strip_suffix(" HTTP/1.0"))
        .and_then(|line| line.strip_prefix("GET /diagnostics"))
    else {
        return not_found();
    };
    let (bare, query) = path.split_once('?').unwrap_or((path, ""));
    match bare {
        "" | "/" => {
            let (mut by_name, mut by_player) = (None, None);
            for pair in query.split('&').filter(|pair| !pair.is_empty()) {
                match pair.split_once('=') {
                    Some(("name", value)) => by_name = Some(decode(value).to_lowercase()),
                    Some(("player", value)) => by_player = Some(decode(value)),
                    _ => return not_found(),
                }
            }
            match stats.diagnostics() {
                None => (
                    "404 Not Found",
                    "text/plain",
                    "this server keeps no diagnostics: see --diagnostics-days\n".to_owned(),
                ),
                Some(Err(error)) => failed(error),
                Some(Ok(mut entries)) => {
                    entries.retain(|entry| {
                        by_name.as_deref().is_none_or(|wanted| {
                            entry
                                .name
                                .as_deref()
                                .is_some_and(|name| name.to_lowercase().contains(wanted))
                        }) && by_player
                            .as_deref()
                            .is_none_or(|wanted| entry.player.as_deref() == Some(wanted))
                    });
                    match serde_json::to_string_pretty(&entries) {
                        Ok(json) => ("200 OK", "application/json", json + "\n"),
                        Err(error) => failed(std::io::Error::other(error)),
                    }
                }
            }
        }
        code => {
            // `/diagnostics/<code>?source=hook`: one source's lines.
            let code = code.trim_start_matches('/');
            let source = match query {
                "" => None,
                query => match query
                    .strip_prefix("source=")
                    .and_then(tpf3mp_proto::LogSource::from_name)
                {
                    Some(source) => Some(source),
                    None => return not_found(),
                },
            };
            let summary = match stats.diagnostics_summary(code) {
                Ok(Some(summary)) => summary,
                Ok(None) => return not_found(),
                Err(error) => return failed(error),
            };
            match stats.diagnostics_of(code, source) {
                Ok(Some(lines)) => {
                    // First, who it is: the session's player, or each of the
                    // run's sessions and theirs.
                    let mut body = serde_json::json!({ "who": summary }).to_string();
                    body.push('\n');
                    body.push_str(&String::from_utf8_lossy(&lines));
                    ("200 OK", "application/x-ndjson", body)
                }
                Ok(None) => not_found(),
                Err(error) => failed(error),
            }
        }
    }
}

/// `/rooms`: the rooms open now, each with its members; `?invite=K7QM2X`
/// the room that invite opens (in either case), `?player=p-…` those a
/// player is in, `?name=ann` those with a member whose name holds it, in
/// any case. One filter at most. Never logs the invite.
fn rooms(request_line: &[u8], stats: &ServerStats) -> (&'static str, &'static str, String) {
    let not_found = || {
        (
            "404 Not Found",
            "text/plain",
            "not found
"
            .to_owned(),
        )
    };
    let line = String::from_utf8_lossy(request_line);
    let Some(path) = line
        .strip_suffix(" HTTP/1.1")
        .or_else(|| line.strip_suffix(" HTTP/1.0"))
        .and_then(|line| line.strip_prefix("GET /rooms"))
    else {
        return not_found();
    };
    let (bare, query) = path.split_once('?').unwrap_or((path, ""));
    if !matches!(bare, "" | "/") {
        return not_found();
    }
    let mut filter = RoomFilter::All;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        if !matches!(filter, RoomFilter::All) {
            return not_found();
        }
        filter = match pair.split_once('=') {
            Some(("invite", value)) => match decode(value).parse::<Invite>() {
                Ok(invite) => RoomFilter::Invite(invite),
                Err(error) => {
                    return (
                        "400 Bad Request",
                        "text/plain",
                        format!(
                            "{error}
"
                        ),
                    );
                }
            },
            Some(("player", value)) => RoomFilter::Player(decode(value)),
            Some(("name", value)) => RoomFilter::Name(decode(value).to_lowercase()),
            _ => return not_found(),
        };
    }
    let shown = stats.room_view(&filter);
    if shown.is_empty() && matches!(filter, RoomFilter::Invite(_)) {
        return (
            "404 Not Found",
            "text/plain",
            "no room open now has that invite
"
            .to_owned(),
        );
    }
    match serde_json::to_string_pretty(&shown) {
        Ok(json) => (
            "200 OK",
            "application/json",
            json + "
",
        ),
        Err(error) => {
            warn!(%error, "cannot show the rooms");
            (
                "500 Internal Server Error",
                "text/plain",
                "cannot show the rooms
"
                .to_owned(),
            )
        }
    }
}

/// `text` with `+` and `%XX` decoded, as a query's value comes.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => out.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The request's body as UTF-8, read to its `Content-Length`, of at most a
/// kilobyte: a notice is a line.
async fn read_body(
    stream: &mut TcpStream,
    head: &[u8],
    mut body: Vec<u8>,
) -> std::io::Result<Option<String>> {
    const MAX_BODY: usize = 1024;
    let head = String::from_utf8_lossy(head);
    let Some(length) = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())
            .flatten()
    }) else {
        return Ok(None);
    };
    if length > MAX_BODY {
        return Ok(None);
    }
    let mut chunk = [0; 512];
    while body.len() < length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(length);
    Ok(String::from_utf8(body).ok())
}
