//! Deliberately conservative, deterministic exclusions. This is not a universal DLP system.
/// No input is included in errors. Scan strings before serialization/persistence.
pub fn sensitive(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("private key-----") || lower.contains("-----begin pgp private") {
        return true;
    }
    // Provider prefixes. Distinctive ones count anywhere; short ones that also occur
    // inside ordinary words ("risk-matrix", "task-board", "kakiage") are ignored only
    // right after an ASCII letter that is not part of an escape (`\\n`, `%20`).
    // False positives are preferable to saving a known token form.
    if ["ghp_", "gho_", "github_pat_", "xoxb-", "xoxp-", "bearer "]
        .iter()
        .any(|prefix| lower.contains(prefix))
    {
        return true;
    }
    if ["sk-", "akia", "aiza"].iter().any(|prefix| {
        lower
            .match_indices(prefix)
            .any(|(at, _)| !inside_word(&lower.as_bytes()[..at]))
    }) {
        return true;
    }
    // Covers prose, JSON, .env and URL query assignments, including quoted keys.
    for key in [
        "password",
        "passwd",
        "contraseña",
        "contrasena",
        "api_key",
        "api-key",
        "apikey",
        "access_token",
        "refresh_token",
        "token",
        "secret",
        "authorization",
    ] {
        for (offset, _) in lower.match_indices(key) {
            let rest = &lower[offset + key.len()..];
            let rest =
                rest.trim_start_matches(|c: char| c.is_whitespace() || c == '\'' || c == '"');
            if rest.starts_with('=') || rest.starts_with(':') {
                return true;
            }
        }
    }
    // A JWT-shaped value: three substantial base64url segments, starting with JSON header.
    lower
        .split(|c: char| !(c.is_ascii_alphanumeric() || "-_ .".contains(c)))
        .flat_map(str::split_whitespace)
        .any(|word| {
            let parts: Vec<_> = word.split('.').collect();
            parts.len() == 3 && parts[0].starts_with("eyj") && parts.iter().all(|p| p.len() >= 8)
        })
}
/// Whether the text before a match ends in a letter of an ordinary word: an ASCII
/// letter that is not the code of an escape (`\\n`) or of percent-encoding (`%2f`).
fn inside_word(before: &[u8]) -> bool {
    let n = before.len();
    if n == 0 || !before[n - 1].is_ascii_alphabetic() {
        return false;
    }
    let escaped = n >= 2 && before[n - 2] == b'\\';
    let percent = n >= 3 && before[n - 3] == b'%' && before[n - 2].is_ascii_hexdigit();
    !(escaped || percent)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn excludes_supported_secret_forms() {
        for value in [
            "-----BEGIN OPENSSH PRIVATE KEY-----\nfake",
            "password=fictional",
            "TOKEN : abc",
            "\"api_key\": \"fake\"",
            "Authorization: Bearer fictitious",
            "ghp_fictional",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJmYWtlIn0.c2lnbmF0dXJl",
        ] {
            assert!(sensitive(value), "fixture not rejected");
        }
    }
    #[test]
    fn permits_access_references_and_ordinary_decisions() {
        for value in [
            "ssh server1",
            "Use ssh-agent for authentication",
            "SQLite FTS5",
            "No guardar credenciales",
            "El token permanece en un gestor externo",
            "Canvas compartido: https://claude.ai/artifact/0c9f2d1e risk-matrix",
            "task-board del equipo",
            "kakiage y paiza",
        ] {
            assert!(!sensitive(value), "{value}");
        }
    }
    #[test]
    fn provider_prefixes_count_at_word_start() {
        for value in [
            "sk-fake",
            "clave sk-proj-fictional",
            "(ghp_fictional)",
            "url?x=github_pat_fictional",
            "AKIAEXAMPLE",
            "xoxb-fictional",
            "log: \\nsk-proj-fictional",
            "https://x/?r=a%20sk-proj-fictional",
            "https://x/?r=%2fsk-proj-fictional",
            "abcghp_fictional",
            "tokenxoxp-fictional",
            "密钥sk-proj-fictional",
            "ásk-proj-fictional",
            "1sk-proj-fictional",
        ] {
            assert!(sensitive(value), "{value}");
        }
    }
}
