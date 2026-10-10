//! Paged reads retain only the requested lines, not the entire source file.
use super::OUTPUT_BYTES;
use crate::context::tools::SourceRoot;
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read};

const MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024;

pub(super) struct Page {
    pub path: String,
    pub offset: usize,
    pub lines: usize,
    /// Raw page lines without line-number labels, for citation fingerprints.
    pub texts: Vec<String>,
    pub result: Value,
}

pub(super) fn read(root: &SourceRoot, path: &str, offset: usize, limit: usize) -> Result<Page> {
    let resolved = root.resolve(path)?;
    ensure!(resolved.is_file(), "not a file: {path}");
    let file = std::fs::File::open(&resolved)?;
    page(
        BufReader::new(file),
        root.relative(&resolved)?,
        offset,
        limit,
        MAX_SCAN_BYTES,
    )
}

fn page(
    reader: impl BufRead,
    path: String,
    offset: usize,
    limit: usize,
    scan_limit: u64,
) -> Result<Page> {
    ensure!(offset > 0 && limit > 0, "offset and limit must be positive");
    let mut reader = reader.take(scan_limit.saturating_add(1));
    let mut scanned = 0u64;
    for _ in 1..offset {
        let n = reader.skip_until(b'\n')?;
        scanned += n as u64;
        ensure!(
            scanned <= scan_limit,
            "read exceeds scan byte limit; use Bash to narrow the source"
        );
        ensure!(n > 0, "offset is beyond end of file");
    }
    ensure!(
        offset == 1 || !reader.fill_buf()?.is_empty(),
        "offset is beyond end of file"
    );

    // Reserve exact envelope cost plus room for the loop's budget metadata.
    let overhead = serde_json::to_string(&json!({"path":path,"content":"","total_lines":usize::MAX,"next_offset":usize::MAX,"truncated":false}))?.len() + 128;
    let budget = OUTPUT_BYTES.saturating_sub(overhead);
    let mut content = String::new();
    let mut texts = Vec::new();
    let mut encoded_bytes = 0;
    let mut lines = 0;
    let mut eof = false;
    while lines < limit {
        if reader.fill_buf()?.is_empty() {
            eof = true;
            break;
        }
        // A giant/minified line must not allocate memory proportional to file size.
        let mut bytes = Vec::new();
        let n = (&mut reader)
            .take(OUTPUT_BYTES as u64 + 1)
            .read_until(b'\n', &mut bytes)?;
        scanned += n as u64;
        ensure!(
            scanned <= scan_limit,
            "read exceeds scan byte limit; use Bash to narrow the source"
        );
        if n > OUTPUT_BYTES {
            ensure!(
                lines > 0,
                "line {} exceeds output cap; inspect it with Bash",
                offset
            );
            break;
        }
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
        }
        let text = std::str::from_utf8(&bytes).context("source page is not UTF-8")?;
        let numbered = format!("{}: {}\n", offset + lines, text);
        let cost = serde_json::to_string(&numbered)?.len();
        if encoded_bytes + cost > budget {
            ensure!(
                lines > 0,
                "line {} exceeds output cap; inspect it with Bash",
                offset
            );
            break;
        }
        encoded_bytes += cost;
        content.push_str(&numbered);
        texts.push(text.to_owned());
        lines += 1;
        if reader.fill_buf()?.is_empty() {
            eof = true;
            break;
        }
    }
    let next = offset - 1 + lines;
    Ok(Page {
        result: json!({"path":path,"content":content,"total_lines":if eof {Some(next)} else {None},"next_offset":if eof {None} else {Some(next+1)},"truncated":!eof}),
        path,
        offset,
        lines,
        texts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn paging_handles_crlf_empty_files_and_unterminated_lines() {
        let first = page(Cursor::new("first\r\n\r\n雪"), "a.py".into(), 1, 2, 1024).unwrap();
        assert_eq!(first.result["content"], "1: first\n2: \n");
        assert_eq!(first.result["next_offset"], 3);
        assert!(first.result["total_lines"].is_null());
        let last = page(Cursor::new("first\r\n\r\n雪"), "a.py".into(), 3, 2, 1024).unwrap();
        assert_eq!(last.result["content"], "3: 雪\n");
        assert_eq!(last.result["total_lines"], 3);
        assert_eq!(last.result["truncated"], false);
        let empty = page(Cursor::new(""), "a.py".into(), 1, 1, 10).unwrap();
        assert_eq!(empty.lines, 0);
        assert_eq!(empty.result["total_lines"], 0);
        assert!(page(Cursor::new("x\n"), "a.py".into(), 2, 1, 10).is_err());
    }

    #[test]
    fn oversized_lines_and_scan_limits_are_bounded_and_explicit() {
        assert!(page(
            Cursor::new("x".repeat(OUTPUT_BYTES * 2)),
            "a.py".into(),
            1,
            1,
            100_000
        )
        .is_err());
        let input = format!("ok\n{}", "x".repeat(OUTPUT_BYTES * 2));
        let result = page(Cursor::new(input), "a.py".into(), 1, 10, 100_000).unwrap();
        assert_eq!(result.lines, 1);
        assert_eq!(result.result["next_offset"], 2);
        assert!(page(Cursor::new("0123456789\nx"), "a.py".into(), 2, 1, 5).is_err());
        assert!(page(Cursor::new("0123456789"), "a.py".into(), 1, 1, 5).is_err());
        let exact = page(Cursor::new("01234"), "a.py".into(), 1, 1, 5).unwrap();
        assert_eq!(exact.result["total_lines"], 1);
    }
}
