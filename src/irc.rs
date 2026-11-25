//! IRCv3 wire protocol: parsing, connecting (plain + TLS) and the async
//! connection task. Everything the UI knows about the network arrives as a
//! [`NetEvent`] on a plain `std` channel.

use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf,
};
use tokio::net::TcpStream;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{unbounded_channel, Sender, UnboundedSender};
use tokio::time::{timeout, Instant};

#[derive(Debug, Clone)]
pub struct IrcMessage {
    pub tags: Vec<(String, String)>,
    pub prefix: Option<String>,
    pub command: String,
    pub params: Vec<String>,
}

impl IrcMessage {
    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    pub fn nick(&self) -> Option<&str> {
        let prefix = self.prefix.as_deref()?;
        let end = prefix.find('!').unwrap_or(prefix.len());
        let nick = &prefix[..end];
        if nick.is_empty() {
            None
        } else {
            Some(nick)
        }
    }

    pub fn param(&self, index: usize) -> Option<&str> {
        self.params.get(index).map(String::as_str)
    }

    pub fn trailing(&self) -> &str {
        self.params.last().map(String::as_str).unwrap_or("")
    }
}

fn unescape_tag(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(':') => out.push(';'),
            Some('s') => out.push(' '),
            Some('\\') => out.push('\\'),
            Some('r') => out.push('\r'),
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

pub fn parse(line: &str) -> Option<IrcMessage> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.trim().is_empty() {
        return None;
    }
    let mut rest = line;

    let mut tags = Vec::new();
    if let Some(stripped) = rest.strip_prefix('@') {
        let end = stripped.find(' ')?;
        for item in stripped[..end].split(';') {
            if item.is_empty() {
                continue;
            }
            let (key, value) = item.split_once('=').unwrap_or((item, ""));
            tags.push((key.to_string(), unescape_tag(value)));
        }
        rest = stripped[end + 1..].trim_start_matches(' ');
    }

    let prefix = if let Some(stripped) = rest.strip_prefix(':') {
        let end = stripped.find(' ')?;
        let prefix = stripped[..end].to_string();
        rest = stripped[end + 1..].trim_start_matches(' ');
        Some(prefix)
    } else {
        None
    };

    let mut parts: Vec<String> = Vec::new();
    loop {
        rest = rest.trim_start_matches(' ');
        if rest.is_empty() {
            break;
        }
        if let Some(rest_trim) = rest.strip_prefix(':') {
            parts.push(rest_trim.to_string());
            break;
        }
        match rest.find(' ') {
            Some(idx) => {
                parts.push(rest[..idx].to_string());
                rest = &rest[idx + 1..];
            }
            None => {
                parts.push(rest.to_string());
                break;
            }
        }
    }

    if parts.is_empty() {
        return None;
    }
    let command = parts.remove(0).to_ascii_uppercase();
    Some(IrcMessage {
        tags,
        prefix,
        command,
        params: parts,
    })
}

#[derive(Debug, Clone)]
pub enum NetEvent {
    Connected {
        server: usize,
        token: u64,
    },
    Line {
        server: usize,
        token: u64,
        line: String,
    },
    Closed {
        server: usize,
        token: u64,
        reason: String,
    },
    Error {
        server: usize,
        token: u64,
        message: String,
    },
}

/// Spawn a connection task. Returns the sender used to queue outbound lines.
pub fn connect(
    rt: &Handle,
    server: usize,
    token: u64,
    host: &str,
    port: u16,
    tls: bool,
    events: Sender<NetEvent>,
) -> UnboundedSender<String> {
    let (tx, rx) = unbounded_channel::<String>();
    let host = host.to_string();
    let handle = rt.clone();
    handle.spawn(async move {
        run(server, token, &host, port, tls, events, rx).await;
    });
    tx
}

enum Stream {
    Plain(TcpStream),
    Tls(tokio_native_tls::TlsStream<TcpStream>),
}

