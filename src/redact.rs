//! Secret redaction (spec §4): each distinct secret becomes a stable
//! placeholder `[REDACTED:<kind>:<n>]`; the report never holds the value.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use regex::{Captures, Regex};
use serde_json::{json, Value};

struct Rule {
    kind: &'static str,
    re: Regex,
    /// Capture group holding the secret (0 = whole match).
    group: usize,
}

fn rules() -> &'static [Rule] {
    static R: OnceLock<Vec<Rule>> = OnceLock::new();
    R.get_or_init(|| {
        let r = |kind, re: &str, group| Rule {
            kind,
            re: Regex::new(re).expect("rule"),
            group,
        };
        vec![
            r(
                "private_key",
                r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |ENCRYPTED |PGP )?PRIVATE KEY(?: BLOCK)?-----[\s\S]*?-----END (?:RSA |EC |DSA |OPENSSH |ENCRYPTED |PGP )?PRIVATE KEY(?: BLOCK)?-----",
                0,
            ),
            r(
                "private_key",
                r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |ENCRYPTED |PGP )?PRIVATE KEY(?: BLOCK)?-----[\s\S]*$",
                0,
            ),
            r("password", r#"\bhttps?://([^\s/@"'\\]+)@"#, 1),
            // Non-HTTP connection strings: only the password part.
            r(
                "password",
                r#"\b[a-z][a-z0-9+.\-]{1,20}://[^\s/:@"'\\]{1,64}:([^\s@"'\\\[]{1,256})@"#,
                1,
            ),
            r("api_key", r"\bAKIA[0-9A-Z]{16}\b", 0),
            r("api_key", r"\bsk-(?:ant-|proj-)?[A-Za-z0-9_\-]{32,}", 0),
            r("api_key", r"\bsk_live_[0-9A-Za-z]{24,}\b", 0),
            r("api_key", r"\bAIza[0-9A-Za-z_\-]{35}\b", 0),
            r("token", r"\bgh[pousr]_[A-Za-z0-9]{36,}\b", 0),
            r("token", r"\bgithub_pat_[A-Za-z0-9_]{60,}\b", 0),
            r("token", r"\bglpat-[A-Za-z0-9_\-]{20,}\b", 0),
            r("token", r"\bxox[abprs]-[A-Za-z0-9\-]{10,}", 0),
            r("token", r"\bhf_[A-Za-z0-9]{30,}\b", 0),
            r("token", r"\bnpm_[A-Za-z0-9]{36}\b", 0),
            r("token", r"\b(?:mcp|api)_[0-9a-f]{32,}\b", 0),
            r(
                "token",
                r"\beyJ[A-Za-z0-9_\-]{10,}\.eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}",
                0,
            ),
            r("token", r"(?i)\bbearer\s+([A-Za-z0-9._\-]{24,})", 1),
            r("token", r#"(?i)[?&](?:token|access_token|api_key|x-amz-signature|x-goog-signature|sig)=([^&\s"'#\[]+)"#, 1),
            // .env style: FOO_SECRET=value, API_KEY=value, DB_PASSWORD=value
            r(
                "password",
                r#"(?m)\b[A-Z0-9_]*(?:SECRET|PASSWORD|PASSWD|TOKEN|API_KEY|PRIVATE_KEY)[A-Z0-9_]*\s*=\s*["']?([^\s"'\[]{1,})"#,
                1,
            ),
        ]
    })
}

/// A redaction finding (spec §4 `manifest.redaction.findings[]`).
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub placeholder: String,
    pub kind: &'static str,
    pub count: usize,
    pub files: BTreeSet<String>,
}

#[derive(Default)]
pub struct Redactor {
    disabled: bool,
    literals: Vec<String>,
    values: HashMap<String, String>,
    per_kind: HashMap<&'static str, usize>,
    findings: BTreeMap<String, Finding>,
}

