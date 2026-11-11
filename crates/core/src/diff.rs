//! Bounded review excerpts using a correct whole changed-region replacement hunk.
#[derive(Debug)]
pub struct Diff {
    pub left: String,
    pub right: String,
    pub inline: String,
}
fn line(out: &mut String, prefix: &str, text: &str) -> bool {
    if out.len() > 250_000 {
        return false;
    }
    out.push_str(prefix);
    for c in text.chars().take(4096) {
        out.push(c);
    }
    out.push('\n');
    true
}
pub fn compare(before: &str, after: &str) -> Diff {
    let a: Vec<_> = before.split_inclusive('\n').collect();
    let b: Vec<_> = after.split_inclusive('\n').collect();
    let mut start = 0;
    while start < a.len().min(b.len()) && a[start] == b[start] {
        start += 1;
    }
    let mut suffix = 0;
    while suffix < a.len() - start
        && suffix < b.len() - start
        && a[a.len() - suffix - 1] == b[b.len() - suffix - 1]
    {
        suffix += 1;
    }
    let mut result = Diff {
        left: String::new(),
        right: String::new(),
        inline: String::new(),
    };
    if a == b {
        result.inline = "No unsaved changes.\n".into();
        return result;
    }
    for text in a[start.saturating_sub(3)..start].iter() {
        line(&mut result.left, "  ", text);
        line(&mut result.right, "  ", text);
        line(&mut result.inline, "  ", text);
    }
    for text in &a[start..a.len() - suffix] {
        if !line(&mut result.left, "- ", text) || !line(&mut result.inline, "- ", text) {
            break;
        }
    }
    for text in &b[start..b.len() - suffix] {
        if !line(&mut result.right, "+ ", text) || !line(&mut result.inline, "+ ", text) {
            break;
        }
    }
    for text in a[a.len() - suffix..].iter().take(3) {
        line(&mut result.left, "  ", text);
        line(&mut result.right, "  ", text);
        line(&mut result.inline, "  ", text);
    }
    if result.left.len() > 250_000 || result.right.len() > 250_000 || result.inline.len() > 250_000
    {
        result
            .inline
            .push_str("\n[Review excerpt truncated; inspect full source before accepting.]\n");
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacements_and_newline_changes_are_visible() {
        let d = compare("a\nb\n", "a\nc\n");
        assert!(d.inline.contains("- b\n"));
        assert!(d.inline.contains("+ c\n"));
        let d = compare("same", "same\n");
        assert!(d.inline.contains("- same"));
        assert!(d.inline.contains("+ same"));
    }
}
