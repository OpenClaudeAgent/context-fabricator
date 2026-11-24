use std::error::Error;
use std::path::PathBuf;

use ctxfab_server::domain::corpus::passage::passages;
use serde::Deserialize;

#[test]
fn passages_preserve_original_bytes_and_one_based_lines() -> Result<(), Box<dyn Error>> {
    let source = "alpha\r\né🦀🦀\r\nomega\n";
    let chunks: Vec<_> = passages(source, 7).ok_or("valid byte budget")?.collect();
    let coordinates: Vec<_> = chunks
        .iter()
        .map(|chunk| (chunk.bytes.clone(), chunk.start_line, chunk.end_line))
        .collect();
    assert_eq!(
        coordinates,
        [(0..7, 1, 1), (7..13, 2, 2), (13..19, 2, 2), (19..25, 3, 3)]
    );
    assert!(
        chunks
            .iter()
            .all(|chunk| chunk.text == &source[chunk.bytes.clone()])
    );
    assert_eq!(
        chunks.iter().map(|chunk| chunk.text).collect::<String>(),
        source
    );
    Ok(())
}

#[test]
fn passages_cover_a_line_break_cutoff_and_empty_files() -> Result<(), Box<dyn Error>> {
    let source = "abcd\r\nnext";
    let chunks: Vec<_> = passages(source, 5).ok_or("valid byte budget")?.collect();
    assert_eq!(
        chunks.iter().map(|chunk| chunk.text).collect::<Vec<_>>(),
        ["abcd", "\r\n", "next"]
    );
    assert_eq!((chunks[1].start_line, chunks[1].end_line), (1, 1));
    assert_eq!(chunks[2].start_line, 2);
    assert!(passages("🦀", 3).is_none());
    assert!(passages("", 4).ok_or("valid byte budget")?.next().is_none());
    Ok(())
}

#[test]
#[ignore = "requires pinned source files; run quality/github/reference-corpora.sh"]
fn pinned_sources_preserve_late_file_symbols() -> Result<(), Box<dyn Error>> {
    #[derive(Deserialize)]
    struct Fixture {
        file: String,
        needle: String,
        line: usize,
    }
    const BUDGET: usize = 2_048;
    let directory =
        PathBuf::from(std::env::var_os("CTXFAB_REFERENCE_CORPUS").ok_or("set fixture directory")?);
    let fixtures: Vec<Fixture> = serde_json::from_str(include_str!(
        "../../../quality/github/reference-corpora.json"
    ))?;
    for Fixture { file, needle, line } in fixtures {
        let source = std::fs::read_to_string(directory.join(&file))?;
        let offset = source.find(&needle).ok_or("named symbol missing")?;
        assert_eq!(source[..offset].matches('\n').count() + 1, line);
        let chunks: Vec<_> = passages(&source, BUDGET)
            .ok_or("valid byte budget")?
            .collect();
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.bytes.len() <= BUDGET
                    && chunk.text == &source[chunk.bytes.clone()])
        );
        assert_eq!(
            chunks.iter().map(|chunk| chunk.text).collect::<String>(),
            source
        );
        let chunk = chunks
            .iter()
            .find(|chunk| chunk.bytes.contains(&offset))
            .ok_or("symbol byte offset was not covered")?;
        assert_eq!(
            chunk
                .text
                .get(offset - chunk.bytes.start..offset - chunk.bytes.start + needle.len()),
            Some(needle.as_str()),
            "named symbol split in {file}"
        );
        assert_eq!(
            chunk.start_line + source[chunk.bytes.start..offset].matches('\n').count(),
            line
        );
        assert!(chunk.end_line >= line);
        eprintln!(
            "{file} bytes {}..{}, lines {}–{}, symbol at line {line}",
            chunk.bytes.start, chunk.bytes.end, chunk.start_line, chunk.end_line
        );
    }
    Ok(())
}