impl Redactor {
    pub fn disabled() -> Self {
        Self {
            disabled: true,
            ..Self::default()
        }
    }
    pub fn configured() -> anyhow::Result<Self> {
        let mut red = Self::default();
        if let Some(path) = std::env::var_os("FLIGHTLOG_REDACT_FILE") {
            let bytes = crate::safe_fs::read(std::path::Path::new(&path), 1024 * 1024)?;
            let text = std::str::from_utf8(&bytes)?;
            red.literals = text
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.starts_with('#'))
                .map(str::to_string)
                .collect();
            red.literals.sort_by_key(|s| std::cmp::Reverse(s.len()));
            if red.literals.len() > 1000 || red.literals.iter().any(|s| s.len() < 3) {
                anyhow::bail!(
                    "privacy policy allows at most 1000 literal terms, each at least 3 bytes"
                );
            }
        }
        Ok(red)
    }
    /// Parse native JSON/JSONL so escaping cannot hide fields from the redactor.
    pub fn native(&mut self, bytes: &[u8], file: &str) -> anyhow::Result<Vec<u8>> {
        if self.disabled {
            return Ok(bytes.to_vec());
        }
        let text = std::str::from_utf8(bytes).map_err(|_| {
            anyhow::anyhow!("unsupported binary native content; export with --no-native")
        })?;
        if file.ends_with(".json") {
            let v: Value = serde_json::from_str(text)?;
            if file.ends_with(".cursor-store.json") {
                crate::sources::cursor::validate_dump(&v)?;
            }
            let clean = self.value(&v, file);
            reject_opaque(&clean)?;
            return Ok(serde_json::to_vec(&clean)?);
        }
        if file.ends_with(".jsonl") {
            let mut out = String::new();
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                let v: Value = serde_json::from_str(line)?;
                let clean = self.value(&v, file);
                reject_opaque(&clean)?;
                out.push_str(&serde_json::to_string(&clean)?);
                out.push('\n');
            }
            return Ok(out.into_bytes());
        }
        Ok(self.text(text, file).into_bytes())
    }

    fn placeholder(&mut self, kind: &'static str, secret: &str) -> String {
        if let Some(p) = self.values.get(secret) {
            return p.clone();
        }
        let n = self.per_kind.entry(kind).or_default();
        *n += 1;
        let p = format!("[REDACTED:{kind}:{n}]");
        self.values.insert(secret.to_string(), p.clone());
        p
    }

    fn hit(&mut self, placeholder: &str, kind: &'static str, file: &str) {
        let f = self
            .findings
            .entry(placeholder.to_string())
            .or_insert_with(|| Finding {
                placeholder: placeholder.to_string(),
                kind,
                count: 0,
                files: BTreeSet::new(),
            });
        f.count += 1;
        f.files.insert(file.to_string());
    }

    /// Redact one string; `file` names where it lives, for the report.
    pub fn text(&mut self, s: &str, file: &str) -> String {
        // JSON embedded inside a string is common in tool arguments/results.
        if (s.trim_start().starts_with('{') || s.trim_start().starts_with('['))
            && s.len() < 16 * 1024 * 1024
        {
            if let Ok(v @ (Value::Object(_) | Value::Array(_))) = serde_json::from_str::<Value>(s) {
                let redacted = self.value(&v, file);
                if redacted != v {
                    return redacted.to_string();
                }
            }
        }
        let mut out = s.to_string();
        for literal in self.literals.clone() {
            // Case-insensitive literal matching covers hostnames/tool aliases.
            let re =
                Regex::new(&format!("(?i){}", regex::escape(&literal))).expect("escaped literal");
            // A private term such as "key" must not recursively redact a placeholder.
            let placeholders: Vec<_> = placeholder_pattern()
                .find_iter(&out)
                .map(|m| m.range())
                .collect();
            out = re
                .replace_all(&out, |c: &Captures| {
                    let matched = c.get(0).unwrap();
                    if placeholders
                        .iter()
                        .any(|r| r.start <= matched.start() && matched.end() <= r.end)
                    {
                        return matched.as_str().to_string();
                    }
                    let ph = self.placeholder("custom", &c[0]);
                    self.hit(&ph, "custom", file);
                    ph
                })
                .into_owned();
        }
        for rule in rules() {
            if !rule.re.is_match(&out) {
                continue;
            }
            let mut hits: Vec<(String, String)> = Vec::new();
            let replaced = rule
                .re
                .replace_all(&out, |c: &Captures| {
                    let whole = c.get(0).map(|m| m.as_str()).unwrap_or("");
                    let Some(secret) = c.get(rule.group) else {
                        return whole.to_string();
                    };
                    if is_placeholder(secret.as_str()) {
                        return whole.to_string();
                    }
                    let ph = self.placeholder(rule.kind, secret.as_str());
                    hits.push((ph.clone(), secret.as_str().to_string()));
                    // Replace only the secret inside the match.
                    let start = secret.start() - c.get(0).unwrap().start();
                    let end = start + secret.as_str().len();
                    format!("{}{}{}", &whole[..start], ph, &whole[end..])
                })
                .into_owned();
            for (ph, _) in hits {
                self.hit(&ph, rule.kind, file);
            }
            out = replaced;
        }
        out
    }

    /// Redact every string in a JSON value.
    pub fn value(&mut self, v: &Value, file: &str) -> Value {
        if self.disabled {
            return v.clone();
        }
        match v {
            Value::String(s) => Value::String(self.text(s, file)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.value(x, file)).collect()),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, x)| {
                        let key = self.text(k, file);
                        let sensitive = sensitive_key(k);
                        let value = if sensitive
                            && !x.is_null()
                            && !x
                                .as_str()
                                .is_some_and(|s| s.is_empty() || is_placeholder(s))
                        {
                            let secret = x
                                .as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| x.to_string());
                            let ph = self.placeholder("password", &secret);
                            self.hit(&ph, "password", file);
                            Value::String(ph)
                        } else {
                            self.value(x, file)
                        };
                        (key, value)
                    })
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    pub fn findings(&self) -> Vec<Finding> {
        self.findings.values().cloned().collect()
    }

    pub fn findings_json(&self) -> Value {
        Value::Array(
            self.findings
                .values()
                .map(|f| {
                    json!({ "placeholder": f.placeholder, "kind": f.kind, "count": f.count,
                            "files": f.files.iter().collect::<Vec<_>>() })
                })
                .collect(),
        )
    }
}

