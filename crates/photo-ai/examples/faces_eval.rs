//! Measures face grouping on a folder of people (one sub-folder of photos per person,
//! like LFW): how often faces of one person join, and of two people stay apart.
//! A development aid: `cargo run --release -p photo-ai --example faces_eval -- <models>
//! <people folder> [join threshold]`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use photo_ai::faces::Faces;
use photo_ai::models::{Pack, Store};
use photo_ai::people::{Groups, assign};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let models = PathBuf::from(&args[1]);
    let folder = PathBuf::from(&args[2]);
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    store.install_from(Pack::Runtime, &models).unwrap();
    store.install_from(Pack::People, &models).unwrap();
    let threads = std::env::var("THREADS")
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or(8);
    let faces = Faces::load(&store, threads).unwrap();

    // (person, description) of the face nearest each photo's centre.
    let mut found: Vec<(String, Vec<f32>)> = Vec::new();
    let (mut photos, mut none, mut extra) = (0, 0, 0);
    let (mut t_read, mut t_detect, mut t_describe) = (0f64, 0f64, 0f64);
    let started = std::time::Instant::now();
    let mut people: Vec<_> = std::fs::read_dir(&folder).unwrap().flatten().collect();
    people.sort_by_key(|e| e.file_name());
    for person in people {
        let name = person.file_name().to_string_lossy().into_owned();
        let mut files: Vec<_> = std::fs::read_dir(person.path())
            .unwrap()
            .flatten()
            .collect();
        files.sort_by_key(|e| e.file_name());
        for file in files.iter().take(60) {
            let t = std::time::Instant::now();
            let Some(image) = photo_ai::picture::read(&file.path(), None, 1600, 2400) else {
                continue;
            };
            t_read += t.elapsed().as_secs_f64();
            photos += 1;
            let t = std::time::Instant::now();
            let detected = faces.detect(&image).unwrap();
            t_detect += t.elapsed().as_secs_f64();
            if detected.len() > 1 {
                extra += 1;
            }
            let (cx, cy) = (image.width() as f32 / 2.0, image.height() as f32 / 2.0);
            let Some(face) = detected.iter().min_by(|a, b| {
                let d = |f: &photo_ai::faces::Face| {
                    (f.x + f.w / 2.0 - cx).powi(2) + (f.y + f.h / 2.0 - cy).powi(2)
                };
                d(a).total_cmp(&d(b))
            }) else {
                none += 1;
                continue;
            };
            let t = std::time::Instant::now();
            found.push((name.clone(), faces.describe(&image, face).unwrap()));
            t_describe += t.elapsed().as_secs_f64();
        }
    }
    let ms = started.elapsed().as_millis() as f64 / photos.max(1) as f64;
    println!(
        "{photos} photos, {ms:.0} ms each; no face in {none}, more than one in {extra}; {} faces",
        found.len()
    );
    let n = photos.max(1) as f64 / 1000.0;
    println!(
        "read {:.0} ms, detect {:.0} ms, describe {:.0} ms each",
        t_read / n,
        t_detect / n,
        t_describe / n
    );

    // Pairs of single faces.
    let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    let (mut same, mut other) = (Vec::new(), Vec::new());
    for i in 0..found.len() {
        for j in i + 1..found.len() {
            let s = dot(&found[i].1, &found[j].1);
            if found[i].0 == found[j].0 {
                same.push(s);
            } else {
                other.push(s);
            }
        }
    }
    let share = |v: &[f32], t: f32| v.iter().filter(|&&s| s >= t).count() as f64 / v.len() as f64;
    for t in [0.3, 0.363, 0.4, 0.45, 0.5] {
        println!(
            "pairs at {t}: same person alike {:.1}%, different people alike {:.3}%",
            share(&same, t) * 100.0,
            share(&other, t) * 100.0
        );
    }

    // Grouping, in a shuffled order (as a library would arrive).
    let mut order: Vec<usize> = (0..found.len()).collect();
    let mut seed = 12345u64;
    for i in (1..order.len()).rev() {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        order.swap(i, (seed >> 33) as usize % (i + 1));
    }
    let mut groups = Groups::default();
    let mut next = 0;
    let mut person_of: Vec<i64> = vec![0; found.len()];
    for &i in &order {
        person_of[i] = assign(&mut groups, &[found[i].1.clone()], &mut || {
            next += 1;
            Ok(next)
        })
        .unwrap()[0];
    }
    let mut renamed: HashMap<i64, i64> = HashMap::new();
    for (keep, gone) in groups.merges() {
        renamed.insert(gone, keep);
    }
    let resolve = |mut p: i64| {
        while let Some(&k) = renamed.get(&p) {
            p = k;
        }
        p
    };
    let mut members: HashMap<i64, Vec<&str>> = HashMap::new();
    for (i, (name, _)) in found.iter().enumerate() {
        members.entry(resolve(person_of[i])).or_default().push(name);
    }
    // A group's faces that aren't its main person are wrong; a person spread over
    // several groups (of 3+, as listed) is split.
    let mut wrong = 0;
    let mut listed: HashMap<&str, usize> = HashMap::new();
    for faces in members.values() {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for n in faces {
            *counts.entry(n).or_default() += 1;
        }
        let (main, n) = counts.iter().max_by_key(|e| e.1).unwrap();
        wrong += faces.len() - n;
        if faces.len() >= 3 {
            *listed.entry(main).or_default() += 1;
        }
    }
    let names: HashSet<&str> = found.iter().map(|f| f.0.as_str()).collect();
    println!(
        "grouped: {} groups ({} of 3+), {wrong} faces in someone else's group",
        members.len(),
        members.values().filter(|m| m.len() >= 3).count()
    );
    let mut names: Vec<_> = names.into_iter().collect();
    names.sort();
    for n in names {
        let in_main = members
            .values()
            .filter_map(|m| {
                let c = m.iter().filter(|x| **x == n).count();
                (c > 0).then_some(c)
            })
            .max()
            .unwrap_or(0);
        let total = found.iter().filter(|f| f.0 == n).count();
        println!(
            "  {n}: {in_main} of {total} in their biggest group, listed {} times",
            listed.get(n).copied().unwrap_or(0)
        );
    }
}
