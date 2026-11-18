//! Fuzzy bookmark search over name, host, user, tags and group path.

use uuid::Uuid;

use super::model::Bookmark;
use super::store::BookmarkStore;

/// Scores `pattern` as a case-insensitive subsequence of `text`. Higher is better; `None` when
/// the characters don't all appear in order. Consecutive runs and matches at word starts score
/// higher, so "pdb" ranks "prod-db" above "upload-backup".
pub fn fuzzy_score(pattern: &str, text: &str) -> Option<i32> {
    let p: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
    if p.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    if p.len() > t.len() {
        return None;
    }
    // best[j]: best score with the current pattern char matched at text position j.
    let mut best: Vec<Option<i32>> = (0..t.len())
        .map(|j| (t[j] == p[0]).then(|| 16 + boundary_bonus(&t, j) - (j as i32).min(10)))
        .collect();
    for &pc in &p[1..] {
        let mut next = vec![None; t.len()];
        for j in 0..t.len() {
            if t[j] != pc {
                continue;
            }
            let here = 16 + boundary_bonus(&t, j);
            next[j] = (0..j)
                .filter_map(|k| {
                    best[k].map(|s| {
                        let link = if k + 1 == j {
                            12
                        } else {
                            -((j - k - 1) as i32).min(8)
                        };
                        s + here + link
                    })
                })
                .max();
        }
        best = next;
    }
    // Prefer shorter texts when everything else is equal.
    best.into_iter()
        .flatten()
        .max()
        .map(|s| s - t.len() as i32 / 8)
}

fn boundary_bonus(t: &[char], i: usize) -> i32 {
    if i == 0 || matches!(t[i - 1], ' ' | '-' | '_' | '.' | '@' | '/' | ':') {
        10
    } else {
        0
    }
}

/// Best score of one search term against a bookmark's fields; the name counts most.
fn term_score(term: &str, b: &Bookmark, group_path: &str, label: &str) -> Option<i32> {
    let fields: [(&str, i32); 5] = [
        (&b.name, 3),
        (&b.host, 2),
        (&b.username, 1),
        (group_path, 1),
        (label, 1),
    ];
    let mut best: Option<i32> = None;
    for (text, weight) in fields {
        if let Some(s) = fuzzy_score(term, text) {
            best = Some(best.map_or(s * weight, |x| x.max(s * weight)));
        }
    }
    for tag in &b.tags {
        if let Some(s) = fuzzy_score(term, tag) {
            best = Some(best.map_or(s * 2, |x| x.max(s * 2)));
        }
    }
    best
}

/// The characters of `text` (by char index) that the terms of `query` match, for highlighting:
/// each term is matched left to right, preferring the earliest run.
pub fn match_positions(query: &str, text: &str) -> Vec<usize> {
    let t: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let mut out = Vec::new();
    for term in query.split_whitespace() {
        let p: Vec<char> = term.chars().flat_map(char::to_lowercase).collect();
        // A contiguous match first, then any subsequence.
        let contiguous = (0..t.len()).find(|&s| t[s..].starts_with(&p));
        match contiguous {
            Some(s) => out.extend(s..s + p.len()),
            None => {
                let mut found = Vec::new();
                let mut j = 0;
                for (i, c) in t.iter().enumerate() {
                    if j < p.len() && *c == p[j] {
                        found.push(i);
                        j += 1;
                    }
                }
                if j == p.len() {
                    out.extend(found);
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Why a bookmark matched `query`, for the right side of its row in the results: its address
/// when the name or the address matched, `#tag` when a tag did, otherwise its group. With the
/// characters (by index) the query matched there.
pub fn match_reason(query: &str, b: &Bookmark, group_path: &str) -> (String, Vec<usize>) {
    let terms: Vec<&str> = query.split_whitespace().collect();
    let address = b.address();
    let name = b.display_name();
    if terms.iter().all(|t| fuzzy_score(t, &name).is_some()) {
        return (address, Vec::new());
    }
    // A term that only a tag explains: show that tag.
    for tag in &b.tags {
        let explains = terms.iter().any(|t| {
            fuzzy_score(t, tag).is_some()
                && fuzzy_score(t, &name).is_none()
                && fuzzy_score(t, &address).is_none()
        });
        if explains {
            let hits = match_positions(query, tag)
                .into_iter()
                .map(|i| i + 1)
                .collect();
            return (format!("#{tag}"), hits);
        }
    }
    if terms.iter().any(|t| fuzzy_score(t, &address).is_some()) {
        return (address.clone(), match_positions(query, &address));
    }
    if !group_path.is_empty() {
        return (group_path.to_string(), match_positions(query, group_path));
    }
    (address, Vec::new())
}

/// Bookmarks matching every whitespace-separated term in `query`, best first. An empty query
/// returns all bookmarks in store order.
pub fn search(store: &BookmarkStore, query: &str) -> Vec<Uuid> {
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() {
        return store.bookmarks.iter().map(|b| b.id).collect();
    }
    let mut hits: Vec<(i32, usize, Uuid)> = Vec::new();
    for (idx, b) in store.bookmarks.iter().enumerate() {
        let path = store.group_path(b.group);
        let label = store.label_of(b).map(|l| l.tag()).unwrap_or_default();
        let mut total = 0;
        let mut all = true;
        for term in &terms {
            match term_score(term, b, &path, &label) {
                Some(s) => total += s,
                None => {
                    all = false;
                    break;
                }
            }
        }
        if all {
            hits.push((total, idx, b.id));
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hits.into_iter().map(|h| h.2).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_and_ranking() {
        assert!(fuzzy_score("pdb", "prod-db").is_some());
        assert!(fuzzy_score("xyz", "prod-db").is_none());
        assert!(fuzzy_score("pdb", "prod-db") > fuzzy_score("pdb", "upload-backup-db"));
        assert!(fuzzy_score("WEB", "web-01").is_some());
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }

    #[test]
    fn search_matches_all_terms_across_fields() {
        let mut s = BookmarkStore::default();
        let g = s.add_group("Production", None);
        let mk = |name: &str, host: &str, tags: &[&str], group| Bookmark {
            name: name.into(),
            host: host.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            group,
            ..Default::default()
        };
        s.upsert_bookmark(mk("web 1", "10.0.0.1", &["nginx"], Some(g)));
        s.upsert_bookmark(mk("db main", "db.internal", &["postgres"], Some(g)));
        s.upsert_bookmark(mk("staging web", "stg.example.com", &[], None));

        let names = |q: &str| -> Vec<String> {
            search(&s, q)
                .iter()
                .map(|id| s.bookmark(*id).unwrap().name.clone())
                .collect()
        };
        assert_eq!(names("postgres"), ["db main"]);
        assert_eq!(names("prod web"), ["web 1"]);
        assert_eq!(names("web")[0], "web 1");
        assert_eq!(names("").len(), 3);
        assert!(names("nomatch").is_empty());

        let web = s.bookmark(search(&s, "web 1")[0]).unwrap().clone();
        assert_eq!(match_reason("web", &web, "Production").0, web.address());
        assert_eq!(
            match_reason("ngi", &web, "Production"),
            ("#nginx".into(), vec![1, 2, 3])
        );
        assert_eq!(match_reason("10.0", &web, "Production").0, "10.0.0.1");
    }
}
