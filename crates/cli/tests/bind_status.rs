use std::process::Command;

#[test]
fn second_review_bind_status_prints_exact_relay_identity_fingerprint() {
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let directory = Cleanup(
        std::env::temp_dir().join(format!("pockymoe-bind-status-{}", uuid::Uuid::new_v4())),
    );
    std::fs::create_dir_all(&directory.0).unwrap();
    let database = directory.0.join("status.sqlite");
    let db = pockymoe_runtime::db::Database::open(&database).unwrap();
    let v: serde_json::Value = serde_json::from_str(include_str!(
        "../../supervisor/tests/fixtures/account-key-v2.json"
    ))
    .unwrap();
    pockymoe_supervisor::bind_account(
        &database,
        v["account"]["accountPublicKey"].as_str().unwrap(),
        v["enrollment"]["enrollSecret"].as_str().unwrap(),
        None,
    )
    .unwrap();
    drop(db);
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_pockymoe"))
            .env_clear()
            .current_dir(&directory.0)
            .args(args)
            .arg("--database")
            .arg(&database)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap()
    };
    let fingerprint = run(&["relay-fingerprint"]);
    let status = run(&["bind", "--status"]);
    assert!(fingerprint.starts_with("SHA-256 "));
    assert!(status.lines().any(|line| line == fingerprint.trim_end()));
}
