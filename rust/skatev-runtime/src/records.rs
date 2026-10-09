//! Player records: the best Hall of Meat bails and
//! banked trick lines, overall and per GTA map zone (a "spot"), shared by
//! the protagonists and tagged with who set them. The book is the player's
//! own play, saved as JSON where the host says (beside the runtime log); the
//! labels the host shows are Skate 3's own strings, read from the converted
//! language table.
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Entries kept per category overall and per spot.
pub const TOP: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    HallOfMeat = 1,
    Line = 2,
}

impl Category {
    pub fn from_u32(v: u32) -> Option<Self> {
        match v {
            1 => Some(Self::HallOfMeat),
            2 => Some(Self::Line),
            _ => None,
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::HallOfMeat => "hall_of_meat",
            Self::Line => "line",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub score: u32,
    pub character: String,
    /// GTA zone code (GET_NAME_OF_ZONE), empty when unknown.
    pub spot: String,
    /// The zone's display name as GTA showed it.
    pub spot_name: String,
    /// Unix seconds.
    pub time: u64,
}

/// Where a new score landed (ranks are 1-based; 0: not in that top list).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Placement {
    pub rank: u32,
    pub spot_rank: u32,
}

/// Who and where the next score is set (host-supplied).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Context {
    pub character: String,
    pub spot: String,
    pub spot_name: String,
}

#[derive(Clone, Debug, Default)]
pub struct Book {
    hall_of_meat: Vec<Entry>,
    line: Vec<Entry>,
}

impl Book {
    fn list(&self, c: Category) -> &Vec<Entry> {
        match c {
            Category::HallOfMeat => &self.hall_of_meat,
            Category::Line => &self.line,
        }
    }
    fn list_mut(&mut self, c: Category) -> &mut Vec<Entry> {
        match c {
            Category::HallOfMeat => &mut self.hall_of_meat,
            Category::Line => &mut self.line,
        }
    }

    /// Records `entry`; returns where it landed. Ties keep the older entry
    /// ahead (a record has to be beaten, not matched).
    pub fn add(&mut self, c: Category, entry: Entry) -> Placement {
        if entry.score == 0 {
            return Placement::default();
        }
        let list = self.list_mut(c);
        let at = list.iter().position(|e| e.score < entry.score).unwrap_or(list.len());
        list.insert(at, entry);
        let placement = self.placement(c, at);
        self.prune(c);
        placement
    }

    fn placement(&self, c: Category, at: usize) -> Placement {
        let list = self.list(c);
        let spot = &list[at].spot;
        let rank = if at < TOP { at as u32 + 1 } else { 0 };
        let spot_index = list[..at].iter().filter(|e| &e.spot == spot).count();
        let spot_rank = if !spot.is_empty() && spot_index < TOP { spot_index as u32 + 1 } else { 0 };
        Placement { rank, spot_rank }
    }

    /// Keeps the overall top and every spot's top; the rest can never show.
    fn prune(&mut self, c: Category) {
        let list = self.list_mut(c);
        let mut per_spot: std::collections::HashMap<String, usize> = Default::default();
        let mut kept = Vec::with_capacity(list.len());
        for (i, e) in list.drain(..).enumerate() {
            let n = per_spot.entry(e.spot.clone()).or_default();
            *n += 1;
            if i < TOP || (!e.spot.is_empty() && *n <= TOP) {
                kept.push(e);
            }
        }
        *list = kept;
    }

    /// The top entries overall, or of one spot.
    pub fn top(&self, c: Category, spot: Option<&str>) -> Vec<&Entry> {
        self.list(c)
            .iter()
            .filter(|e| spot.is_none_or(|s| !s.is_empty() && e.spot == s))
            .take(TOP)
            .collect()
    }

    pub fn best(&self, c: Category, spot: Option<&str>) -> u32 {
        self.top(c, spot).first().map_or(0, |e| e.score)
    }

    pub fn to_json(&self) -> Value {
        let list = |l: &Vec<Entry>| {
            l.iter()
                .map(|e| {
                    json!({"score": e.score, "character": e.character, "spot": e.spot,
                           "spot_name": e.spot_name, "time": e.time})
                })
                .collect::<Vec<_>>()
        };
        json!({"version": 1, Category::HallOfMeat.key(): list(&self.hall_of_meat),
               Category::Line.key(): list(&self.line)})
    }

