use std::process::Command;

#[test]
#[ignore = "starts an ephemeral TLS fixture using Python and OpenSSL"]
fn tls_survives_outbound_relay() {
    let status = Command::new("python3")
        .args(["tests/native_transport_fixture_test.py"])
        .status()
        .expect("start fixture test");
    assert!(status.success());
}
