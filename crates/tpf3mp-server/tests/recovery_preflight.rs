//! The executable refuses incompatible or unidentified room data before
//! generating its development certificate, invite key or storage folders.

use std::{
    fs,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[test]
fn refused_cli_startup_writes_no_certificate_key_or_store() {
    for damaged in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        fs::create_dir(&data).unwrap();
        let log = data.join("legacy.log");
        // The old schema's remainder need not be readable by this binary.
        let payload = [8_u8, 0xff, 0xff];
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&crc32fast::hash(&payload).to_le_bytes());
        bytes.extend_from_slice(&payload);
        if damaged {
            bytes[8] ^= 1;
        }
        fs::write(&log, &bytes).unwrap();
        let certificate = temp.path().join("existing-cert.der");
        fs::write(&certificate, b"existing certificate must remain intact").unwrap();
        let secret = temp.path().join("missing-invite.key");
        let mut child = Command::new(env!("CARGO_BIN_EXE_tpf3mp-server"))
            .arg("--data-dir")
            .arg(&data)
            .arg("--secret-file")
            .arg(&secret)
            .arg("--dev-self-signed")
            .arg(&certificate)
            .arg("--listen")
            .arg("127.0.0.1:0")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("a refusal never starts a server");
            }
            thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains(if damaged {
                "cannot identify the format"
            } else {
                "uses format 8"
            }),
            "{error}"
        );
        assert_eq!(fs::read(&log).unwrap(), bytes);
        assert_eq!(
            fs::read(&certificate).unwrap(),
            b"existing certificate must remain intact"
        );
        assert!(!secret.exists());
        assert!(!data.join("snapshots").exists());
        assert!(!data.join("diagnostics").exists());
    }
}