fn reject_opaque(v: &Value) -> anyhow::Result<()> {
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                if matches!(k.as_str(), "encrypted_content" | "inlineData" | "image_url")
                    && !x.is_null()
                {
                    anyhow::bail!("encoded native content cannot be redacted (field {k}); use --no-native, then restore with --to TOOL for shared-history conversion");
                }
                reject_opaque(x)?;
            }
        }
        Value::Array(a) => {
            for x in a {
                reject_opaque(x)?;
            }
        }
        Value::String(s) if s.starts_with("data:") => {
            anyhow::bail!("encoded native content cannot be redacted (data URI); use --no-native, then restore with --to TOOL for shared-history conversion")
        }
        _ => {}
    }
    Ok(())
}

fn is_placeholder(s: &str) -> bool {
    placeholder_pattern()
        .find(s)
        .is_some_and(|m| m.start() == 0 && m.end() == s.len())
}

fn placeholder_pattern() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\[REDACTED:[a-z_]+:[0-9]+\]").unwrap())
}

fn sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace(['-', '_', ' '], "");
    matches!(
        key.as_str(),
        "password"
            | "passwd"
            | "secret"
            | "token"
            | "apikey"
            | "accesstoken"
            | "refreshtoken"
            | "clientsecret"
            | "privatekey"
            | "authorization"
            | "cookie"
            | "setcookie"
    ) || key.ends_with("password")
        || key.ends_with("apikey")
        || key.ends_with("secretkey")
}

/// Kinds and counts of secrets still present in `text` (for validation).
#[cfg(test)]
pub fn scan(text: &str) -> Vec<(&'static str, usize)> {
    let mut out: BTreeMap<&'static str, usize> = BTreeMap::new();
    for rule in rules() {
        for c in rule.re.captures_iter(text) {
            if let Some(m) = c.get(rule.group) {
                if !m.as_str().starts_with("[REDACTED:") {
                    *out.entry(rule.kind).or_default() += 1;
                }
            }
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privacy_policy_and_credentials_are_idempotent() {
        let mut r = Redactor {
            literals: vec!["key".into(), "custom".into()],
            ..Default::default()
        };
        let out = r.text(
            "private key custom API_TOKEN=x https://example.test/?X-Amz-Signature=short",
            "t",
        );
        assert!(!out.contains("=x"));
        assert!(!out.contains("=short"));
        assert_eq!(r.text(&out, "t"), out);
        let fake = json!({"password": "[REDACTED:password:1]still-secret"});
        assert!(!r.value(&fake, "t").to_string().contains("still-secret"));
    }

    #[test]
    fn same_secret_same_placeholder_and_only_the_secret_is_replaced() {
        let mut r = Redactor::default();
        let a = r.text(
            "db postgres://app:hunter22@db:5432/x and again postgres://app:hunter22@db/y",
            "t",
        );
        assert_eq!(
            a,
            "db postgres://app:[REDACTED:password:1]@db:5432/x and again postgres://app:[REDACTED:password:1]@db/y"
        );
        let b = r.text("other postgres://u:s3cr3t99@h/z", "u");
        assert!(b.contains("[REDACTED:password:2]"));
        let f = r.findings();
        assert_eq!(f[0].count, 2);
        assert_eq!(f.len(), 2);
    }

    #[test]
    fn recognizes_common_shapes() {
        let mut r = Redactor::default();
        let s = r.text(
            "AKIAABCDEFGHIJKLMNOP ghp_abcdefghijklmnopqrstuvwxyz0123456789 \
             -----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY----- \
             Authorization: Bearer abcdefghijklmnopqrstuvwxyz12 \
             STRIPE_SECRET_KEY=abcdefgh12345 xoxb-1234567890-abc",
            "t",
        );
        assert!(!s.contains("AKIA"), "{s}");
        assert!(!s.contains("ghp_"), "{s}");
        assert!(!s.contains("abc\n-----END"), "{s}");
        assert!(s.contains("Bearer [REDACTED:token:"), "{s}");
        assert!(s.contains("STRIPE_SECRET_KEY=[REDACTED:password:"), "{s}");
        assert!(scan(&s).is_empty(), "{:?}", scan(&s));
    }

    #[test]
    fn leaves_ordinary_code_alone() {
        let mut r = Redactor::default();
        let code = "let token: String = get(); password.len() > 8; https://example.com/a:b@c";
        let out = r.text(code, "t");
        assert!(
            r.findings().is_empty() || !out.contains("let token: [REDACTED"),
            "{out}"
        );
        assert!(out.starts_with("let token: String"));
    }
}
