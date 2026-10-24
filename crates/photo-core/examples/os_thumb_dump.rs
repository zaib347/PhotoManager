//! Writes the OS-provided thumbnail of a file (video, HEIC, …) as a JPEG.
//!
//! cargo run -p photo-core --example os_thumb_dump -- <file> <out.jpg>

use std::time::Instant;

fn main() -> photo_core::Result<()> {
    let mut args = std::env::args().skip(1);
    let input =
        std::path::PathBuf::from(args.next().expect("usage: os_thumb_dump <file> <out.jpg>"));
    let output =
        std::path::PathBuf::from(args.next().expect("usage: os_thumb_dump <file> <out.jpg>"));
    let t = Instant::now();
    let jpeg = photo_core::thumbs::from_os(&input, 0)?;
    println!("{} bytes in {:?}", jpeg.len(), t.elapsed());
    std::fs::write(output, jpeg)?;
    Ok(())
}
