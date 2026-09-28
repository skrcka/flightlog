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
}
