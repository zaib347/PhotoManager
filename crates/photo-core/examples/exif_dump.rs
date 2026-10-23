//! Prints the EXIF fields of a file and what the indexer extracts from it.
//!
//! cargo run -p photo-core --example exif_dump -- <file>

use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use photo_core::media;

fn main() {
    let path = PathBuf::from(std::env::args().nth(1).expect("usage: exif_dump <file>"));
    match exif::Reader::new().read_from_container(&mut BufReader::new(File::open(&path).unwrap())) {
        Ok(exif) => {
            for f in exif.fields() {
                println!(
                    "{:<8} {:<28} {}",
                    f.ifd_num,
                    f.tag,
                    f.display_value().with_unit(&exif)
                );
            }
        }
        Err(e) => println!("no exif: {e}"),
    }
    if let Some(kind) = media::kind_of(&path) {
        let mut meta = media::extract(&path, kind, 0);
        let thumb = meta.embedded_thumb.take().map(|t| t.len());
        println!("\nextracted: {meta:?}\nembedded thumbnail bytes: {thumb:?}");
    }
}