impl AsyncRead for Stream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match &mut *self {
            Stream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Stream::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match &mut *self {
            Stream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Stream::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            Stream::Plain(s) => Pin::new(s).poll_flush(cx),
            Stream::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            Stream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Stream::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

async fn open(host: &str, port: u16, tls: bool) -> Result<Stream, String> {
    let addrs: Vec<SocketAddr> = timeout(
        Duration::from_secs(15),
        tokio::net::lookup_host((host, port)),
    )
    .await
    .map_err(|_| "DNS lookup timed out".to_string())?
    .map_err(|e| format!("DNS lookup failed: {e}"))?
    .collect();

    if addrs.is_empty() {
        return Err("no addresses found".to_string());
    }

    let mut last_err = String::from("could not connect");
    for addr in addrs {
        match timeout(Duration::from_secs(12), TcpStream::connect(addr)).await {
            Ok(Ok(socket)) => {
                let _ = socket.set_nodelay(true);
                if !tls {
                    return Ok(Stream::Plain(socket));
                }
                let connector =
                    native_tls::TlsConnector::new().map_err(|e| format!("TLS init failed: {e}"))?;
                let connector = tokio_native_tls::TlsConnector::from(connector);
                return match timeout(Duration::from_secs(15), connector.connect(host, socket)).await
                {
                    Ok(Ok(stream)) => Ok(Stream::Tls(stream)),
                    Ok(Err(e)) => Err(format!("TLS handshake failed: {e}")),
                    Err(_) => Err("TLS handshake timed out".to_string()),
                };
            }
            Ok(Err(e)) => last_err = format!("connect failed: {e}"),
            Err(_) => last_err = "connect timed out".to_string(),
        }
    }
    Err(last_err)
}

async fn read_frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
) -> std::io::Result<usize> {
    const MAX_FRAME_BYTES: usize = 16 * 1024;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(buf.len());
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map(|pos| pos + 1).unwrap_or(available.len());
        if buf.len() + take > MAX_FRAME_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "IRC frame exceeds 16 KiB",
            ));
        }
        buf.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_some() {
            return Ok(buf.len());
        }
    }
}

