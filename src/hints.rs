//! Hint labels (qutebrowser browser/hints.py `_hint_scattered`).

/// Shortest labels first, spread over the first char.
pub fn scattered(n: usize, chars: &str, min_chars: usize) -> Vec<String> {
    let chars: Vec<char> = chars.chars().collect();
    // fewer than 2 chars can't make distinct labels (CEF audit A1: endless loop)
    if chars.len() < 2 || n == 0 {
        return Vec::new();
    }
    let base = chars.len();
    let mut needed = 1; // ceil_log(n, base), at least 1
    let mut accum = base;
    while accum < n {
        needed += 1;
        accum *= base;
    }
    let needed = needed.max(min_chars);
    let short = if needed > min_chars && needed > 1 {
        (base.pow(needed as u32) - n) / (base - 1)
    } else {
        0
    };
    let mut out: Vec<String> = Vec::with_capacity(n);
    if needed > 1 {
        out.extend((0..short).map(|i| to_label(i, &chars, needed - 1)));
    }
    let start = short * base;
    out.extend((start..start + n - short).map(|i| to_label(i, &chars, needed)));
    // _shuffle_hints
    let mut buckets = vec![Vec::new(); base];
    for (i, l) in out.into_iter().enumerate() {
        buckets[i % base].push(l);
    }
    buckets.concat()
}

fn to_label(mut n: usize, chars: &[char], digits: usize) -> String {
    let base = chars.len();
    let mut s = Vec::new();
    loop {
        s.insert(0, chars[n % base]);
        n /= base;
        if n == 0 {
            break;
        }
    }
    while s.len() < digits {
        s.insert(0, chars[0]);
    }
    s.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scattered_matches_qutebrowser() {
        assert_eq!(scattered(3, "asdf", 1), ["a", "s", "d"]);
        assert_eq!(scattered(5, "abc", 1), ["a", "cb", "b", "cc", "ca"]);
        assert_eq!(
            scattered(12, "asdfghjkl", 1),
            ["a", "ls", "s", "ld", "d", "lf", "f", "g", "h", "j", "k", "la"]
        );
        let many = scattered(100, "asdfghjkl", 1);
        assert_eq!(many.len(), 100);
        for (i, a) in many.iter().enumerate() {
            for (j, b) in many.iter().enumerate() {
                assert!(i == j || !b.starts_with(a.as_str()), "{a} prefixes {b}");
            }
        }
        assert_eq!(scattered(2, "asdf", 2), ["aa", "as"]);
    }

    #[test]
    fn one_char_alphabet_does_not_hang() {
        assert!(scattered(5, "a", 1).is_empty());
        assert!(scattered(5, "", 1).is_empty());
    }
}
