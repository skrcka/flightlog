//! Adversarial CLI regression tests. Never touch actual agent stores or networks.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output},
};
struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}
impl Fixture {
    fn edit_manifest(&self, edit: impl FnOnce(&mut Value)) {
        use std::io::Read;
        let mut z =
            zip::ZipArchive::new(fs::File::open(self.root.join("test.zip")).unwrap()).unwrap();
        let mut manifest: Value =
            serde_json::from_reader(z.by_name("manifest.json").unwrap()).unwrap();
        edit(&mut manifest);
        let mut files = Vec::new();
        for i in 0..z.len() {
            let mut entry = z.by_index(i).unwrap();
            if entry.name() == "manifest.json" {
                continue;
            }
            let name = entry.name().to_owned();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            files.push((name, bytes));
        }
        drop(z);
        self.pack(
            manifest,
            files
                .iter()
                .map(|(name, bytes)| (name.as_str(), bytes.clone()))
                .collect(),
        );
    }
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        Self { _tmp: tmp, root }
    }
    fn cmd(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_flightlog"));
        c.current_dir(&self.root)
            .env("CODEX_HOME", self.root.join("codex"))
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            .env("CURSOR_CONFIG_DIR", self.root.join("cursor"))
            .env("GEMINI_CLI_HOME", self.root.join("gemini"))
            .env_remove("FLIGHTLOG_REDACT_FILE");
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd().args(args).output().unwrap()
    }
    fn bundle(&self, native: Value, extras: &[(&str, Vec<u8>)], unlisted: bool) -> PathBuf {
        let mut files = vec![(
            "trajectory.json",
            serde_json::to_vec(&json!({"schema_version":"ATIF-v1.8","session_id":"s","steps":[]}))
                .unwrap(),
        )];
        files.extend_from_slice(extras);
        let m = json!({"format":"flightlog","format_version":"1.0","bundle_id":"00000000-0000-4000-8000-000000000001","created_at":"2026-09-28T00:00:00Z","source":{"tool":"codex","session_id":"s"},"summary":{"goal":"g","state":"s"},"redaction":{"findings":[]},"native":native,"files":files.iter().filter(|(n,_)| !unlisted || *n=="trajectory.json").map(|(n,b)|json!({"path":n,"bytes":b.len(),"sha256":format!("{:x}",Sha256::digest(b))})).collect::<Vec<_>>()});
        self.pack(m, files)
    }
    fn pack(&self, m: Value, files: Vec<(&str, Vec<u8>)>) -> PathBuf {
        let p = self.root.join("test.zip");
        let mut z = zip::ZipWriter::new(fs::File::create(&p).unwrap());
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        z.start_file("manifest.json", opts).unwrap();
        z.write_all(&serde_json::to_vec(&m).unwrap()).unwrap();
        for (n, b) in files {
            z.start_file(n, opts).unwrap();
            z.write_all(&b).unwrap();
        }
        z.finish().unwrap();
        p
    }
}
fn assert_failed(o: Output) {
    assert!(
        !o.status.success(),
        "unexpected success: {}",
        String::from_utf8_lossy(&o.stdout)
    );
}
#[test]
fn refuses_arbitrary_restore_destinations() {
    let f = Fixture::new();
    let victim = f.root.join("victim");
    f.bundle(json!({"tool":"codex","layout":"codex/rollout-v1","entries":[{"path":"native/rollout-s.jsonl","restore_to":victim}]}),&[("native/rollout-s.jsonl",b"{}\n".to_vec())],false);
    assert_failed(f.run(&["restore", "test.zip"]));
    assert!(!victim.exists());
}

