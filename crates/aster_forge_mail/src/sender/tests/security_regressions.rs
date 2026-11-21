//! Regressions at the SMTP/message dependency boundary for lettre 0.11.24.
use crate::{
    MailMessage, MailRecipient,
    sender::{build_lettre_message, mailbox},
};
use lettre::transport::smtp::{
    client::{AsyncSmtpConnection, TlsParameters},
    extension::{ClientId, ServerInfo},
    response::Response,
};
use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::{TcpListener, TcpStream},
    time::Duration,
};

#[test]
fn whitespace_only_ehlo_lines_do_not_panic() {
    for reply in [
        "250-host.example\r\n250- \t \r\n250 AUTH PLAIN\r\n",
        "250   \t\r\n",
    ] {
        let response: Response = reply.parse().unwrap();
        let result = ServerInfo::from_response(&response);
        if reply.starts_with("250-host") {
            assert_eq!(result.unwrap().name(), "host.example");
        } else {
            assert!(result.is_err());
        }
    }
}

#[test]
fn quoted_local_parts_and_hostile_display_names_preserve_the_recipient() {
    for address in [
        r#""a b"@example.com"#,
        r#""john..doe"@example.com"#,
        r#""d "@example.com"#,
    ] {
        for name in [
            None,
            Some("normal"),
            Some("line\r\nbreak"),
            Some("\r"),
            Some("\n"),
        ] {
            let recipient = mailbox(MailRecipient {
                address: address.into(),
                display_name: name.map(str::to_owned),
            })
            .unwrap();
            assert_eq!(recipient.email.to_string(), address);
            let text = recipient.to_string();
            assert!(!text.contains(['\r', '\n']));
            let parsed: lettre::message::Mailbox = text.parse().unwrap();
            assert_eq!(parsed.email, recipient.email);
        }
    }
}

#[test]
fn message_headers_and_envelope_do_not_redirect_quoted_addresses() {
    let address = r#""a b"@example.com"#;
    let message = build_lettre_message(MailMessage {
        from: MailRecipient {
            address: "sender@example.com".into(),
            display_name: Some("Sender\r\nInjected".into()),
        },
        to: MailRecipient {
            address: address.into(),
            display_name: Some("Recipient\r\nInjected".into()),
        },
        subject: "subject".into(),
        text_body: "body\n.\r\nQUIT\r\n".into(),
        html_body: "<p>body</p>".into(),
    })
    .unwrap();
    assert_eq!(message.envelope().to()[0].to_string(), address);
    let wire = String::from_utf8(message.formatted()).unwrap();
    let headers = wire.split("\r\n\r\n").next().unwrap();
    assert!(!headers.contains("\r\nInjected"));
    assert!(headers.contains("Subject: subject"));
}

fn handshake(listener: TcpListener, extensions: &[u8]) -> BufReader<TcpStream> {
    let (stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut stream = BufReader::new(stream);
    stream.get_mut().write_all(b"220 localhost\r\n").unwrap();
    let mut command = String::new();
    stream.read_line(&mut command).unwrap();
    assert!(command.starts_with("EHLO "));
    stream.get_mut().write_all(extensions).unwrap();
    stream
}

#[tokio::test]
async fn oversized_reply_is_rejected_before_the_peer_closes_or_send_times_out() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let _ = stream.write_all(&vec![b'X'; 100_001]);
        // Keep the malicious unterminated line open; EOF must not cause the rejection.
        let _ = wait.recv_timeout(Duration::from_secs(5));
    });
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        AsyncSmtpConnection::connect_tokio1(
            address,
            Some(Duration::from_secs(5)),
            &ClientId::default(),
            None,
            None,
        ),
    )
    .await;
    let _ = release.send(());
    peer.join().unwrap();
    let error = result
        .expect("reply limit must fail before the transport timeout")
        .err()
        .expect("oversized reply must fail");
    assert!(format!("{error:?}").contains("SMTP response too large"));
}

#[tokio::test]
async fn starttls_rejects_extra_plaintext_response_before_any_tls_handshake() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let mut stream = handshake(listener, b"250-localhost\r\n250 STARTTLS\r\n");
        let mut command = String::new();
        stream.read_line(&mut command).unwrap();
        assert_eq!(command, "STARTTLS\r\n");
        stream
            .get_mut()
            .write_all(b"220 Ready to start TLS\r\n250 injected reply\r\n")
            .unwrap();
        let mut extra = Vec::new();
        stream.read_to_end(&mut extra).unwrap();
        assert_eq!(
            extra,
            Vec::<u8>::new(),
            "no command or TLS handshake may follow a forged plaintext reply"
        );
    });
    let mut connection = AsyncSmtpConnection::connect_tokio1(
        address,
        Some(Duration::from_secs(3)),
        &ClientId::default(),
        None,
        None,
    )
    .await
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        connection.starttls(
            TlsParameters::new("localhost".into()).unwrap(),
            &ClientId::default(),
        ),
    )
    .await
    .unwrap();
    assert!(
        format!("{:?}", result.unwrap_err()).contains("unexpected data after the STARTTLS reply")
    );
    drop(connection);
    peer.join().unwrap();
}

#[tokio::test]
async fn raw_data_dot_stuffing_covers_bare_cr_lf_crlf_and_chunk_boundaries() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let mut stream = handshake(listener, b"250 localhost\r\n");
        let mut bytes = Vec::new();
        loop {
            let mut line = Vec::new();
            assert_ne!(stream.read_until(b'\n', &mut line).unwrap(), 0);
            bytes.extend_from_slice(&line);
            if line == b".\r\n" {
                break;
            }
        }
        stream.get_mut().write_all(b"250 accepted\r\n").unwrap();
        bytes
    });
    let mut connection = AsyncSmtpConnection::connect_tokio1(
        address,
        Some(Duration::from_secs(3)),
        &ClientId::default(),
        None,
        None,
    )
    .await
    .unwrap();
    let chunks: &[&[u8]] = &[
        b".start\n",
        b".after-lf\r",
        b".after-cr\r",
        b"\n",
        b".after-crlf\r\n",
    ];
    connection.message_iter(chunks.iter()).await.unwrap();
    let wire = peer.join().unwrap();
    assert_eq!(
        wire,
        b"..start\n..after-lf\r..after-cr\r\n..after-crlf\r\n.\r\n"
    );
}
