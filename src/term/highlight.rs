//! Minimal, calm client-side highlighting for plain (unstyled) terminal lines.
//!
//! Only highlights three high-signal elements on default-styled text (never overriding ANSI
//! colors sent by the server, and skipped completely in alternate screen mode such as vim,
//! htop, or tmux):
//! - Plain shell prompts (`user@host:path$` / `[user@host path]$`): `user@host` in green
//!   (red for `root`), path in blue.
//! - Web URLs (`https://…`, `http://…`): blue.
//! - Critical status / log keywords (`ERROR`/`FAILED` red, `WARN` amber, `OK`/`ONLINE` green).

use eframe::egui::Color32;

use super::scheme::Scheme;

/// Computes optional foreground overrides for one visible terminal row.
///
/// `bytes` holds one ASCII byte per column (non-ASCII cells passed as `b' '`), and `plain`
/// indicates whether each column uses default foreground/background with no special flags.
/// Returns `None` when the row has no highlightable tokens.
pub fn highlight_row(
    bytes: &[u8],
    plain: &[bool],
    scheme: &Scheme,
) -> Option<Vec<Option<Color32>>> {
    let len = bytes.len().min(plain.len());
    if len == 0 || !plain[..len].iter().any(|&p| p) {
        return None;
    }
    let end = bytes[..len]
        .iter()
        .rposition(|&b| b != b' ' && b != 0)
        .map_or(0, |i| i + 1);
    if end == 0 {
        return None;
    }

    let mut out: Vec<Option<Color32>> = vec![None; len];
    let mut any = false;

    let red = scheme.ansi[1];
    let green = scheme.ansi[2];
    let amber = scheme.ansi[3];
    let blue = scheme.ansi[4];

    let mut paint = |range: std::ops::Range<usize>, color: Color32| {
        if range.start >= range.end || range.end > end {
            return;
        }
        if !plain[range.clone()].iter().all(|&p| p) {
            return;
        }
        if out[range.clone()].iter().any(|c| c.is_some()) {
            return;
        }
        for slot in &mut out[range] {
            *slot = Some(color);
        }
        any = true;
    };

    // 1. Plain shell prompt at column 0 (`ubuntu@host:~/dir$` or `[root@host dir]#`).
    let mut scan_from = 0;
    if let Some(p) = match_prompt(&bytes[..end]) {
        paint(p.user_host, if p.is_root { red } else { green });
        paint(p.path, blue);
        scan_from = p.after;
    }

    // 2. Scan the rest of the row for URLs and critical status/log words only.
    let mut i = scan_from;
    while i < end {
        let b = bytes[i];
        if b == b' ' || b == b'\t' {
            i += 1;
            continue;
        }

        if let Some(url_end) = match_url(&bytes[..end], i) {
            paint(i..url_end, blue);
            i = url_end;
            continue;
        }

        if b.is_ascii_alphabetic() && is_word_open_boundary(bytes, i) {
            let mut j = i + 1;
            while j < end && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if is_word_close_boundary(&bytes[..end], j)
                && let Some(color) = keyword_color(&bytes[i..j], red, amber, green)
            {
                paint(i..j, color);
            }
            i = j;
            continue;
        }

        i += 1;
    }

    any.then_some(out)
}

struct PromptMatch {
    user_host: std::ops::Range<usize>,
    path: std::ops::Range<usize>,
    is_root: bool,
    after: usize,
}

fn match_prompt(row: &[u8]) -> Option<PromptMatch> {
    if row.is_empty() {
        return None;
    }
    // Style B: `[user@host path]$` or `[user@host path]#`
    if row[0] == b'[' {
        let close = row.iter().position(|&b| b == b']')?;
        let sigil_pos = close + 1;
        if sigil_pos >= row.len() || !matches!(row[sigil_pos], b'$' | b'#') {
            return None;
        }
        let after = sigil_pos + 1;
        if after < row.len() && row[after] != b' ' {
            return None;
        }
        let inner = &row[1..close];
        let space = inner.iter().position(|&b| b == b' ')?;
        let uh = &inner[..space];
        let at = uh.iter().position(|&b| b == b'@')?;
        let user = &uh[..at];
        let host = &uh[at + 1..];
        if !valid_ident(user) || !valid_host(host) || space + 1 >= inner.len() {
            return None;
        }
        return Some(PromptMatch {
            user_host: 1..1 + space,
            path: 1 + space + 1..close,
            is_root: user == b"root" || row[sigil_pos] == b'#',
            after,
        });
    }

    // Style A: `user@host:path$` or `user@host:path#`
    let limit = row.len().min(96);
    let at = row[..limit].iter().position(|&b| b == b'@')?;
    let user = &row[..at];
    if !valid_ident(user) {
        return None;
    }
    let colon = row[at + 1..limit]
        .iter()
        .position(|&b| b == b':')
        .map(|o| at + 1 + o)?;
    let host = &row[at + 1..colon];
    if !valid_host(host) {
        return None;
    }
    let path_start = colon + 1;
    let mut j = path_start;
    while j < row.len() && !matches!(row[j], b'$' | b'#' | b' ') {
        j += 1;
    }
    if j == path_start || j >= row.len() || !matches!(row[j], b'$' | b'#') {
        return None;
    }
    let after = j + 1;
    if after < row.len() && row[after] != b' ' {
        return None;
    }
    Some(PromptMatch {
        user_host: 0..colon,
        path: path_start..j,
        is_root: user == b"root" || row[j] == b'#',
        after,
    })
}