async fn run(
    server: usize,
    token: u64,
    host: &str,
    port: u16,
    tls: bool,
    events: Sender<NetEvent>,
    mut outbound: tokio::sync::mpsc::UnboundedReceiver<String>,
) {
    let stream = match open(host, port, tls).await {
        Ok(stream) => stream,
        Err(e) => {
            let _ = events
                .send(NetEvent::Error {
                    server,
                    token,
                    message: e,
                })
                .await;
            return;
        }
    };

    if events
        .send(NetEvent::Connected { server, token })
        .await
        .is_err()
    {
        return;
    }

    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    let mut frame = Vec::new();
    let mut keepalive = tokio::time::interval_at(
        Instant::now() + Duration::from_secs(120),
        Duration::from_secs(120),
    );
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            biased;

            command = outbound.recv() => {
                match command {
                    Some(command) => {
                        let bytes = format!("{command}\r\n");
                        if writer.write_all(bytes.as_bytes()).await.is_err() {
                            break;
                        }
                        if writer.flush().await.is_err() {
                            break;
                        }
                    }
                    None => {
                        let _ = writer.write_all(b"QUIT :client closing\r\n").await;
                        let _ = writer.flush().await;
                        let _ = events.send(NetEvent::Closed {
                            server,
                            token,
                            reason: "client closed".to_string(),
                        }).await;
                        return;
                    }
                }
            }

            read = read_frame(&mut reader, &mut frame) => {
                match read {
                    Ok(0) => {
                        let _ = events.send(NetEvent::Closed {
                            server,
                            token,
                            reason: "server closed the connection".to_string(),
                        }).await;
                        return;
                    }
                    Ok(_) => {
                        let raw = String::from_utf8_lossy(&frame)
                            .trim_end_matches(['\r', '\n'])
                            .to_string();
                        frame.clear();
                        if !raw.is_empty() {
                            let _ = events.send(NetEvent::Line { server, token, line: raw }).await;
                        }
                    }
                    Err(e) => {
                        let _ = events.send(NetEvent::Closed {
                            server,
                            token,
                            reason: format!("read error: {e}"),
                        }).await;
                        return;
                    }
                }
            }

            _ = keepalive.tick() => {
                if writer.write_all(b"PING :rirc-keepalive\r\n").await.is_err() {
                    break;
                }
                let _ = writer.flush().await;
            }
        }
    }

    let _ = events
        .send(NetEvent::Closed {
            server,
            token,
            reason: "connection lost".to_string(),
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::{channel, error::TryRecvError, Receiver};

    fn recv_timeout(rx: &mut Receiver<NetEvent>, timeout: Duration) -> Option<NetEvent> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match rx.try_recv() {
                Ok(event) => return Some(event),
                Err(TryRecvError::Empty) => {
                    if std::time::Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(TryRecvError::Disconnected) => return None,
            }
        }
    }

    #[test]
    fn parses_tags_prefix_and_trailing() {
        let msg = parse(
            "@time=2024-01-02T03:04:05.000Z;msgid=abc :nick!user@host PRIVMSG #rust :hello world",
        )
        .unwrap();
        assert_eq!(msg.command, "PRIVMSG");
        assert_eq!(msg.nick(), Some("nick"));
        assert_eq!(msg.param(0), Some("#rust"));
        assert_eq!(msg.trailing(), "hello world");
        assert_eq!(msg.tag("msgid"), Some("abc"));
        assert_eq!(msg.tags.len(), 2);
    }

    #[test]
    fn parses_empty_trailing_and_escapes() {
        let msg = parse("@a=hello\\sworld;b=x\\:y\\\\z :srv NOTICE * :hi there").unwrap();
        assert_eq!(msg.tag("a"), Some("hello world"));
        assert_eq!(msg.tag("b"), Some("x;y\\z"));
        assert_eq!(msg.trailing(), "hi there");
        let msg = parse("PING :").unwrap();
        assert_eq!(msg.trailing(), "");
    }

    #[test]
    fn ignores_garbage() {
        assert!(parse("   ").is_none());
        assert!(parse("@onlytags").is_none());
    }

    #[test]
    fn speaks_to_a_local_server() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(1)
            .build()
            .unwrap();
        let (events, mut rx) = channel(64);
        let outbound = connect(rt.handle(), 5, 1, "127.0.0.1", addr.port(), false, events);

        let first = recv_timeout(&mut rx, Duration::from_secs(5)).expect("connected event");
        assert!(matches!(
            first,
            NetEvent::Connected {
                server: 5,
                token: 1
            }
        ));

        outbound.send("NICK tester".to_string()).unwrap();
        outbound
            .send("USER tester 0 * :tester".to_string())
            .unwrap();

        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(socket.try_clone().unwrap());
        let mut lines = Vec::new();
        for _ in 0..2 {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            lines.push(line.trim_end().to_string());
        }
        assert_eq!(lines[0], "NICK tester");
        assert_eq!(lines[1], "USER tester 0 * :tester");

        // A frame split across writes must stay one frame.
        socket.write_all(b":srv 001 tester :Wel").unwrap();
        socket.flush().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        socket.write_all(b"come\r\n").unwrap();
        socket.flush().unwrap();

        let line = loop {
            match recv_timeout(&mut rx, Duration::from_secs(5)) {
                Some(NetEvent::Line { line, .. }) => break line,
                Some(_) => continue,
                None => panic!("timed out waiting for welcome"),
            }
        };
        assert_eq!(line, ":srv 001 tester :Welcome");

        // Invalid UTF-8 must not kill the connection; following frames survive.
        socket
            .write_all(b":srv NOTICE tester :caf\xe9\r\n")
            .unwrap();
        socket
            .write_all(b":srv NOTICE tester :still-here\r\n")
            .unwrap();
        socket.flush().unwrap();

        let mut notices = Vec::new();
        while notices.len() < 2 {
            match recv_timeout(&mut rx, Duration::from_secs(5)) {
                Some(NetEvent::Line { line, .. }) => {
                    if line.contains("caf") || line.contains("still-here") {
                        notices.push(line);
                    }
                }
                Some(NetEvent::Closed { reason, .. }) => panic!("disconnected: {reason}"),
                Some(_) => {}
                None => panic!("timed out waiting for lossy notice"),
            }
        }
        assert!(notices[0].contains("caf"));
        assert!(notices[1].contains("still-here"));
    }

    #[test]
    fn frame_reader_is_bounded_and_framed() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let data = b"one\r\ntwo\r\n";
        let mut reader = BufReader::new(&data[..]);
        let mut buf = Vec::new();
        assert_eq!(
            rt.block_on(read_frame(&mut reader, &mut buf)).unwrap(),
            buf.len()
        );
        assert_eq!(String::from_utf8_lossy(&buf), "one\r\n");
        buf.clear();
        rt.block_on(read_frame(&mut reader, &mut buf)).unwrap();
        assert_eq!(String::from_utf8_lossy(&buf), "two\r\n");
        buf.clear();
        assert_eq!(rt.block_on(read_frame(&mut reader, &mut buf)).unwrap(), 0);

        let oversized = vec![b'a'; 16 * 1024 + 1];
        let mut reader = BufReader::new(&oversized[..]);
        let mut buf = Vec::new();
        assert!(rt.block_on(read_frame(&mut reader, &mut buf)).is_err());
    }

    #[test]
    #[ignore = "requires outbound internet access; run sparingly (networks may ban automated probes)"]
    fn connects_to_libera_over_tls() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(1)
            .build()
            .unwrap();
        let (events, mut rx) = channel(64);
        let outbound = connect(rt.handle(), 9, 1, "irc.libera.chat", 6697, true, events);

        let deadline = std::time::Instant::now() + Duration::from_secs(25);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match recv_timeout(&mut rx, left).expect("connect event") {
                NetEvent::Connected { .. } => break,
                NetEvent::Error { message, .. } => panic!("TLS connect failed: {message}"),
                _ => continue,
            }
        }

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            % 100_000;
        let nick = format!("rircprobe{suffix}");
        outbound.send(format!("NICK {nick}")).unwrap();
        outbound
            .send("USER rirc 0 * :rirc probe".to_string())
            .unwrap();

        loop {
            match recv_timeout(&mut rx, Duration::from_secs(25)).expect("welcome event") {
                NetEvent::Line { line, .. } => {
                    if line.contains(" 001 ") || line.contains(" 433 ") {
                        println!("registration response: {line}");
                        break;
                    }
                }
                _ => continue,
            }
        }
        let _ = outbound.send("QUIT :probe done".to_string());
    }
}