    pub fn from_json(v: &Value) -> Self {
        let list = |c: Category| {
            let mut l: Vec<Entry> = v[c.key()]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|e| {
                            let s = |k: &str| e[k].as_str().unwrap_or_default().to_string();
                            Some(Entry {
                                score: u32::try_from(e["score"].as_u64()?).ok()?,
                                character: s("character"),
                                spot: s("spot"),
                                spot_name: s("spot_name"),
                                time: e["time"].as_u64().unwrap_or(0),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            l.sort_by(|a, b| b.score.cmp(&a.score));
            l
        };
        let mut b = Self { hall_of_meat: list(Category::HallOfMeat), line: list(Category::Line) };
        b.prune(Category::HallOfMeat);
        b.prune(Category::Line);
        b
    }
}

/// The last score placed in the book, for the host's call-out.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Event {
    /// Increments with every placed score (0: none yet).
    pub sequence: u32,
    pub category: u32,
    pub score: u32,
    pub placement: Placement,
}

/// The book on disk plus the host's context and the line-bank watcher.
#[derive(Default)]
pub struct Records {
    path: Option<PathBuf>,
    pub book: Book,
    pub context: Context,
    pub last: Event,
    /// Skate's lifetime banked-line total last seen (None: not seen yet).
    completed_lines: Option<f32>,
}

impl Records {
    /// Opens (or starts) the book at `path`.
    pub fn open(&mut self, path: &Path, log: &crate::worker::Log) {
        self.book = match std::fs::read(path) {
            Ok(b) => match serde_json::from_slice::<Value>(&b) {
                Ok(v) => Book::from_json(&v),
                Err(e) => {
                    log(&format!("records: {} unreadable ({e}); starting a new book", path.display()));
                    Book::default()
                }
            },
            Err(_) => Book::default(),
        };
        log(&format!(
            "records: {} (Hall of Meat best {}, line best {})",
            path.display(),
            self.book.best(Category::HallOfMeat, None),
            self.book.best(Category::Line, None)
        ));
        self.path = Some(path.to_path_buf());
    }

    fn save(&self, log: &crate::worker::Log) {
        let Some(path) = &self.path else { return };
        let partial = path.with_extension("json.partial");
        let text = serde_json::to_string_pretty(&self.book.to_json()).unwrap_or_default();
        if let Err(e) = std::fs::write(&partial, text).and_then(|_| std::fs::rename(&partial, path)) {
            log(&format!("records: could not save {} ({e})", path.display()));
        }
    }

    /// Places a finished score; returns where it landed.
    pub fn add(&mut self, c: Category, score: u32, time: u64, log: &crate::worker::Log) -> Placement {
        let entry = Entry {
            score,
            character: self.context.character.clone(),
            spot: self.context.spot.clone(),
            spot_name: self.context.spot_name.clone(),
            time,
        };
        let placement = self.book.add(c, entry);
        if placement.rank > 0 || placement.spot_rank > 0 {
            self.last = Event {
                sequence: self.last.sequence.wrapping_add(1).max(1),
                category: c as u32,
                score,
                placement,
            };
            log(&format!(
                "records: {c:?} {score} by {} at {} -> rank {} overall, {} at the spot",
                self.context.character, self.context.spot, placement.rank, placement.spot_rank
            ));
            self.save(log);
        }
        placement
    }

    /// Watches Skate's lifetime banked-line total: each increase is one
    /// banked line worth the increase. A drop (new session) resynchronises.
    pub fn observe_lines(&mut self, completed_lines: f32, time: u64, log: &crate::worker::Log) {
        let banked = match self.completed_lines {
            Some(previous) if completed_lines > previous => completed_lines - previous,
            _ => 0.0,
        };
        self.completed_lines = Some(completed_lines);
        if banked >= 1.0 {
            self.add(Category::Line, banked.round() as u32, time, log);
        }
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Skate 3 strings the record display uses, by language id.
pub const LABELS: [&str; 9] = [
    "ID_LEADERBOARD_PLAYER_RECORDS",
    "ID_LEADERBOARD_NEW_PERSONAL_BEST",
    "ID_RECORD_BESTS_34",
    "ID_RECORD_BESTS_05",
    "ID_CREATEASPOT_TOP_10",
    "ID_HOM_ANNALS_OF_MEAT_RECORD",
    "ID_LEADERBOARD_HEADER_RANK",
    "ID_OTS_PERSONAL_BEST_LABEL",
    "ID_RANKED_LEADERBOARDS_NO_SCORES",
];

/// The record labels from the converted trickdisplay language table.
pub fn load_labels(data_root: &Path) -> std::collections::HashMap<String, String> {
    let path = data_root.join("private/hud/runtime/trickdisplay.json");
    let Ok(bytes) = std::fs::read(&path) else { return Default::default() };
    let Ok(v) = serde_json::from_slice::<Value>(&bytes) else { return Default::default() };
    LABELS
        .iter()
        .filter_map(|id| Some((id.to_string(), v["language"][*id].as_str()?.to_string())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(score: u32, spot: &str) -> Entry {
        Entry { score, character: "A".into(), spot: spot.into(), spot_name: spot.into(), time: 1 }
    }

    #[test]
    fn placement_ranks_overall_and_at_the_spot() {
        let mut b = Book::default();
        assert_eq!(b.add(Category::HallOfMeat, e(100, "X")), Placement { rank: 1, spot_rank: 1 });
        assert_eq!(b.add(Category::HallOfMeat, e(50, "Y")), Placement { rank: 2, spot_rank: 1 });
        assert_eq!(b.add(Category::HallOfMeat, e(70, "Y")), Placement { rank: 2, spot_rank: 1 });
        // A tie does not beat the record.
        assert_eq!(b.add(Category::HallOfMeat, e(100, "Y")), Placement { rank: 2, spot_rank: 1 });
        assert_eq!(b.best(Category::HallOfMeat, Some("Y")), 100);
        assert_eq!(b.best(Category::Line, None), 0);
        assert_eq!(b.add(Category::Line, e(0, "X")), Placement::default());
    }

    #[test]
    fn spot_tops_survive_outside_the_overall_top() {
        let mut b = Book::default();
        for s in 0..TOP as u32 {
            b.add(Category::Line, e(1000 + s, "X"));
        }
        // Eleventh overall but first at Y: kept and ranked at its spot.
        let p = b.add(Category::Line, e(10, "Y"));
        assert_eq!(p, Placement { rank: 0, spot_rank: 1 });
        assert_eq!(b.top(Category::Line, Some("Y")).len(), 1);
        assert_eq!(b.top(Category::Line, None).len(), TOP);
        // Below X's own top ten and the overall top: dropped.
        assert_eq!(b.add(Category::Line, e(5, "X")), Placement::default());
        assert!(b.top(Category::Line, Some("X")).iter().all(|e| e.score >= 1000));
    }

    #[test]
    fn json_round_trip_keeps_order_and_fields() {
        let mut b = Book::default();
        b.add(Category::HallOfMeat, e(30, "X"));
        b.add(Category::HallOfMeat, e(90, "Y"));
        b.add(Category::Line, e(7, ""));
        let back = Book::from_json(&b.to_json());
        assert_eq!(back.top(Category::HallOfMeat, None), b.top(Category::HallOfMeat, None));
        assert_eq!(back.top(Category::Line, None)[0].score, 7);
        // An unknown spot has no spot list.
        assert!(back.top(Category::Line, Some("")).is_empty());
    }

    #[test]
    fn banked_lines_are_increases_of_the_lifetime_total() {
        let log: crate::worker::Log = std::sync::Arc::new(|_| {});
        let mut r = Records::default();
        r.observe_lines(0.0, 1, &log);
        r.observe_lines(0.0, 1, &log);
        assert_eq!(r.book.best(Category::Line, None), 0);
        r.observe_lines(250.0, 1, &log);
        r.observe_lines(400.0, 1, &log);
        assert_eq!(r.book.top(Category::Line, None).iter().map(|e| e.score).collect::<Vec<_>>(), [250, 150]);
        assert_eq!(r.last.score, 150);
        assert_eq!(r.last.placement.rank, 2);
        // A new session restarts the total without inventing a line.
        r.observe_lines(0.0, 1, &log);
        r.observe_lines(80.0, 1, &log);
        assert_eq!(r.book.top(Category::Line, None).len(), 3);
    }
}
