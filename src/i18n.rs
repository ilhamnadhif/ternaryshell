//! English UI strings and formatting helpers.

/// Returns the English UI string unchanged.
#[inline]
pub fn t(en: &'static str) -> &'static str {
    en
}

/// Replaces each `{}` with the next argument, in order.
pub fn tf(en: &'static str, args: &[&str]) -> String {
    let mut out = String::new();
    let mut rest = t(en);
    let mut args = args.iter();
    while let Some(pos) = rest.find("{}") {
        out.push_str(&rest[..pos]);
        out.push_str(args.next().copied().unwrap_or(""));
        rest = &rest[pos + 2..];
    }
    out.push_str(rest);
    out
}

/// `tf` with the count as the first `{}`: `one` when it is 1, `many` otherwise.
/// `tn("{} file", "{} files", n, &[])`.
pub fn tn<N>(one: &'static str, many: &'static str, n: N, args: &[&str]) -> String
where
    N: ToString + PartialEq + From<u8>,
{
    let count = n.to_string();
    let mut all: Vec<&str> = Vec::with_capacity(args.len() + 1);
    all.push(&count);
    all.extend_from_slice(args);
    tf(if n == N::from(1) { one } else { many }, &all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_and_plural() {
        assert_eq!(t("Connect"), "Connect");
        assert_eq!(tf("{} of {}", &["1", "2"]), "1 of 2");
        assert_eq!(tn("{} file", "{} files", 1usize, &[]), "1 file");
        assert_eq!(tn("{} file", "{} files", 3u64, &[]), "3 files");
    }
}