fn valid_ident(s: &[u8]) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.iter()
            .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

fn valid_host(s: &[u8]) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.iter()
            .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'))
}

fn match_url(bytes: &[u8], i: usize) -> Option<usize> {
    const SCHEMES: &[&[u8]] = &[b"https://", b"http://"];
    let rest = &bytes[i..];
    let scheme = SCHEMES.iter().find(|s| rest.starts_with(s))?;
    let mut j = i + scheme.len();
    while j < bytes.len()
        && !bytes[j].is_ascii_whitespace()
        && !matches!(bytes[j], b')' | b']' | b'}' | b'>' | b'"' | b'\'' | b',')
    {
        j += 1;
    }
    while j > i + scheme.len() && matches!(bytes[j - 1], b'.' | b';' | b':') {
        j -= 1;
    }
    (j > i + scheme.len()).then_some(j)
}

fn is_word_open_boundary(bytes: &[u8], i: usize) -> bool {
    i == 0
        || !(bytes[i - 1].is_ascii_alphanumeric()
            || matches!(bytes[i - 1], b'_' | b'-' | b'.' | b'/'))
}

fn is_word_close_boundary(bytes: &[u8], j: usize) -> bool {
    j == bytes.len()
        || !(bytes[j].is_ascii_alphanumeric() || matches!(bytes[j], b'_' | b'-' | b'/' | b'.'))
}

fn keyword_color(word: &[u8], red: Color32, amber: Color32, green: Color32) -> Option<Color32> {
    if word.len() < 2 || word.len() > 12 {
        return None;
    }
    let mut buf = [0u8; 12];
    for (dst, &src) in buf.iter_mut().zip(word.iter()) {
        *dst = src.to_ascii_lowercase();
    }
    match &buf[..word.len()] {
        b"error" | b"err" | b"fatal" | b"failed" | b"fail" | b"critical" | b"panic" | b"denied"
        | b"refused" | b"stopped" | b"offline" | b"dead" => Some(red),
        b"warn" | b"warning" | b"timeout" => Some(amber),
        b"ok" | b"success" | b"active" | b"running" | b"online" | b"connected" | b"listening" => {
            Some(green)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::scheme::find;

    fn run(line: &str) -> Vec<Option<Color32>> {
        let s = find("TernaryShell Dark", true);
        let bytes = line.as_bytes();
        let plain = vec![true; bytes.len()];
        highlight_row(bytes, &plain, s).unwrap_or_else(|| vec![None; bytes.len()])
    }

    #[test]
    fn highlights_prompt_urls_and_status_only() {
        let s = find("TernaryShell Dark", true);
        // Plain prose, versions, timestamps, IPs and quoted text stay neutral.
        let motd = run("Welcome to Ubuntu 16.04.4 LTS (GNU/Linux 4.4.0-1128-aws x86_64)");
        assert!(motd.iter().all(|c| c.is_none()));
        let login = run("Last login: Wed Nov 25 12:54:31 2026 from 18.136.95.165");
        assert!(login.iter().all(|c| c.is_none()));
        let quote = run("New release '18.04.6 LTS' available.");
        assert!(quote.iter().all(|c| c.is_none()));

        // URLs are blue.
        let link = run("  * Documentation:  https://help.ubuntu.com");
        assert_eq!(link[4], None);
        assert_eq!(link[20], Some(s.ansi[4]));

        // Plain bash prompts get user@host green (red for root) and path blue; command stays neutral.
        let prompt = run("ubuntu@RnD-01:~/linkit-go$ cd '/home/ubuntu/linkit-go'");
        assert_eq!(prompt[0], Some(s.ansi[2]));
        assert_eq!(prompt[14], Some(s.ansi[4]));
        assert_eq!(prompt[25], None);
        assert_eq!(prompt[30], None);

        let root_prompt = run("root@prod:/etc/nginx# systemctl status");
        assert_eq!(root_prompt[0], Some(s.ansi[1]));
        assert_eq!(root_prompt[10], Some(s.ansi[4]));
    }

    #[test]
    fn respects_already_styled_cells() {
        let s = find("TernaryShell Dark", true);
        let line = b"ERROR: service failed, retrying WARN ok";
        let mut plain = vec![true; line.len()];
        for slot in &mut plain[0..5] {
            *slot = false;
        }
        let out = highlight_row(line, &plain, s).unwrap();
        assert_eq!(out[0], None);
        assert_eq!(out[15], Some(s.ansi[1]));
        assert_eq!(out[32], Some(s.ansi[3]));
        assert_eq!(out[37], Some(s.ansi[2]));
    }
}