#[test]
fn rejected_gemini_restore_does_not_register_project() {
    let f = Fixture::new();
    let files = vec![
        (
            "trajectory.json",
            br#"{"schema_version":"ATIF-v1.8","steps":[]}"#.to_vec(),
        ),
        ("native/session-s.jsonl", b"{}\n".to_vec()),
    ];
    let manifest = json!({"format":"flightlog","format_version":"1.0","bundle_id":"00000000-0000-4000-8000-000000000001","source":{"tool":"gemini_cli","session_id":"s"},"summary":{"goal":"g","state":"s"},"redaction":{"findings":[]},"native":{"tool":"gemini_cli","layout":"gemini-cli/chats-v1","entries":[{"path":"native/session-s.jsonl","restore_to":"/unexpected"}]},"files":files.iter().map(|(n,b)|json!({"path":n,"bytes":b.len(),"sha256":format!("{:x}",Sha256::digest(b))})).collect::<Vec<_>>()});
    f.pack(manifest, files);
    assert_failed(f.run(&["restore", "test.zip"]));
    assert!(!f.root.join("gemini/.gemini/projects.json").exists());
}
#[test]
fn legitimate_restore_derives_command_and_refuses_overwrite() {
    let f = Fixture::new();
    f.bundle(json!({"tool":"codex","layout":"codex/rollout-v1","resume_command":"echo unsafe-command","entries":[{"path":"native/rollout-s.jsonl","restore_to":"~/.codex/sessions/2026/09/28/rollout-s.jsonl"}]}),&[("native/rollout-s.jsonl",b"{}\n".to_vec())],false);
    let o = f.run(&["restore", "test.zip"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stdout).contains("now run: codex resume s"));
    assert!(!String::from_utf8_lossy(&o.stdout).contains("unsafe-command"));
    assert_failed(f.run(&["restore", "test.zip"]));
    assert!(f.run(&["restore", "test.zip", "--force"]).status.success());
}
#[test]
fn rejects_unlisted_files_and_invalid_inspect() {
    let f = Fixture::new();
    f.bundle(Value::Null, &[("native/extra.txt", b"ok".to_vec())], true);
    assert_failed(f.run(&["validate", "test.zip"]));
    assert_failed(f.run(&["inspect", "test.zip"]));
    assert_failed(f.run(&["extract", "test.zip", "-o", "out"]));
    assert!(!f.root.join("out").exists());
}
#[test]
fn rejects_secrets_in_native_and_plaintext_transport() {
    let f = Fixture::new();
    f.bundle(
        Value::Null,
        &[(
            "native/session.json",
            br#"{"password":"sensitive-value"}"#.to_vec(),
        )],
        false,
    );
    assert_failed(f.run(&["validate", "test.zip"]));
    f.bundle(Value::Null, &[], false);
    let o = f.run(&[
        "push",
        "test.zip",
        "--url",
        "http://127.0.0.1:1/?token=do-not-print",
    ]);
    assert!(!o.status.success());
    assert!(!String::from_utf8_lossy(&o.stderr).contains("do-not-print"));
}
#[test]
fn refuses_existing_extract_directory() {
    let f = Fixture::new();
    f.bundle(Value::Null, &[], false);
    fs::create_dir(f.root.join("out")).unwrap();
    fs::write(f.root.join("out/trajectory.json"), "original").unwrap();
    assert_failed(f.run(&["extract", "test.zip", "-o", "out"]));
    assert_eq!(
        fs::read_to_string(f.root.join("out/trajectory.json")).unwrap(),
        "original"
    );
}
#[cfg(unix)]
#[test]
fn refuses_symlinked_restore_parent_and_dangling_target() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    fs::create_dir_all(f.root.join("codex/sessions")).unwrap();
    fs::create_dir(f.root.join("outside")).unwrap();
    symlink(f.root.join("outside"), f.root.join("codex/sessions/2026")).unwrap();
    f.bundle(json!({"tool":"codex","layout":"codex/rollout-v1","entries":[{"path":"native/rollout-s.jsonl","restore_to":"~/.codex/sessions/2026/09/28/rollout-s.jsonl"}]}),&[("native/rollout-s.jsonl",b"{}\n".to_vec())],false);
    assert_failed(f.run(&["restore", "test.zip", "--force"]));
    assert!(!f.root.join("outside/09").exists());
    fs::remove_file(f.root.join("codex/sessions/2026")).unwrap();
    fs::create_dir_all(f.root.join("codex/sessions/2026/09/28")).unwrap();
    symlink(
        f.root.join("missing-victim"),
        f.root.join("codex/sessions/2026/09/28/rollout-s.jsonl"),
    )
    .unwrap();
    assert_failed(f.run(&["restore", "test.zip", "--force"]));
    assert!(!f.root.join("missing-victim").exists());
}
#[test]
fn rejects_understated_zip_size() {
    let f = Fixture::new();
    let p = f.bundle(
        Value::Null,
        &[("assets/payload", vec![b'x'; 2 * 1024 * 1024])],
        false,
    );
    let mut bytes = fs::read(&p).unwrap();
    let at = bytes
        .windows(4)
        .enumerate()
        .find(|(i, b)| *b == b"PK\x01\x02" && bytes.get(i + 46..i + 60) == Some(b"assets/payload"))
        .unwrap()
        .0;
    let local = u32::from_le_bytes(bytes[at + 42..at + 46].try_into().unwrap()) as usize;
    bytes[at + 24..at + 28].copy_from_slice(&1u32.to_le_bytes());
    bytes[local + 22..local + 26].copy_from_slice(&1u32.to_le_bytes());
    fs::write(p, bytes).unwrap();
    assert_failed(f.run(&["validate", "test.zip"]));
}
fn make_claude(f: &Fixture) -> PathBuf {
    let cwd = f.root.to_string_lossy();
    let slug: String = cwd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let folder = f.root.join("claude/projects").join(slug);
    fs::create_dir_all(&folder).unwrap();
    let rows = [
        json!({"type":"ai-title","title":"private-example","cwd":cwd}),
        json!({"type":"user","cwd":cwd,"message":{"content":"hi"}}),
        json!({"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"t","name":"mcp__private-example__query","input":{"password":"sensitive-value"}}]}}),
    ];
    fs::write(
        folder.join("s.jsonl"),
        rows.iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    folder
}
#[test]
fn export_redacts_native_metadata_and_custom_names_before_writing() {
    let f = Fixture::new();
    let folder = make_claude(&f);
    fs::create_dir(folder.join("s")).unwrap();
    fs::write(folder.join("s/output.log"), "API_KEY=sensitive-value").unwrap();
    fs::write(f.root.join("policy.txt"), "private-example\n").unwrap();
    let o = f.run(&[
        "export",
        "--tool",
        "claude",
        "--session",
        "s",
        "--redact-file",
        "policy.txt",
        "-o",
        "out.zip",
    ]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let mut z = zip::ZipArchive::new(fs::File::open(f.root.join("out.zip")).unwrap()).unwrap();
    for i in 0..z.len() {
        use std::io::Read;
        let mut text = String::new();
        z.by_index(i).unwrap().read_to_string(&mut text).unwrap();
        assert!(
            !text.contains("private-example") && !text.contains("sensitive-value"),
            "{text}"
        );
    }
    let m: Value = serde_json::from_reader(z.by_name("manifest.json").unwrap()).unwrap();
    assert!(m["source"]["cwd"].is_null());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(f.root.join("out.zip"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert_failed(f.run(&[
        "export",
        "--tool",
        "claude",
        "--session",
        "s",
        "-o",
        "out.zip",
    ]));
}

#[test]
fn unredacted_migration_preserves_native_bytes_and_requires_opt_in() {
    use std::io::Read;
    let f = Fixture::new();
    let folder = make_claude(&f);
    let original = fs::read(folder.join("s.jsonl")).unwrap();
    fs::create_dir(folder.join("s")).unwrap();
    let binary = b"\xff\x00private-migration-bytes";
    fs::write(folder.join("s/raw.bin"), binary).unwrap();
    let o = f.run(&[
        "export",
        "--tool",
        "claude",
        "--session",
        "s",
        "--no-redact",
        "-o",
        "raw.zip",
    ]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stderr).contains("DISABLED"));
    let mut z = zip::ZipArchive::new(fs::File::open(f.root.join("raw.zip")).unwrap()).unwrap();
    let m: Value = serde_json::from_reader(z.by_name("manifest.json").unwrap()).unwrap();
    assert_eq!(m["redaction"]["mode"], "none");
    assert_eq!(m["redaction"]["native"], "skipped");
    assert!(m["redaction"]["applied_at"].is_null());
    let mut native = Vec::new();
    z.by_name("native/s.jsonl")
        .unwrap()
        .read_to_end(&mut native)
        .unwrap();
    assert_eq!(native, original);
    for cmd in ["inspect", "validate", "restore"] {
        assert_failed(f.run(&[cmd, "raw.zip"]));
    }
    assert_failed(f.run(&["extract", "raw.zip", "-o", "blocked"]));
    assert!(!f.root.join("blocked").exists());
    assert!(f
        .run(&["validate", "raw.zip", "--allow-unredacted"])
        .status
        .success());
    assert_failed(f.run(&["restore", "raw.zip", "--allow-unredacted"]));
    assert!(f
        .run(&["restore", "raw.zip", "--allow-unredacted", "--force"])
        .status
        .success());
    assert_eq!(fs::read(folder.join("s.jsonl")).unwrap(), original);
    assert_eq!(fs::read(folder.join("s/raw.bin")).unwrap(), binary);
    assert!(f
        .run(&["extract", "raw.zip", "--allow-unredacted", "-o", "out"])
        .status
        .success());
    assert_eq!(
        fs::read(f.root.join("out/native/s.jsonl")).unwrap(),
        original
    );
}

#[test]
fn allow_unredacted_does_not_bypass_normal_bundle_secret_checks() {
    let f = Fixture::new();
    f.bundle(
        Value::Null,
        &[(
            "native/session.json",
            br#"{"password":"sensitive-value"}"#.to_vec(),
        )],
        false,
    );
    assert_failed(f.run(&["validate", "test.zip", "--allow-unredacted"]));
    assert!(f
        .run(&["validate", "test.zip", "--skip-content-checks"])
        .status
        .success());
}

#[test]
fn individual_validation_overrides_are_independent() {
    let f = Fixture::new();
    f.bundle(Value::Null, &[], false);
    f.edit_manifest(|m| m["files"][0]["sha256"] = json!("incorrect"));
    assert_failed(f.run(&["validate", "test.zip"]));
    assert!(f
        .run(&["validate", "test.zip", "--skip-checksums"])
        .status
        .success());
    assert_failed(f.run(&["validate", "test.zip", "--skip-format-checks"]));
    f.bundle(Value::Null, &[], false);
    f.edit_manifest(|m| m["files"][0]["bytes"] = json!(1));
    assert_failed(f.run(&["validate", "test.zip", "--skip-checksums"]));
    assert!(f
        .run(&["validate", "test.zip", "--skip-size-checks"])
        .status
        .success());
    f.bundle(Value::Null, &[], false);
    f.edit_manifest(|m| m["format_version"] = json!("999.0"));
    assert_failed(f.run(&["validate", "test.zip"]));
    assert!(f
        .run(&["validate", "test.zip", "--skip-format-checks"])
        .status
        .success());
    f.edit_manifest(|m| m["bundle_id"] = json!("../escape"));
    assert_failed(f.run(&["validate", "test.zip", "--skip-format-checks"]));
}

#[test]
fn archive_entry_limit_has_an_independent_override() {
    let f = Fixture::new();
    let names: Vec<_> = (0..10_000).map(|i| format!("assets/{i}.txt")).collect();
    let extras: Vec<_> = names
        .iter()
        .map(|name| (name.as_str(), Vec::new()))
        .collect();
    f.bundle(Value::Null, &extras, false);
    assert_failed(f.run(&["validate", "test.zip"]));
    assert_failed(f.run(&["validate", "test.zip", "--skip-format-checks"]));
    let o = f.run(&["validate", "test.zip", "--skip-size-checks"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

#[test]
fn path_override_does_not_disable_restore_format_validation() {
    let f = Fixture::new();
    let victim = f.root.join("destination");
    f.bundle(json!({"tool":"claude_code","layout":"codex/rollout-v1","entries":[{"path":"native/rollout-s.jsonl","restore_to":victim}]}), &[("native/rollout-s.jsonl", b"{}\n".to_vec())], false);
    assert_failed(f.run(&["restore", "test.zip", "--skip-path-checks"]));
    assert!(!victim.exists());
    assert!(f
        .run(&[
            "restore",
            "test.zip",
            "--skip-path-checks",
            "--skip-format-checks"
        ])
        .status
        .success());
}

#[test]
fn format_override_does_not_disable_namespace_containment() {
    let f = Fixture::new();
    f.bundle(
        Value::Null,
        &[("unexpected/config", b"data".to_vec())],
        false,
    );
    assert_failed(f.run(&["validate", "test.zip", "--skip-format-checks"]));
    assert!(f
        .run(&["validate", "test.zip", "--skip-path-checks"])
        .status
        .success());
}

#[test]
fn path_override_is_explicit_and_does_not_imply_overwrite() {
    let f = Fixture::new();
    f.bundle(
        Value::Null,
        &[("native/../../escaped.txt", b"marker".to_vec())],
        false,
    );
    assert_failed(f.run(&["extract", "test.zip", "-o", "out"]));
    assert_failed(f.run(&["extract", "test.zip", "--skip-format-checks", "-o", "out"]));
    let o = f.run(&["extract", "test.zip", "--skip-path-checks", "-o", "out"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(fs::read(f.root.join("escaped.txt")).unwrap(), b"marker");
    assert_failed(f.run(&["extract", "test.zip", "--skip-path-checks", "-o", "out"]));
    assert!(f
        .run(&[
            "extract",
            "test.zip",
            "--skip-path-checks",
            "--overwrite",
            "-o",
            "out"
        ])
        .status
        .success());
}

#[test]
fn yolo_combines_bypasses_and_preserves_exported_secrets() {
    let f = Fixture::new();
    make_claude(&f);
    for _ in 0..2 {
        let o = f.run(&[
            "export",
            "--tool",
            "claude",
            "--session",
            "s",
            "--yolo",
            "-o",
            "out.zip",
        ]);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    assert!(f.run(&["restore", "out.zip", "--yolo"]).status.success());
    let victim = f.root.join("explicit-destination");
    fs::write(&victim, "old").unwrap();
    f.bundle(json!({"tool":"codex","layout":"codex/rollout-v1","entries":[{"path":"native/rollout-s.jsonl","restore_to":victim}]}), &[("native/rollout-s.jsonl", b"{\"password\":\"sensitive-value\"}\n".to_vec())], false);
    f.edit_manifest(|m| {
        m["format_version"] = json!("999");
        m["files"][0]["sha256"] = json!("bad");
        m["files"][0]["bytes"] = json!(0);
    });
    assert_failed(f.run(&["restore", "test.zip"]));
    let o = f.run(&["restore", "test.zip", "--yolo"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        fs::read_to_string(victim).unwrap(),
        "{\"password\":\"sensitive-value\"}\n"
    );
}

#[test]
fn http_requires_its_own_explicit_opt_in() {
    use std::{io::Read, net::TcpListener};
    let f = Fixture::new();
    let bundle = f.bundle(Value::Null, &[], false);
    let bytes = fs::read(bundle).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/bundle", listener.local_addr().unwrap());
    assert_failed(f.run(&["pull", &url, "-o", "received.zip"]));
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut request = [0; 8192];
        let n = stream.read(&mut request).unwrap();
        assert!(n > 0);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        )
        .unwrap();
        stream.write_all(&bytes).unwrap();
    });
    let o = f.run(&["pull", &url, "--allow-http", "-o", "received.zip"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    worker.join().unwrap();
}

#[cfg(unix)]
#[test]
fn symlink_overrides_require_paths_and_overwrite_separately() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let f = Fixture::new();
    f.bundle(Value::Null, &[], false);
    fs::create_dir(f.root.join("out")).unwrap();
    fs::write(f.root.join("victim"), "original").unwrap();
    fs::set_permissions(f.root.join("victim"), fs::Permissions::from_mode(0o644)).unwrap();
    symlink(f.root.join("victim"), f.root.join("out/trajectory.json")).unwrap();
    assert_failed(f.run(&["extract", "test.zip", "-o", "out", "--overwrite"]));
    assert_eq!(
        fs::read_to_string(f.root.join("victim")).unwrap(),
        "original"
    );
    assert!(f
        .run(&[
            "extract",
            "test.zip",
            "-o",
            "out",
            "--skip-path-checks",
            "--overwrite"
        ])
        .status
        .success());
    let trajectory: Value =
        serde_json::from_slice(&fs::read(f.root.join("victim")).unwrap()).unwrap();
    assert_eq!(trajectory["schema_version"], "ATIF-v1.8");
    assert_eq!(
        fs::metadata(f.root.join("victim"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
#[test]
fn unsupported_native_never_leaves_output_and_no_native_still_works() {
    let f = Fixture::new();
    let folder = make_claude(&f);
    fs::create_dir(folder.join("s")).unwrap();
    fs::write(folder.join("s/raw.bin"), [0xff, 0x00]).unwrap();
    assert_failed(f.run(&[
        "export",
        "--tool",
        "claude",
        "--session",
        "s",
        "-o",
        "out.zip",
    ]));
    assert!(!f.root.join("out.zip").exists());
    assert!(f
        .run(&[
            "export",
            "--tool",
            "claude",
            "--session",
            "s",
            "--no-native",
            "-o",
            "out.zip"
        ])
        .status
        .success());
}
#[test]
fn invalid_bundle_id_cannot_escape_work_directory() {
    let f = Fixture::new();
    let p = f.bundle(Value::Null, &[], false);
    let mut z = zip::ZipArchive::new(fs::File::open(p).unwrap()).unwrap();
    let mut m: Value = serde_json::from_reader(z.by_name("manifest.json").unwrap()).unwrap();
    m["bundle_id"] = json!("../../escape");
    let t: Value = serde_json::from_reader(z.by_name("trajectory.json").unwrap()).unwrap();
    drop(z);
    f.pack(
        m,
        vec![("trajectory.json", serde_json::to_vec(&t).unwrap())],
    );
    assert_failed(f.run(&["restore", "test.zip"]));
}
#[test]
fn modern_sqlite_and_bundle_sql_are_not_executed() {
    assert!(rusqlite::version_number() >= 3_053_002);
    let f = Fixture::new();
    let id = "a".repeat(64);
    let d = json!({"format":"flightlog.cursor-store/1","store_meta":{"latestRootBlobId":id},"blobs":[{"id":id,"json":{"role":"user","content":"hello"}}],"schema":["CREATE TABLE proof AS SELECT sqlite_version()"],"meta_json":{}});
    let p = f.bundle(
        json!({"tool":"cursor","layout":"cursor/chats-v1"}),
        &[(
            "native/s.cursor-store.json",
            serde_json::to_vec(&d).unwrap(),
        )],
        false,
    );
    let mut z = zip::ZipArchive::new(fs::File::open(p).unwrap()).unwrap();
    let mut m: Value = serde_json::from_reader(z.by_name("manifest.json").unwrap()).unwrap();
    m["source"]["tool"] = json!("cursor");
    let t: Value = serde_json::from_reader(z.by_name("trajectory.json").unwrap()).unwrap();
    drop(z);
    f.pack(
        m,
        vec![
            ("trajectory.json", serde_json::to_vec(&t).unwrap()),
            (
                "native/s.cursor-store.json",
                serde_json::to_vec(&d).unwrap(),
            ),
        ],
    );
    assert_failed(f.run(&["restore", "test.zip"]));
    assert!(!f.root.join("cursor").exists());
    let o = f.run(&["restore", "test.zip", "--yolo"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let chats = fs::read_dir(f.root.join("cursor/chats")).unwrap();
    let hash_dir = chats.into_iter().next().unwrap().unwrap().path();
    let conn = rusqlite::Connection::open(hash_dir.join("s/store.db")).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='proof'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "bundle SQL must not execute even with overrides");
    let count: i64 = conn
        .query_row("SELECT count(*) FROM blobs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn yolo_keeps_opencode_restore_functional() {
    let f = Fixture::new();
    f.bundle(
        json!({"tool":"opencode","layout":"opencode/export-v1","entries":[]}),
        &[("native/s.json", b"{}".to_vec())],
        false,
    );
    f.edit_manifest(|m| m["source"]["tool"] = json!("opencode"));
    let o = f.run(&["restore", "test.zip", "--yolo"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        fs::read(
            f.root
                .join(".flightlog/00000000-0000-4000-8000-000000000001/s.json")
        )
        .unwrap(),
        b"{}"
    );
}
