//! CLIP's tokenizer: byte-level BPE over lower-cased text, as in OpenAI's
//! `simple_tokenizer.py`, read from the model's `vocab.json` and `merges.txt`.
//!
//! Text is split into words (letters, single digits, runs of other symbols, and the
//! English contractions), each word's UTF-8 bytes are mapped to printable characters,
//! the last one marked with `</w>`, and pairs are merged in rank order.

use std::collections::HashMap;

use photo_core::{Error, Result};

/// Tokens per text, including the start and end markers.
pub const CONTEXT: usize = 77;
const START: &str = "<|startoftext|>";
const END: &str = "<|endoftext|>";
const CONTRACTIONS: [&str; 7] = ["'s", "'t", "'re", "'ve", "'m", "'ll", "'d"];

pub struct Tokenizer {
    vocab: HashMap<String, i64>,
    ranks: HashMap<(String, String), usize>,
    /// Byte → the printable character that stands for it.
    byte_char: [char; 256],
    start: i64,
    end: i64,
}

/// GPT-2's reversible byte ↔ character table: printable bytes stand for themselves,
/// the rest get characters from 256 upwards.
fn bytes_to_chars() -> [char; 256] {
    let mut table = ['\0'; 256];
    let mut next = 256u32;
    for b in 0..=255u8 {
        let printable =
            (b'!'..=b'~').contains(&b) || (0xA1..=0xAC).contains(&b) || (0xAE..=0xFF).contains(&b);
        table[b as usize] = if printable {
            char::from(b)
        } else {
            let c = char::from_u32(next).expect("valid");
            next += 1;
            c
        };
    }
    table
}

fn invalid(what: &str) -> Error {
    Error::Invalid(format!(
        "The search model's {what} is damaged; remove and install it again"
    ))
}

impl Tokenizer {
    pub fn new(vocab_json: &str, merges_txt: &str) -> Result<Tokenizer> {
        let vocab: HashMap<String, i64> =
            serde_json::from_str(vocab_json).map_err(|_| invalid("vocabulary"))?;
        let mut ranks = HashMap::new();
        // The first line is a version header.
        for (rank, line) in merges_txt.lines().skip(1).enumerate() {
            if let Some((a, b)) = line.split_once(' ') {
                ranks.insert((a.to_owned(), b.to_owned()), rank);
            }
        }
        let start = *vocab.get(START).ok_or_else(|| invalid("vocabulary"))?;
        let end = *vocab.get(END).ok_or_else(|| invalid("vocabulary"))?;
        if ranks.is_empty() {
            return Err(invalid("merge list"));
        }
        Ok(Tokenizer {
            vocab,
            ranks,
            byte_char: bytes_to_chars(),
            start,
            end,
        })
    }

    /// Splits cleaned text into words the way CLIP's regex does.
    fn words(text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c.is_whitespace() {
                i += 1;
                continue;
            }
            if c == '\'' {
                let rest: String = chars[i..chars.len().min(i + 3)].iter().collect();
                if let Some(k) = CONTRACTIONS.iter().find(|k| rest.starts_with(**k)) {
                    out.push((*k).to_owned());
                    i += k.chars().count();
                    continue;
                }
            }
            let start = i;
            if c.is_alphabetic() {
                while i < chars.len() && chars[i].is_alphabetic() {
                    i += 1;
                }
            } else if c.is_numeric() {
                i += 1;
            } else {
                while i < chars.len()
                    && !(chars[i].is_whitespace()
                        || chars[i].is_alphabetic()
                        || chars[i].is_numeric())
                {
                    i += 1;
                }
            }
            out.push(chars[start..i].iter().collect());
        }
        out
    }

    /// BPE merges of one word (already mapped to byte characters).
    fn bpe(&self, word: &str) -> Vec<String> {
        let chars: Vec<char> = word.chars().collect();
        let mut parts: Vec<String> = chars.iter().map(|c| c.to_string()).collect();
        if let Some(last) = parts.last_mut() {
            last.push_str("</w>");
        }
        loop {
            // The best-ranked adjacent pair.
            let best = parts
                .windows(2)
                .enumerate()
                .filter_map(|(i, w)| {
                    self.ranks
                        .get(&(w[0].clone(), w[1].clone()))
                        .map(|&r| (r, i))
                })
                .min();
            let Some((_, i)) = best else { break };
            let (a, b) = (parts[i].clone(), parts[i + 1].clone());
            // Merge every occurrence of the pair, left to right.
            let mut merged = Vec::with_capacity(parts.len());
            let mut j = 0;
            while j < parts.len() {
                if j + 1 < parts.len() && parts[j] == a && parts[j + 1] == b {
                    merged.push(format!("{a}{b}"));
                    j += 2;
                } else {
                    merged.push(parts[j].clone());
                    j += 1;
                }
            }
            parts = merged;
            if parts.len() == 1 {
                break;
            }
        }
        parts
    }

    /// Token ids for `text`, with the start and end markers, cut to fit and padded with
    /// the end marker to [`CONTEXT`]. Also returns how many are real (not padding).
    pub fn encode(&self, text: &str) -> (Vec<i64>, usize) {
        let cleaned = text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let mut ids = vec![self.start];
        for word in Self::words(&cleaned) {
            let mapped: String = word.bytes().map(|b| self.byte_char[b as usize]).collect();
            for piece in self.bpe(&mapped) {
                if let Some(&id) = self.vocab.get(&piece) {
                    ids.push(id);
                }
            }
        }
        ids.truncate(CONTEXT - 1);
        ids.push(self.end);
        let real = ids.len();
        ids.resize(CONTEXT, self.end);
        (ids, real)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_words_like_clip() {
        assert_eq!(
            Tokenizer::words("a dog's ball, 2024!! café"),
            vec![
                "a", "dog", "'s", "ball", ",", "2", "0", "2", "4", "!!", "café"
            ]
        );
    }

    #[test]
    fn byte_table_is_reversible() {
        let table = bytes_to_chars();
        let unique: std::collections::HashSet<char> = table.iter().copied().collect();
        assert_eq!(unique.len(), 256);
        assert_eq!(table[b'a' as usize], 'a');
        assert_eq!(table[b' ' as usize], 'Ġ');
    }

    /// Needs the real vocabulary: `PHOTO_AI_TEST_MODELS` points at a folder with the
    /// search model's `vocab.json` and `merges.txt`.
    #[test]
    fn matches_clips_token_ids() {
        let Some(dir) = std::env::var_os("PHOTO_AI_TEST_MODELS") else {
            eprintln!("skipped: set PHOTO_AI_TEST_MODELS");
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let t = Tokenizer::new(
            &std::fs::read_to_string(dir.join("vocab.json")).unwrap(),
            &std::fs::read_to_string(dir.join("merges.txt")).unwrap(),
        )
        .unwrap();
        let (ids, real) = t.encode("A photo of a  CAT");
        assert_eq!(&ids[..real], &[49406, 320, 1125, 539, 320, 2368, 49407]);
        assert_eq!(ids.len(), CONTEXT);
        assert!(ids[real..].iter().all(|&i| i == 49407));
        // Long text is cut, still ending with the end marker.
        let (long, real) = t.encode(&"word ".repeat(200));
        assert_eq!((long.len(), real), (CONTEXT, CONTEXT));
        assert_eq!(long[CONTEXT - 1], 49407);
    }
}
