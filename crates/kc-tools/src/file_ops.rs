//! File-oriented helpers: bounded reads, gitignore-aware listings, atomic
//! writes and exact-string edits with in-process diff previews.
//!
//! Every path handled here has already been through
//! [`kc_core::paths::Workspace`], so the functions themselves never re-check
//! confinement; they only reject shapes that make no sense (a directory where a
//! file is required, binary content, oversized payloads).

use kc_core::message::ImageAttachment;
use kc_core::paths::Workspace;
use kc_core::{Error, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_util::sync::CancellationToken;

pub const DEFAULT_READ_LINES: usize = 400;
pub const MAX_READ_LINES: usize = 2000;
/// Hard cap for a single `write_file` payload.
pub const MAX_FILE_WRITE_BYTES: usize = 2 * 1024 * 1024;
/// Cap for `edit_file` (the whole file is loaded to compute the diff).
pub const MAX_EDIT_BYTES: u64 = 8 * 1024 * 1024;
/// How many line numbers an ambiguity refusal lists before "…".
const MAX_REPORTED_LINES: usize = 10;
pub const MAX_LIST_ENTRIES: usize = 500;
/// How many leading bytes are inspected for a NUL byte when *planning an edit*.
///
/// `read_file` no longer uses a fixed prefix: it sniffs the whole window it is
/// about to return (a NUL at offset 20 000 used to slip past an 8 KiB check and
/// reach the model). An edit needs the whole file in memory anyway, so this is
/// only an early-out for an obviously binary target.
pub const BINARY_SNIFF_BYTES: usize = 8 * 1024;
/// Upper bound on the line count `read_file` will compute for a byte-capped read.
///
/// The count exists so the header can say "of N" for a file larger than
/// `limits.max_file_read_bytes`. Beyond this the number stops being useful and the
/// scan is wasted IO, so counting stops and the total is reported as a lower bound.
pub const MAX_COUNTED_LINES: usize = 5_000_000;
/// How many leading bytes are used to recognise an image by its magic number.
///
/// The extension is deliberately *not* trusted: `logo.png` that is really a
/// ZIP would otherwise be base64-attached and rejected by the endpoint, and a
/// real PNG saved as `assets/logo` must still work.
pub const IMAGE_SNIFF_BYTES: usize = 32;
pub const MAX_DEPTH: u32 = 3;
const DIFF_MAX_LINES: usize = 80;
const DIFF_CONTEXT_LINES: usize = 2;
/// Upper bound for the LCS table used to build the edit preview.
const DIFF_MAX_CELLS: usize = 400_000;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// A rendered tool body plus machine-readable metadata and a renderer-level
/// truncation flag (before the global tool-output cap is applied).
#[derive(Debug, Clone)]
pub struct Rendered {
    pub text: String,
    pub truncated: bool,
    pub meta: serde_json::Value,
}

/// Read a UTF-8 file, rendering a `path — lines A-B of N (bytes)` header and
/// `%5d\t` numbered lines.
///
/// `max_bytes` bounds what is *returned*, not what is *reachable*: when the
/// requested `offset` starts past the bytes a prefix read could cover, the file
/// is streamed forward to the window instead of that window being unreachable
/// (see [`read_window`]).
pub fn read_file(
    ws: &Workspace,
    path: &str,
    offset: Option<u64>,
    limit: Option<u64>,
    max_bytes: usize,
) -> Result<Rendered> {
    read_file_cancellable(
        ws,
        path,
        offset,
        limit,
        max_bytes,
        &CancellationToken::new(),
    )
}

/// [`read_file`] with a cancellation token, so a scan for a far window or a line
/// count can stop when the run is cancelled instead of finishing its IO first.
pub fn read_file_cancellable(
    ws: &Workspace,
    path: &str,
    offset: Option<u64>,
    limit: Option<u64>,
    max_bytes: usize,
    cancel: &CancellationToken,
) -> Result<Rendered> {
    let abs = ws.resolve_existing(path)?;
    let rel = ws.relative(&abs);
    let meta = std::fs::metadata(&abs).map_err(|e| Error::io(&abs, e))?;
    if meta.is_dir() {
        return Err(Error::BadRequest(format!(
            "`{rel}` is a directory; use list_dir to enumerate it"
        )));
    }
    if !meta.is_file() {
        return Err(Error::BadRequest(format!(
            "`{rel}` is not a regular file; refusing to read it"
        )));
    }
    let size = meta.len();
    let offset = offset.unwrap_or(1).max(1) as usize;
    let limit = limit
        .unwrap_or(DEFAULT_READ_LINES as u64)
        .clamp(1, MAX_READ_LINES as u64) as usize;

    // A window that starts beyond the byte cap used to be unreachable: the read
    // took `max_bytes` from byte 0 and then reported that the offset was "beyond
    // the lines that fit in the first N bytes". The cap bounds bytes *returned*,
    // so a later window is streamed to (bounded, cancellable) instead of refused.
    if offset > 1 && size > max_bytes as u64 {
        return read_window(&abs, &rel, size, offset, limit, max_bytes, cancel);
    }

    // One open, one pass. The prefix is read first (that is what gets returned);
    // when the byte cap cut it short, the *same* handle keeps reading to count the
    // file's lines. The old path opened the file a second time and re-read it from
    // byte 0, so the prefix was read twice and a big file cost two full scans.
    let want = (max_bytes as u64).saturating_add(1).min(size);
    let mut buf = Vec::new();
    let mut file = std::fs::File::open(&abs).map_err(|e| Error::io(&abs, e))?;
    {
        let mut limited = std::io::Read::by_ref(&mut file).take(want);
        limited
            .read_to_end(&mut buf)
            .map_err(|e| Error::io(&abs, e))?;
    }
    let byte_limited = buf.len() > max_bytes;
    if byte_limited {
        buf.truncate(max_bytes);
    }

    // Sniff the window that is actually returned, not a fixed 8 KiB prefix: a NUL
    // at offset 20 000 used to pass the check and reach the model as U+0000.
    if let Some(pos) = buf.iter().position(|b| *b == 0) {
        return Err(Error::BadRequest(format!(
            "`{rel}` looks like a binary file (NUL byte at offset {pos}); read_file only returns UTF-8 text. \
             Use run_command with `file`/`xxd` if you need to inspect it."
        )));
    }

    let text = String::from_utf8_lossy(&buf).into_owned();
    let lines: Vec<&str> = text.lines().collect();
    // `lines` only covers the bytes that were read. When the read was byte
    // limited, count the whole file so the header, `lines_total` and the
    // "next N lines" hint describe the real file instead of the prefix.
    let available = lines.len();
    let (total, total_capped) = if byte_limited {
        match continue_counting(&mut file, &buf) {
            // The count is a nicety; never fail a readable file over it.
            Ok(counted) => counted,
            Err(_) => (available, false),
        }
    } else {
        (available, false)
    };

    let start = offset - 1;
    let end = if start >= total {
        start
    } else {
        (start + limit).min(total)
    };
    // Never index past the bytes that were actually read.
    let shown_end = end.min(available);

    let (first, last) = if total == 0 {
        (0, 0)
    } else if start >= total {
        (total, total)
    } else if start >= available {
        // Inside the file, but past the bytes the cap let us read.
        (start + 1, available)
    } else {
        (start + 1, shown_end)
    };
    let mut out = String::new();
    // A capped count is a lower bound, so it is written `N+` and explained rather
    // than presented as the file's real length.
    let total_label = if total_capped {
        format!("{total}+")
    } else {
        total.to_string()
    };
    out.push_str(&format!(
        "{rel} — lines {first}-{last} of {total_label} ({size} bytes)"
    ));

    // Explain the (rare) byte cap before the content so it survives output
    // truncation of a huge file.
    if byte_limited {
        out.push_str(&format!(
            "\n[note: only the first {max_bytes} of {size} bytes were read (limits.max_file_read_bytes); \
             raise the limit to see the rest]"
        ));
        if total_capped {
            out.push_str(&format!(
                "\n[note: the line count stopped at {MAX_COUNTED_LINES} lines, so `{total}+` is a \
                 lower bound]"
            ));
        }
    }

    if total == 0 {
        out.push_str("\n[file is empty]");
    } else if start >= total {
        out.push_str(&format!(
            "\n[offset {offset} is past the end of the file ({total} lines); start at or before {total}]"
        ));
    } else if start >= available {
        out.push_str(&format!(
            "\n[offset {offset} is beyond the {available} lines that fit in the first {max_bytes} bytes; \
             raise limits.max_file_read_bytes to read further]"
        ));
    } else {
        for (i, line) in lines[start..shown_end].iter().enumerate() {
            out.push_str(&format!("\n{:>5}\t{}", start + i + 1, line));
        }
        // Only offer "continue with offset" when the window was not cut short by
        // the byte cap; otherwise the note above already explains the limit.
        if shown_end < total && shown_end == end {
            out.push_str(&format!(
                "\n[next {} lines omitted; continue with offset={}]",
                total - shown_end,
                shown_end + 1
            ));
        }
    }

    Ok(Rendered {
        text: out,
        truncated: byte_limited || end < total,
        meta: serde_json::json!({
            "path": rel,
            "lines_total": total,
            "lines_total_is_lower_bound": total_capped,
            "lines_from": first,
            "lines_to": last,
            "bytes": size,
            "byte_limited": byte_limited,
            "offset": offset,
            "limit": limit,
        }),
    })
}

/// Bytes read per step when a file is streamed rather than read whole.
const SCAN_CHUNK_BYTES: usize = 64 * 1024;

/// Read the line window `offset..offset + limit` of a file too large to read
/// from byte 0, by streaming forward to it.
///
/// The prefix is never buffered: the scan counts newlines and keeps only the
/// chunk bytes that follow the window's first line, so memory is bounded by
/// [`SCAN_CHUNK_BYTES`] plus the returned window (at most `max_bytes`).
///
/// Everything the prefix path reports is reported here too: the same header, the
/// real `lines_total`, the byte-cap note when the cap (not `limit`) ended the
/// window, the "past the end of the file" note, the absolute NUL offset of the
/// binary sniff, and the same `meta` keys.
fn read_window(
    abs: &Path,
    rel: &str,
    size: u64,
    offset: usize,
    limit: usize,
    max_bytes: usize,
    cancel: &CancellationToken,
) -> Result<Rendered> {
    let mut file = std::fs::File::open(abs).map_err(|e| Error::io(abs, e))?;
    let mut chunk = vec![0u8; SCAN_CHUNK_BYTES];
    let need = offset - 1;

    // Phase 1: skip `offset - 1` whole lines, retaining nothing but the tail of
    // the chunk that carried the last newline of the prefix.
    let mut skipped = 0usize;
    let mut window_start = 0u64;
    let mut carry: Vec<u8> = Vec::new();
    let mut last_byte: Option<u8> = None;
    let mut reached = need == 0;
    while !reached {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let n = match read_chunk(&mut file, &mut chunk, abs)? {
            Some(n) => n,
            None => break,
        };
        last_byte = Some(chunk[n - 1]);
        let slice = &chunk[..n];
        let mut seen = 0usize;
        let mut cut = n;
        for (i, byte) in slice.iter().enumerate() {
            if *byte != b'\n' {
                continue;
            }
            seen += 1;
            if skipped + seen == need {
                cut = i + 1;
                reached = true;
                break;
            }
        }
        skipped += seen;
        window_start += cut as u64;
        if reached {
            carry.extend_from_slice(&slice[cut..]);
        }
    }

    if !reached {
        // The file ended before the requested line: the same note and shape the
        // prefix path produces for an offset past the end.
        let total = skipped + usize::from(last_byte.is_some_and(|b| b != b'\n'));
        return Ok(Rendered {
            text: past_end_text(rel, size, total, offset),
            truncated: false,
            meta: serde_json::json!({
                "path": rel,
                "lines_total": total,
                "lines_total_is_lower_bound": false,
                "lines_from": total,
                "lines_to": total,
                "bytes": size,
                "byte_limited": false,
                "offset": offset,
                "limit": limit,
            }),
        });
    }

    // Phase 2: the window itself, bounded by `limit` lines and by `max_bytes`
    // returned bytes. Both bounds land on a line boundary where they can, so the
    // window is always a whole number of lines plus, at worst, one partial line
    // when the byte cap falls inside it.
    //
    // `carry` can be as large as one read step, so it is bounded here before it is
    // counted: nothing beyond `max_bytes` is ever returned.
    let mut buf = carry;
    buf.truncate(max_bytes);
    let mut window_newlines = buf.iter().filter(|b| **b == b'\n').count();
    let mut ended_at_eof = false;
    let mut line_limited = false;
    loop {
        if window_newlines >= limit {
            cut_after_newlines(&mut buf, limit);
            line_limited = true;
            break;
        }
        if buf.len() >= max_bytes {
            break;
        }
        let want = (max_bytes - buf.len()).min(chunk.len());
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let n = match read_chunk(&mut file, &mut chunk[..want], abs)? {
            Some(n) => n,
            None => {
                ended_at_eof = true;
                break;
            }
        };
        last_byte = Some(chunk[n - 1]);
        window_newlines += chunk[..n].iter().filter(|b| **b == b'\n').count();
        buf.extend_from_slice(&chunk[..n]);
    }
    // `window_newlines` above counts the bytes that were read; only `buf` is
    // returned, and `cut_after_newlines` may have shortened it.
    let window_newlines = buf.iter().filter(|b| **b == b'\n').count();

    // Sniff the window that is actually returned. The offset reported to the
    // model is the byte offset in the *file*, not in the window.
    if let Some(pos) = buf.iter().position(|b| *b == 0) {
        return Err(Error::BadRequest(format!(
            "`{rel}` looks like a binary file (NUL byte at offset {}); read_file only returns UTF-8 text. \
             Use run_command with `file`/`xxd` if you need to inspect it.",
            window_start + pos as u64
        )));
    }

    // Phase 3: count the rest of the file, bounded by `MAX_COUNTED_LINES`. The
    // read steps ran past the window (a whole chunk at a time), so the count
    // resumes from the window's end, not from the handle's position.
    let window_end = window_start + buf.len() as u64;
    let (tail_newlines, tail_last, capped) = if ended_at_eof || window_end >= size {
        (0usize, None, false)
    } else {
        file.seek(std::io::SeekFrom::Start(window_end))
            .map_err(|e| Error::io(abs, e))?;
        drain_counting(&mut file, abs, cancel)?
    };
    if let Some(byte) = tail_last {
        last_byte = Some(byte);
    }
    let total_newlines = skipped + window_newlines + tail_newlines;
    let total_capped = capped;
    // `str::lines` semantics: a trailing newline does not open a new line, and an
    // empty file has none.
    let unterminated = size > 0 && last_byte.is_some_and(|b| b != b'\n');
    let total = total_newlines + usize::from(unterminated && !capped);

    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().collect();
    let available = lines.len();
    // The cap, not `limit` and not the end of the file, ended this window.
    let byte_limited =
        !line_limited && !ended_at_eof && window_end < size && buf.len() >= max_bytes;

    let start = offset - 1;
    let end = (start + limit).min(total);
    let (first, last) = if available == 0 {
        (total, total)
    } else {
        (offset, offset + available - 1)
    };

    let mut out = String::new();
    let total_label = if total_capped {
        format!("{total}+")
    } else {
        total.to_string()
    };
    out.push_str(&format!(
        "{rel} — lines {first}-{last} of {total_label} ({size} bytes)"
    ));
    if byte_limited {
        out.push_str(&format!(
            "\n[note: only the first {max_bytes} of {size} bytes were read \
             (limits.max_file_read_bytes); raise the limit to see the rest]"
        ));
    }
    if total_capped {
        out.push_str(&format!(
            "\n[note: the line count stopped at {MAX_COUNTED_LINES} lines, so `{total}+` is a \
             lower bound]"
        ));
    }

    if total == 0 {
        out.push_str("\n[file is empty]");
    } else if available == 0 {
        out.push_str(&format!(
            "\n[offset {offset} is past the end of the file ({total} lines); start at or before {total}]"
        ));
    } else {
        for (i, line) in lines.iter().enumerate() {
            out.push_str(&format!("\n{:>5}\t{}", offset + i, line));
        }
        // Only offer "continue with offset" when the window was not cut short by
        // the byte cap; otherwise the note above already explains the limit.
        if last < total && last == end {
            out.push_str(&format!(
                "\n[next {} lines omitted; continue with offset={}]",
                total - last,
                last + 1
            ));
        }
    }

    Ok(Rendered {
        text: out,
        truncated: byte_limited || end < total,
        meta: serde_json::json!({
            "path": rel,
            "lines_total": total,
            "lines_total_is_lower_bound": total_capped,
            "lines_from": first,
            "lines_to": last,
            "bytes": size,
            "byte_limited": byte_limited,
            "offset": offset,
            "limit": limit,
        }),
    })
}

/// Cut `buf` to just after its `limit`-th newline, so a 64 KiB read step cannot
/// return more lines than the caller asked for.
fn cut_after_newlines(buf: &mut Vec<u8>, limit: usize) {
    let mut seen = 0usize;
    let mut cut = buf.len();
    for (i, byte) in buf.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        seen += 1;
        if seen == limit {
            cut = i + 1;
            break;
        }
    }
    buf.truncate(cut);
}

/// The `[offset N is past the end]` body the prefix path renders.
fn past_end_text(rel: &str, size: u64, total: usize, offset: usize) -> String {
    format!(
        "{rel} — lines {total}-{total} of {total} ({size} bytes)\n\
         [offset {offset} is past the end of the file ({total} lines); start at or before {total}]"
    )
}

/// One bounded read, treating end of file as `None` and retrying a signal.
fn read_chunk(file: &mut std::fs::File, buf: &mut [u8], path: &Path) -> Result<Option<usize>> {
    loop {
        match file.read(buf) {
            Ok(0) => return Ok(None),
            Ok(n) => return Ok(Some(n)),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(Error::io(path, e)),
        }
    }
}

/// Keep reading `file` from where the byte-capped prefix left off and return the
/// whole file's line count, plus whether the count stopped at the cap.
///
/// `prefix` is what [`read_file`] already read, so the scan starts there instead of
/// at byte 0 (the previous implementation re-opened the file and re-read it from
/// the beginning, so the prefix was read twice).
///
/// The count is bounded by [`MAX_COUNTED_LINES`]: a pathological file (say a 2 GiB
/// log of short lines) would otherwise cost a full extra scan to produce a number
/// nobody needs. When the bound is hit the caller reports the count as "N+" rather
/// than lying about the total.
///
/// Matches `str::lines` semantics: a trailing newline does not start a new line,
/// and an empty file has zero lines.
fn continue_counting(file: &mut std::fs::File, prefix: &[u8]) -> Result<(usize, bool)> {
    let mut newlines = prefix.iter().filter(|b| **b == b'\n').count();
    let mut last_byte = prefix.last().copied();
    let (tail, tail_last, capped) =
        drain_counting(file, Path::new("<file>"), &CancellationToken::new())?;
    newlines += tail;
    if let Some(byte) = tail_last {
        last_byte = Some(byte);
    }
    // A file that does not end in a newline has one more (partial) line.
    if !capped && last_byte.is_some() && last_byte != Some(b'\n') {
        newlines += 1;
    }
    Ok((newlines, capped))
}

/// Count the newlines from the handle's current position to end of file.
///
/// Returns `(newlines, last byte read, stopped at the cap)`. The cap bounds the
/// work: a count is a nicety, and a 2 GiB log should not be read twice for one.
fn drain_counting(
    file: &mut std::fs::File,
    path: &Path,
    cancel: &CancellationToken,
) -> Result<(usize, Option<u8>, bool)> {
    let mut newlines = 0usize;
    let mut last = None;
    let mut buf = vec![0u8; SCAN_CHUNK_BYTES];
    let mut capped = false;
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let n = match read_chunk(file, &mut buf, path)? {
            Some(n) => n,
            None => break,
        };
        newlines += buf[..n].iter().filter(|b| **b == b'\n').count();
        last = Some(buf[n - 1]);
        if newlines > MAX_COUNTED_LINES {
            capped = true;
            break;
        }
    }
    Ok((newlines, last, capped))
}

/// What a file turned out to be, as far as the vision APIs are concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageProbe {
    /// A PNG/JPEG/GIF/WebP: an image a vision model can be shown.
    Vision {
        rel: String,
        media_type: &'static str,
        size: u64,
    },
    /// A real image in a format the OpenAI-compatible vision APIs reject.
    Unsupported {
        rel: String,
        media_type: &'static str,
        size: u64,
    },
    /// Not an image at all (text, a directory, or some other binary).
    NotAnImage,
}

/// Media types every OpenAI-compatible vision endpoint accepts. Anything else
/// is refused before the request is built, because an endpoint that does not
/// understand a format fails the *whole turn*, not just the image.
pub const VISION_MEDIA_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Classify a workspace file by its **magic bytes**, never by its extension.
///
/// Directories, missing files and permission problems are `NotAnImage`: the
/// plain text path in [`read_file`] produces the precise error for those, and a
/// probe must not change the message a user sees for a non-image.
pub fn probe_image(ws: &Workspace, path: &str) -> Result<ImageProbe> {
    let abs = ws.resolve_existing(path)?;
    let rel = ws.relative(&abs);
    let meta = match std::fs::metadata(&abs) {
        Ok(meta) => meta,
        Err(_) => return Ok(ImageProbe::NotAnImage),
    };
    if !meta.is_file() {
        return Ok(ImageProbe::NotAnImage);
    }
    let head = read_head(&abs, IMAGE_SNIFF_BYTES)?;
    let size = meta.len();
    if let Some(media_type) = sniff_image(&head) {
        return Ok(ImageProbe::Vision {
            rel,
            media_type,
            size,
        });
    }
    if let Some(media_type) = sniff_other_image(&head) {
        return Ok(ImageProbe::Unsupported {
            rel,
            media_type,
            size,
        });
    }
    Ok(ImageProbe::NotAnImage)
}

fn read_head(abs: &Path, max: usize) -> Result<Vec<u8>> {
    let mut file = std::fs::File::open(abs).map_err(|e| Error::io(abs, e))?;
    let mut head = Vec::with_capacity(max);
    Read::by_ref(&mut file)
        .take(max as u64)
        .read_to_end(&mut head)
        .map_err(|e| Error::io(abs, e))?;
    Ok(head)
}

/// Formats the vision APIs accept.
fn sniff_image(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Real images in formats the vision APIs reject, named so the refusal can tell
/// the model what it is looking at instead of "binary file".
fn sniff_other_image(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(b"BM") {
        return Some("image/bmp");
    }
    if head.starts_with(&[0x00, 0x00, 0x01, 0x00]) {
        return Some("image/x-icon");
    }
    if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") {
        return Some("image/tiff");
    }
    // ISO base media (`ftyp`) containers: AVIF and HEIC/HEIF.
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        return match &head[8..12] {
            b"avif" | b"avis" => Some("image/avif"),
            b"heic" | b"heix" | b"heim" | b"heis" | b"mif1" | b"msf1" => Some("image/heic"),
            _ => None,
        };
    }
    None
}

// Read at most one byte beyond the limit, even if the file grows after metadata.
fn read_bounded(reader: impl Read, max_bytes: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Read an image file and encode it for a vision model.
///
/// `media_type` comes from [`probe_image`]; the bytes are read once, so the
/// payload always describes one consistent snapshot of the file.
pub fn read_image(
    ws: &Workspace,
    path: &str,
    media_type: &str,
    max_bytes: usize,
) -> Result<ImageAttachment> {
    let abs = ws.resolve_existing(path)?;
    let rel = ws.relative(&abs);
    let meta = std::fs::metadata(&abs).map_err(|e| Error::io(&abs, e))?;
    if !meta.is_file() {
        return Err(Error::BadRequest(format!(
            "`{rel}` is not a regular file; refusing to read it as an image"
        )));
    }
    if meta.len() > max_bytes as u64 {
        return Err(Error::BadRequest(format!(
            "`{rel}` is {} and limits.max_image_read_bytes is {}; refusing to attach it. \
             Shrink or convert it first (e.g. run_command with `magick`/`convert`/`ffmpeg`) and read \
             the smaller file, or the user can raise the limit.",
            format_size(meta.len()),
            format_size(max_bytes as u64)
        )));
    }
    let file = std::fs::File::open(&abs).map_err(|e| Error::io(&abs, e))?;
    let bytes = read_bounded(file, max_bytes as u64).map_err(|e| Error::io(&abs, e))?;
    if bytes.len() > max_bytes {
        // The file grew between the metadata check and the read.
        return Err(Error::BadRequest(format!(
            "`{rel}` grew to {} while being read, past limits.max_image_read_bytes ({}); \
             refusing to attach it",
            format_size(bytes.len() as u64),
            format_size(max_bytes as u64)
        )));
    }
    use base64::Engine as _;
    Ok(ImageAttachment {
        media_type: media_type.to_string(),
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        bytes: bytes.len() as u64,
        name: Some(rel),
    })
}

/// List a directory tree, honouring `.gitignore` rules.
///
/// `depth` is clamped to `1..=3`; directories sort before files and carry a
/// trailing `/`.
pub fn list_dir(ws: &Workspace, path: &str, depth: u32, max_entries: usize) -> Result<Rendered> {
    let abs = ws.resolve_existing(path)?;
    let rel = ws.relative(&abs);
    let meta = std::fs::metadata(&abs).map_err(|e| Error::io(&abs, e))?;
    if !meta.is_dir() {
        return Err(Error::BadRequest(format!(
            "`{rel}` is a file; use read_file instead of list_dir"
        )));
    }
    let depth = depth.clamp(1, MAX_DEPTH);
    let max_entries = max_entries.max(1);

    let walker = ignore::WalkBuilder::new(&abs)
        .max_depth(Some(depth as usize + 1))
        .hidden(false)
        .require_git(false)
        .git_global(false)
        .follow_links(false)
        .filter_entry(|entry| !is_skipped_dir(entry.file_name()))
        .build();

    struct Item {
        is_dir: bool,
        rel: String,
        size: u64,
    }
    // The walk stops one entry past the cap. Collecting the whole tree and
    // truncating afterwards meant a directory of 1 000 files walked 1,000
    // `readdir` entries and stat'ed every one of them to show 500 (verified:
    // 100,200 entries walked for a 500-entry cap, 249 ms, +10 MB RSS). `readdir`
    // order is arbitrary, so the *set* that fits in the cap is not the
    // alphabetically-first one; the entries that are shown stay sorted, and the
    // truncation note says the list was cut.
    let mut items: Vec<Item> = Vec::with_capacity(max_entries.min(1024) + 1);
    let mut skipped_errors = 0usize;
    let mut capped = false;
    for entry in walker {
        // `max_entries + 1` so the caller still learns that there *was* more.
        if items.len() > max_entries {
            capped = true;
            break;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                skipped_errors += 1;
                continue;
            }
        };
        if entry.depth() == 0 {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let rel = ws.relative(entry.path());
        let size = if is_dir {
            0
        } else {
            entry.metadata().map(|m| m.len()).unwrap_or(0)
        };
        items.push(Item { is_dir, rel, size });
    }
    if items.len() > max_entries {
        capped = true;
    }

    items.sort_by(|a, b| (!a.is_dir, &a.rel).cmp(&(!b.is_dir, &b.rel)));
    // `entries` is how many entries the walk collected: the loop breaks at the top of
    // the iteration that finds more than the cap, so it stops at `max_entries + 1`
    // - enough to know there was more, without walking the rest of the tree.
    let entries = items.len();
    let truncated = capped || entries > max_entries;
    items.truncate(max_entries);
    // With `capped` the directory's true size is unknown, so the total is a lower
    // bound rather than a number the walk never established.
    let total_label = if capped {
        format!("{}+", max_entries)
    } else {
        entries.to_string()
    };

    let mut out = format!(
        "Listing {rel} (depth {depth}, {total_label} entr{}, {} shown)",
        if items.len() == 1 { "y" } else { "ies" },
        items.len()
    );
    if items.is_empty() {
        out.push_str("\n[empty]");
    }
    for item in &items {
        if item.is_dir {
            out.push_str(&format!("\n{}/", item.rel));
        } else {
            out.push_str(&format!("\n{} ({})", item.rel, format_size(item.size)));
        }
    }
    if truncated {
        out.push_str(&format!(
            "\n[truncated: showing the first {max_entries} of {total_label} entries; \
             raise the path or depth narrowing instead of increasing the cap]"
        ));
    }
    if skipped_errors > 0 {
        out.push_str(&format!(
            "\n[{skipped_errors} entries could not be read and were skipped]"
        ));
    }

    Ok(Rendered {
        text: out,
        truncated,
        meta: serde_json::json!({
            "path": rel,
            "depth": depth,
            // With `capped` this is how many entries were walked, not how many
            // the directory has; `entries_capped` says which.
            "entries": entries,
            "entries_capped": capped,
            "shown": items.len(),
            "errors": skipped_errors,
        }),
    })
}

fn is_skipped_dir(name: &std::ffi::OsStr) -> bool {
    name == ".git" || name == "node_modules"
}

/// Cap on the paths one `glob` call returns.
pub const MAX_GLOB_ENTRIES: usize = 200;

/// Find files and directories by *name* with a glob, `.gitignore`-aware.
///
/// The walker is the one [`list_dir`] and the search fallback already use, so
/// ignored build output is not listed and `.git`/`node_modules` are skipped. The
/// pattern is matched against an entry's file name, its path relative to `path`
/// and its workspace-relative path, so `*.rs`, `src/**/*.rs` and
/// `crates/*/src/lib.rs` all mean what a reader expects.
///
/// The walk stops at `max_entries + 1` entries: the extra one is what proves the
/// cap cut something, so the note can say so instead of implying that is all
/// there was.
pub fn glob(ws: &Workspace, pattern: &str, path: &str, max_entries: usize) -> Result<Rendered> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Err(Error::BadRequest(
            "glob needs a `pattern`, e.g. \"*.rs\" or \"src/**/*.toml\"".into(),
        ));
    }
    let set = globset::Glob::new(pattern)
        .map_err(|e| Error::BadRequest(format!("invalid glob {pattern:?}: {e}")))
        .and_then(|glob| {
            let mut builder = globset::GlobSetBuilder::new();
            builder.add(glob);
            builder
                .build()
                .map_err(|e| Error::BadRequest(format!("invalid glob {pattern:?}: {e}")))
        })?;

    let abs = ws.resolve_existing(path)?;
    let rel_root = ws.relative(&abs);
    let meta = std::fs::metadata(&abs).map_err(|e| Error::io(&abs, e))?;
    if !meta.is_dir() {
        return Err(Error::BadRequest(format!(
            "`{rel_root}` is a file; glob matches names under a directory, so pass its parent \
             (or use read_file / search_files for one file)"
        )));
    }
    let max_entries = max_entries.max(1);

    let walker = ignore::WalkBuilder::new(&abs)
        .hidden(false)
        .require_git(false)
        .git_global(false)
        .follow_links(false)
        .filter_entry(|entry| !is_skipped_dir(entry.file_name()))
        .build();

    struct Found {
        is_dir: bool,
        rel: String,
    }
    let mut found: Vec<Found> = Vec::new();
    let mut walk_errors = 0usize;
    let mut capped = false;
    for entry in walker {
        if found.len() > max_entries {
            capped = true;
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                walk_errors += 1;
                continue;
            }
        };
        if entry.depth() == 0 {
            continue;
        }
        let rel = ws.relative(entry.path());
        let name = entry.file_name().to_string_lossy().to_string();
        let under_root = rel
            .strip_prefix(rel_root.trim_end_matches('/'))
            .map(|rest| rest.trim_start_matches('/'))
            .unwrap_or(rel.as_str())
            .to_string();
        if !set.is_match(&name) && !set.is_match(&under_root) && !set.is_match(&rel) {
            continue;
        }
        found.push(Found {
            is_dir: entry.file_type().map(|t| t.is_dir()).unwrap_or(false),
            rel,
        });
    }
    if found.len() > max_entries {
        capped = true;
    }
    found.sort_by(|a, b| a.rel.cmp(&b.rel));
    let total = found.len();
    found.truncate(max_entries);

    let mut out = format!(
        "{rel_root}: {} path(s) matching `{pattern}`",
        if capped {
            format!("{}+", max_entries)
        } else {
            total.to_string()
        }
    );
    if found.is_empty() {
        out.push_str(
            "\n[no path matches this glob; the pattern is matched against names and paths]",
        );
    }
    for item in &found {
        if item.is_dir {
            out.push_str(&format!("\n{}/", item.rel));
        } else {
            out.push_str(&format!("\n{}", item.rel));
        }
    }
    if capped {
        out.push_str(&format!(
            "\n[truncated: showing the first {max_entries} of {max_entries}+ matching paths; \
             narrow the pattern or the path]"
        ));
    }
    if walk_errors > 0 {
        out.push_str(&format!(
            "\n[{walk_errors} paths could not be read and were skipped]"
        ));
    }

    Ok(Rendered {
        text: out,
        truncated: capped,
        meta: serde_json::json!({
            "pattern": pattern,
            "path": rel_root,
            "matches": total,
            "shown": found.len(),
            "capped": capped,
            "errors": walk_errors,
        }),
    })
}

/// Human-readable byte count, kept short so listings stay scannable.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Write `bytes` to `abs` atomically: a temporary file in the same directory is
/// written, flushed and then renamed over the target.
pub fn write_atomic(abs: &Path, bytes: &[u8]) -> Result<()> {
    let parent = match abs.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    std::fs::create_dir_all(&parent).map_err(|e| Error::io(&parent, e))?;

    let name = abs
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let existing_perms = std::fs::metadata(abs).ok().map(|m| m.permissions());
    let tmp = parent.join(format!(
        ".{name}.kc-tmp-{}-{}",
        std::process::id(),
        next_seq()
    ));

    let write_result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::io(&tmp, e));
    }
    if let Some(perms) = existing_perms {
        // Best effort: a rename would otherwise reset the mode to the umask.
        let _ = std::fs::set_permissions(&tmp, perms);
    }
    if let Err(e) = std::fs::rename(&tmp, abs) {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::io(abs, e));
    }
    Ok(())
}

fn next_seq() -> u64 {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    seq.wrapping_mul(1_000_000_007).wrapping_add(nanos)
}

/// A validated edit, ready to be approved and written.
#[derive(Debug)]
pub struct EditPlan {
    pub abs: PathBuf,
    pub rel: String,
    /// SHA-256 of the exact source bytes this plan and approval describe.
    pub fingerprint: [u8; 32],
    pub new_content: String,
    pub occurrences: usize,
    /// 1-based line numbers (in the original file) that changed.
    pub line_numbers: Vec<usize>,
    pub diff: String,
}

/// Validate an exact-string replacement and build the new content plus a
/// compact diff preview. No file is modified here.
pub fn plan_edit(
    ws: &Workspace,
    path: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<EditPlan> {
    let abs = ws.resolve_existing(path)?;
    let rel = ws.relative(&abs);
    let meta = std::fs::metadata(&abs).map_err(|e| Error::io(&abs, e))?;
    if meta.is_dir() {
        return Err(Error::BadRequest(format!(
            "`{rel}` is a directory; edit_file needs a file"
        )));
    }
    if meta.len() > MAX_EDIT_BYTES {
        return Err(Error::BadRequest(format!(
            "`{rel}` is {} bytes; edit_file supports files up to {} MiB. \
             Use write_file to replace the whole file instead.",
            meta.len(),
            MAX_EDIT_BYTES / (1024 * 1024)
        )));
    }
    if old.is_empty() {
        return Err(Error::BadRequest(
            "old_string may not be empty; pass the exact text to replace".into(),
        ));
    }

    let file = std::fs::File::open(&abs).map_err(|e| Error::io(&abs, e))?;
    let bytes = read_bounded(file, MAX_EDIT_BYTES).map_err(|e| Error::io(&abs, e))?;
    if bytes.len() as u64 > MAX_EDIT_BYTES {
        return Err(Error::BadRequest(format!(
            "`{rel}` exceeds the {} MiB edit_file limit while being read",
            MAX_EDIT_BYTES / (1024 * 1024)
        )));
    }
    if bytes.iter().take(BINARY_SNIFF_BYTES).any(|b| *b == 0) {
        return Err(Error::BadRequest(format!(
            "`{rel}` looks like a binary file (NUL byte in the first {BINARY_SNIFF_BYTES} bytes); \
             edit_file only handles UTF-8 text"
        )));
    }
    let content = String::from_utf8(bytes).map_err(|_| {
        Error::BadRequest(format!(
            "`{rel}` is not valid UTF-8; edit_file only handles UTF-8 text"
        ))
    })?;

    let fingerprint = Sha256::digest(content.as_bytes()).into();
    let indices: Vec<usize> = content.match_indices(old).map(|(i, _)| i).collect();
    if indices.is_empty() {
        return Err(Error::Tool(format!(
            "old_string not found in `{rel}`. First 200 chars of old_string: {:?}. \
             Re-read the file and copy the exact text, including whitespace and line endings.",
            preview(old, 200)
        )));
    }
    // The ambiguity refusal needs only the match *count* and the first few line
    // numbers, so it runs before the line numbers for every match are computed.
    // This used to be the other way round: `content[..i].bytes().filter(..).count()`
    // per match is O(occurrences × bytes), which measured 5.8 s on a 633 KB file
    // and 79.6 s at the 1 MiB mark - paid *before* the cheap refusal that then
    // threw the work away.
    if indices.len() > 1 && !replace_all {
        let shown = indices.len().min(MAX_REPORTED_LINES);
        // Only scan up to the last match whose line number is reported: the
        // matches are ordered, so the newlines before it are all that is needed.
        let line_numbers = line_numbers_within(&content, &indices[..shown]);
        let listed: Vec<String> = line_numbers.iter().map(|n| n.to_string()).collect();
        return Err(Error::Tool(format!(
            "old_string matches {} places in `{rel}` (lines {}). Add surrounding context to make it unique, \
             or pass replace_all: true.",
            indices.len(),
            listed.join(", ")
        )));
    }

    // Check the exact result size before allocating or expanding replacements.
    let result_len = indices
        .len()
        .checked_mul(old.len())
        .and_then(|removed| content.len().checked_sub(removed))
        .and_then(|remaining| {
            indices
                .len()
                .checked_mul(new.len())
                .and_then(|added| remaining.checked_add(added))
        })
        .filter(|size| *size as u64 <= MAX_EDIT_BYTES)
        .ok_or_else(|| {
            Error::BadRequest(format!(
                "replacement result exceeds the {} MiB edit_file limit",
                MAX_EDIT_BYTES / (1024 * 1024)
            ))
        })?;
    let line_numbers = line_numbers_within(&content, &indices);

    let mut new_content = String::with_capacity(result_len);
    let mut last = 0usize;
    for idx in &indices {
        new_content.push_str(&content[last..*idx]);
        new_content.push_str(new);
        last = idx + old.len();
    }
    new_content.push_str(&content[last..]);

    let old_lines: Vec<&str> = content.lines().collect();
    let new_lines: Vec<&str> = new_content.lines().collect();
    let diff = render_diff(&old_lines, &new_lines, DIFF_MAX_LINES);

    Ok(EditPlan {
        abs,
        rel,
        fingerprint,
        new_content,
        occurrences: indices.len(),
        line_numbers,
        diff,
    })
}

/// 1-based line numbers of ascending byte `indices` into `content`.
///
/// One cursor moves forward through the source. There is deliberately no table
/// of every newline: an 8 MiB file of dense one-character lines would otherwise
/// allocate another ~64 MiB before it could even start the edit.
fn line_numbers_within(content: &str, indices: &[usize]) -> Vec<usize> {
    let bytes = content.as_bytes();
    let mut cursor = 0usize;
    let mut line = 1usize;
    indices
        .iter()
        .map(|idx| {
            debug_assert!(*idx >= cursor && *idx <= bytes.len());
            line += bytes[cursor..*idx].iter().filter(|b| **b == b'\n').count();
            cursor = *idx;
            line
        })
        .collect()
}

/// First `max_chars` characters of `s`, with an ellipsis when shortened.
pub fn preview(s: &str, max_chars: usize) -> String {
    let mut out: String = s.chars().take(max_chars).collect();
    if s.chars().count() > max_chars {
        out.push('…');
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffOp {
    Eq,
    Del,
    Ins,
}

impl DiffOp {
    fn sigil(self) -> char {
        match self {
            DiffOp::Eq => ' ',
            DiffOp::Del => '-',
            DiffOp::Ins => '+',
        }
    }
}

/// Compact unified-diff-style preview, capped at `max_lines` output lines.
pub fn render_diff(old: &[&str], new: &[&str], max_lines: usize) -> String {
    let mut prefix = 0usize;
    while prefix < old.len() && prefix < new.len() && old[prefix] == new[prefix] {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < old.len().saturating_sub(prefix)
        && suffix < new.len().saturating_sub(prefix)
        && old[old.len() - 1 - suffix] == new[new.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let mid_old = &old[prefix..old.len() - suffix];
    let mid_new = &new[prefix..new.len() - suffix];
    if mid_old.is_empty() && mid_new.is_empty() {
        return String::from("[no textual change]");
    }

    // A dense replace_all commonly changes every line. Rendering that as a block
    // needs only the 80 lines we actually return; constructing millions of diff
    // ops/positions first can dwarf the source and new content. Empty sides are
    // included here as well: their mathematical n*m product is zero, which used
    // to smuggle an arbitrarily wide, useless LCS allocation past the cell cap.
    if !use_lcs(mid_old.len(), mid_new.len()) {
        return render_block_diff(mid_old, mid_new, prefix, max_lines);
    }

    let ops = diff_ops(mid_old, mid_new);
    // Line numbers in the original file for every op (prefix lines are equal).
    let mut positions: Vec<(usize, usize)> = Vec::with_capacity(ops.len());
    let (mut old_no, mut new_no) = (prefix + 1, prefix + 1);
    for (op, _) in &ops {
        positions.push((old_no, new_no));
        match op {
            DiffOp::Eq => {
                old_no += 1;
                new_no += 1;
            }
            DiffOp::Del => old_no += 1,
            DiffOp::Ins => new_no += 1,
        }
    }

    let mut include = vec![false; ops.len()];
    for (idx, (op, _)) in ops.iter().enumerate() {
        if *op == DiffOp::Eq {
            continue;
        }
        let lo = idx.saturating_sub(DIFF_CONTEXT_LINES);
        let hi = (idx + DIFF_CONTEXT_LINES).min(ops.len() - 1);
        for slot in include.iter_mut().take(hi + 1).skip(lo) {
            *slot = true;
        }
    }

    let mut out = String::new();
    let mut emitted = 0usize;
    let mut truncated = false;
    let mut idx = 0usize;
    'outer: while idx < ops.len() {
        if !include[idx] {
            idx += 1;
            continue;
        }
        let start = idx;
        while idx < ops.len() && include[idx] {
            idx += 1;
        }
        let hunk = &ops[start..idx];
        let old_count = hunk.iter().filter(|(op, _)| *op != DiffOp::Ins).count();
        let new_count = hunk.iter().filter(|(op, _)| *op != DiffOp::Del).count();
        let header = format!(
            "@@ -{},{} +{},{} @@",
            positions[start].0, old_count, positions[start].1, new_count
        );
        if emitted >= max_lines {
            truncated = true;
            break;
        }
        out.push_str(&header);
        emitted += 1;
        for (op, line) in hunk {
            if emitted >= max_lines {
                truncated = true;
                break 'outer;
            }
            out.push('\n');
            out.push(op.sigil());
            out.push_str(line);
            emitted += 1;
        }
    }
    if truncated {
        out.push_str(&format!(
            "\n[... diff preview truncated at {max_lines} lines]"
        ));
    }
    out
}

/// LCS is useful only for two non-empty, modest changed regions. The empty-side
/// check must precede the cell product: zero times a huge side is not permission
/// to allocate a giant table for a pure insertion/deletion preview.
fn use_lcs(n: usize, m: usize) -> bool {
    n > 0 && m > 0 && lcs_table_cells(n, m) <= DIFF_MAX_CELLS
}

/// Number of cells including the sentinel row and column.
fn lcs_table_cells(n: usize, m: usize) -> usize {
    n.saturating_add(1)
        .max(m.saturating_add(1))
        .saturating_mul(n.saturating_add(1).min(m.saturating_add(1)))
}

/// Render a whole changed region without constructing an operation vector.
fn render_block_diff(old: &[&str], new: &[&str], prefix: usize, max_lines: usize) -> String {
    let mut out = format!(
        "@@ -{},{} +{},{} @@",
        prefix + 1,
        old.len(),
        prefix + 1,
        new.len()
    );
    let mut emitted = 1usize;
    let mut truncated = false;
    for (sigil, lines) in [('-', old), ('+', new)] {
        for line in lines {
            if emitted >= max_lines {
                truncated = true;
                break;
            }
            out.push('\n');
            out.push(sigil);
            out.push_str(line);
            emitted += 1;
        }
        if truncated {
            break;
        }
    }
    if truncated {
        out.push_str(&format!(
            "\n[... diff preview truncated at {max_lines} lines]"
        ));
    }
    out
}

fn diff_ops<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<(DiffOp, &'a str)> {
    let (n, m) = (old.len(), new.len());
    if !use_lcs(n, m) {
        let mut ops = Vec::with_capacity(n + m);
        ops.extend(old.iter().map(|l| (DiffOp::Del, *l)));
        ops.extend(new.iter().map(|l| (DiffOp::Ins, *l)));
        return ops;
    }
    let width = m + 1;
    let mut table = vec![0u32; lcs_table_cells(n, m)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i * width + j] = if old[i] == new[j] {
                table[(i + 1) * width + j + 1] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }
    let mut ops = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if old[i] == new[j] {
            ops.push((DiffOp::Eq, old[i]));
            i += 1;
            j += 1;
        } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
            ops.push((DiffOp::Del, old[i]));
            i += 1;
        } else {
            ops.push((DiffOp::Ins, new[j]));
            j += 1;
        }
    }
    while i < n {
        ops.push((DiffOp::Del, old[i]));
        i += 1;
    }
    while j < m {
        ops.push((DiffOp::Ins, new[j]));
        j += 1;
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempWs {
        path: PathBuf,
    }

    impl TempWs {
        fn new(tag: &str) -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "kc-tools-fileops-{tag}-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn write(&self, rel: &str, content: &str) {
            self.write_bytes(rel, content.as_bytes());
        }

        fn write_bytes(&self, rel: &str, bytes: &[u8]) {
            let path = self.path.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, bytes).unwrap();
        }

        fn ws(&self) -> Workspace {
            Workspace::new(&self.path).unwrap()
        }
    }

    impl Drop for TempWs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// Bug 4: `total` used to be counted from the *truncated* buffer, so a file
    /// over `max_file_read_bytes` was reported as far shorter than it is (a
    /// 40 000-line file claimed to be 7 408 lines) and the "next N lines" hint
    /// was computed from that wrong number.
    #[test]
    fn a_byte_limited_read_reports_the_real_total_line_count() {
        let tmp = TempWs::new("byte-cap-total");
        let lines = 40_000usize;
        let body: String = (1..=lines).map(|i| format!("row {i}\n")).collect();
        tmp.write("big.txt", &body);
        let ws = tmp.ws();
        let size = body.len();
        assert!(size > 200_000, "the fixture must exceed the default cap");

        let rendered = read_file(&ws, "big.txt", None, None, 200_000).unwrap();
        assert!(rendered.meta["byte_limited"].as_bool().unwrap());
        assert_eq!(
            rendered.meta["lines_total"].as_u64().unwrap(),
            lines as u64,
            "lines_total must describe the whole file, not the bytes that were read"
        );
        assert!(
            rendered
                .text
                .starts_with(&format!("big.txt — lines 1-400 of {lines} ({size} bytes)")),
            "header must carry the real total: {:?}",
            rendered.text.lines().next()
        );
        // The 400-line window ended this read (not the byte cap), so the hint is
        // valid and must be computed from the real total.
        assert!(
            rendered.text.contains(&format!(
                "[next {} lines omitted; continue with offset=401]",
                lines - 400
            )),
            "the omitted-line hint must use the real total: {:?}",
            rendered.text.lines().last()
        );
        assert!(rendered.text.contains("max_file_read_bytes"));
        assert!(rendered.truncated);
    }

    /// When the *byte* cap is what cut the window short, an "offset=" hint would
    /// promise lines that cannot be read: it must be suppressed.
    #[test]
    fn a_byte_capped_window_does_not_promise_more_lines_via_offset() {
        let tmp = TempWs::new("byte-cap-window");
        let lines = 40_000usize;
        let body: String = (1..=lines).map(|i| format!("row {i}\n")).collect();
        tmp.write("big.txt", &body);
        let ws = tmp.ws();

        // 1 000 bytes is ~125 lines, far short of the 2 000-line window.
        let rendered = read_file(&ws, "big.txt", None, Some(2000), 1000).unwrap();
        assert!(rendered.meta["byte_limited"].as_bool().unwrap());
        assert_eq!(rendered.meta["lines_total"].as_u64().unwrap(), lines as u64);
        assert!(
            !rendered.text.contains("continue with offset="),
            "a byte-capped read must not promise more lines via offset: {:?}",
            rendered.text.lines().last()
        );
        assert!(rendered.text.contains("max_file_read_bytes"));
    }

    #[test]
    fn bounded_read_stops_after_one_extra_byte_even_for_an_unending_reader() {
        let bytes = read_bounded(std::io::repeat(b'x'), 32).unwrap();
        assert_eq!(bytes.len(), 33);
        assert_eq!(read_bounded(&b"short"[..], 5).unwrap(), b"short");
    }

    /// The measured failure: a file over `max_file_read_bytes` could never be read
    /// past the lines that fit in the first 200 000 bytes, so a 9 774-line file was
    /// unreachable past line 4 334 and the model fell back to `sed -n`.
    #[test]
    fn a_window_past_the_byte_cap_returns_the_real_lines() {
        let tmp = TempWs::new("window-past-cap");
        let lines = 30_000usize;
        let body: String = (1..=lines).map(|i| format!("row {i}\n")).collect();
        tmp.write("big.txt", &body);
        let size = body.len();
        assert!(size > 200_000, "the fixture must exceed the default cap");
        let ws = tmp.ws();

        let rendered = read_file(&ws, "big.txt", Some(25_000), Some(3), 200_000).unwrap();
        assert_eq!(rendered.meta["lines_total"].as_u64().unwrap(), lines as u64);
        assert_eq!(rendered.meta["lines_from"].as_u64().unwrap(), 25_000);
        assert_eq!(rendered.meta["lines_to"].as_u64().unwrap(), 25_002);
        assert!(
            !rendered.meta["byte_limited"].as_bool().unwrap(),
            "the cap must bound the bytes returned, not what is reachable"
        );
        assert!(
            rendered.text.starts_with(&format!(
                "big.txt — lines 25000-25002 of {lines} ({size} bytes)"
            )),
            "header: {:?}",
            rendered.text.lines().next()
        );
        assert!(
            rendered.text.contains("25000\trow 25000"),
            "{}",
            rendered.text
        );
        assert!(
            rendered.text.contains("25002\trow 25002"),
            "{}",
            rendered.text
        );
        assert!(!rendered.text.contains("row 24999"), "{}", rendered.text);
        assert!(!rendered.text.contains("row 25003"), "{}", rendered.text);
        // 25 000 lines remain, so the next-window hint points at the real line.
        assert!(
            rendered.text.contains(&format!(
                "[next {} lines omitted; continue with offset=25003]",
                lines - 25_002
            )),
            "{:?}",
            rendered.text.lines().last()
        );
    }

    /// A window that *starts* inside the byte cap and runs past it still ends where
    /// either the line window or the byte cap says, with the cap note attached.
    #[test]
    fn a_window_inside_the_cap_still_obeys_the_byte_cap() {
        let tmp = TempWs::new("window-byte-cap");
        let lines = 30_000usize;
        let body: String = (1..=lines).map(|i| format!("row {i}\n")).collect();
        tmp.write("big.txt", &body);
        let ws = tmp.ws();

        // 1 000 bytes of returned window cannot hold the 2 000 lines asked for.
        let rendered = read_file(&ws, "big.txt", Some(100), Some(2000), 1000).unwrap();
        assert!(rendered.meta["byte_limited"].as_bool().unwrap());
        assert_eq!(rendered.meta["lines_from"].as_u64().unwrap(), 100);
        let shown = rendered.meta["lines_to"].as_u64().unwrap();
        assert!(
            shown > 100 && shown < 1099,
            "byte cap cut the window: {shown}"
        );
        assert_eq!(rendered.meta["lines_total"].as_u64().unwrap(), lines as u64);
        assert!(rendered.text.contains("max_file_read_bytes"));
        assert!(!rendered.text.contains("continue with offset="));

        // The next window is still reachable, one call later.
        let next = read_file(&ws, "big.txt", Some(shown + 1), Some(2), 200_000).unwrap();
        assert_eq!(
            next.meta["lines_from"].as_u64().unwrap(),
            shown + 1,
            "{}",
            next.text
        );
    }

    /// Past the end is a note, not a panic and not a wrong line number, whichever
    /// path produced it.
    #[test]
    fn a_window_past_the_end_reports_the_real_total() {
        let tmp = TempWs::new("window-past-end");
        let lines = 30_000usize;
        let body: String = (1..=lines).map(|i| format!("row {i}\n")).collect();
        tmp.write("big.txt", &body);
        let ws = tmp.ws();

        let rendered = read_file(&ws, "big.txt", Some(lines as u64 + 10), None, 200_000).unwrap();
        assert_eq!(rendered.meta["lines_total"].as_u64().unwrap(), lines as u64);
        assert_eq!(rendered.meta["lines_from"].as_u64().unwrap(), lines as u64);
        assert_eq!(rendered.meta["lines_to"].as_u64().unwrap(), lines as u64);
        assert!(
            rendered.text.contains(&format!(
                "[offset {} is past the end of the file ({lines} lines); start at or before {lines}]",
                lines + 10
            )),
            "{}",
            rendered.text
        );
        assert!(!rendered.truncated);
    }

    /// A file with no trailing newline still counts, and reads, its last line.
    #[test]
    fn a_window_reaches_an_unterminated_last_line() {
        let tmp = TempWs::new("window-unterminated");
        let lines = 30_000usize;
        let mut body: String = (1..lines).map(|i| format!("row {i}\n")).collect();
        body.push_str("last row");
        tmp.write("big.txt", &body);
        let ws = tmp.ws();

        let rendered = read_file(&ws, "big.txt", Some(lines as u64), Some(5), 200_000).unwrap();
        assert_eq!(rendered.meta["lines_total"].as_u64().unwrap(), lines as u64);
        assert_eq!(rendered.meta["lines_from"].as_u64().unwrap(), lines as u64);
        assert!(rendered.text.contains("last row"), "{}", rendered.text);
    }

    /// The binary sniff must report the offset in the *file*, even when the window
    /// starts deep inside it: an offset relative to the window would point the model
    /// (and a later `xxd`) at the wrong byte.
    #[test]
    fn a_nul_in_a_far_window_is_reported_at_its_file_offset() {
        let tmp = TempWs::new("window-nul");
        let mut body: Vec<u8> = Vec::new();
        for i in 1..=30_000 {
            body.extend_from_slice(format!("row {i}\n").as_bytes());
        }
        let nul_at = body.len() as u64;
        body.push(0);
        body.extend_from_slice(b"end\n");
        tmp.write_bytes("big.bin", &body);
        let ws = tmp.ws();

        let err = read_file(&ws, "big.bin", Some(29_999), Some(10), 200_000).unwrap_err();
        assert!(
            err.to_string()
                .contains(&format!("NUL byte at offset {nul_at}")),
            "{err}"
        );
    }

    /// The window is a *window*: the byte cap cannot be used to return more lines
    /// than `limit` just because the read step is 64 KiB wide.
    #[test]
    fn a_window_never_returns_more_lines_than_limit() {
        let tmp = TempWs::new("window-limit");
        let lines = 60_000usize;
        let body: String = (1..=lines).map(|i| format!("row {i}\n")).collect();
        tmp.write("big.txt", &body);
        let ws = tmp.ws();

        let rendered = read_file(&ws, "big.txt", Some(2000), Some(5), 200_000).unwrap();
        assert_eq!(rendered.meta["lines_from"].as_u64().unwrap(), 2000);
        assert_eq!(rendered.meta["lines_to"].as_u64().unwrap(), 2004);
        let numbered = rendered
            .text
            .lines()
            .filter(|line| line.contains("\trow 20"))
            .count();
        assert_eq!(
            numbered, 5,
            "exactly five numbered lines: {:?}",
            rendered.text
        );
        assert!(!rendered.text.contains("row 2005"), "{}", rendered.text);
    }

    #[test]
    fn replacement_size_is_checked_before_expansion() {
        let tmp = TempWs::new("edit-amplification");
        tmp.write("a.txt", "xx");
        let ws = tmp.ws();
        let replacement = "a".repeat(MAX_EDIT_BYTES as usize / 2 + 1);
        let err = plan_edit(&ws, "a.txt", "x", &replacement, true).unwrap_err();
        assert!(err.to_string().contains("replacement result exceeds"));
        assert_eq!(
            std::fs::read_to_string(ws.resolve_existing("a.txt").unwrap()).unwrap(),
            "xx"
        );
        let exact = "a".repeat(MAX_EDIT_BYTES as usize / 2);
        assert_eq!(
            plan_edit(&ws, "a.txt", "x", &exact, true)
                .unwrap()
                .new_content
                .len(),
            MAX_EDIT_BYTES as usize
        );
    }

    /// Line numbers must be right across many matches, not just the first: the old
    /// per-match rescan of `content[..i]` was also what made the ambiguity path
    /// quadratic, so a wrong answer here is easy to miss.
    #[test]
    fn edit_line_numbers_are_right_for_every_match() {
        // Matches on lines 1, 3, 4 and 7, with matches sharing a line too.
        let body = "needle\nfiller\nneedle\nneedle needle\nfiller\nfiller\nneedle\n";
        let tmp = TempWs::new("edit-lines");
        tmp.write("a.txt", body);
        let ws = tmp.ws();

        let plan = plan_edit(&ws, "a.txt", "needle", "pin", true).unwrap();
        assert_eq!(plan.occurrences, 5);
        assert_eq!(
            plan.line_numbers,
            vec![1, 3, 4, 4, 7],
            "every match keeps its own 1-based line number"
        );
        assert_eq!(
            plan.new_content,
            "pin\nfiller\npin\npin pin\nfiller\nfiller\npin\n"
        );

        // The single-match path answers the same.
        let plan = plan_edit(&ws, "a.txt", "filler", "stuff", true).unwrap();
        assert_eq!(plan.line_numbers, vec![2, 5, 6]);
    }

    /// CRLF content must keep its line endings, and a BOM must survive an edit: the
    /// whole file is rewritten from the decoded string, so anything the decoder
    /// normalises would be lost.
    #[test]
    fn edits_preserve_crlf_and_a_byte_order_mark() {
        let tmp = TempWs::new("edit-crlf-bom");
        tmp.write("crlf.txt", "alpha\r\nbeta\r\ngamma\r\n");
        tmp.write_bytes("bom.txt", "\u{feff}first\nsecond\n".as_bytes());
        let ws = tmp.ws();

        let plan = plan_edit(&ws, "crlf.txt", "beta", "BETA", false).unwrap();
        assert_eq!(plan.line_numbers, vec![2]);
        assert_eq!(
            plan.new_content, "alpha\r\nBETA\r\ngamma\r\n",
            "CRLF line endings must be preserved byte-for-byte"
        );
        write_atomic(&plan.abs, plan.new_content.as_bytes()).unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.path.join("crlf.txt")).unwrap(),
            "alpha\r\nBETA\r\ngamma\r\n"
        );

        let plan = plan_edit(&ws, "bom.txt", "second", "SECOND", false).unwrap();
        assert_eq!(plan.line_numbers, vec![2]);
        assert!(
            plan.new_content.starts_with('\u{feff}'),
            "the byte order mark must survive: {:?}",
            plan.new_content
        );
        write_atomic(&plan.abs, plan.new_content.as_bytes()).unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.path.join("bom.txt")).unwrap(),
            "\u{feff}first\nSECOND\n"
        );
    }

    /// FIX 2 timing guard: the ambiguity refusal used to compute a line number for
    /// *every* match before refusing - O(occurrences x bytes), measured at 5.8 s for
    /// a 633 KB file and 79.6 s at 1 MiB. The refusal now needs only the match count
    /// and the first few line numbers, and line numbers themselves come from one
    /// forward pass.
    ///
    /// The budget is deliberately enormous (a correct implementation finishes in
    /// milliseconds; the old one takes minutes here) so this cannot flake on a loaded
    /// machine while still failing loudly if the quadratic scan comes back.
    #[test]
    fn a_huge_number_of_matches_is_refused_promptly() {
        let tmp = TempWs::new("edit-many-matches");
        // 640 KB, 40 000 lines, each with one match.
        let lines = 40_000usize;
        let body: String = (0..lines).map(|_| "needle in this row\n").collect();
        assert!(body.len() > 600_000, "the fixture must be large");
        tmp.write("many.txt", &body);
        let ws = tmp.ws();

        let started = std::time::Instant::now();
        let err = plan_edit(&ws, "many.txt", "needle", "pin", false).unwrap_err();
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "the ambiguous refusal must not compute every match's line number first; \
         took {elapsed:?} for {lines} matches"
        );
        let message = err.to_string();
        assert!(message.contains("matches 40000 places"), "{message}");
        // The refusal still names line numbers, and only the ones it shows.
        assert!(
            message.contains("lines 1, 2, 3, 4, 5, 6, 7, 8, 9, 10)"),
            "{message}"
        );
    }

    /// The same guard for the `replace_all` path, which does have to touch every
    /// match but must still be linear in the file size.
    #[test]
    fn a_large_replace_all_is_linear() {
        let tmp = TempWs::new("edit-replace-all-linear");
        let lines = 40_000usize;
        let body: String = (0..lines).map(|_| "needle in this row\n").collect();
        tmp.write("many.txt", &body);
        let ws = tmp.ws();

        let started = std::time::Instant::now();
        let plan = plan_edit(&ws, "many.txt", "needle", "pin", true).unwrap();
        let elapsed = started.elapsed();
        assert_eq!(plan.occurrences, lines);
        assert_eq!(plan.line_numbers.first(), Some(&1));
        assert_eq!(plan.line_numbers.last(), Some(&lines));
        assert!(
            elapsed < std::time::Duration::from_secs(20),
            "replace_all must stay linear in the file size; took {elapsed:?}"
        );
    }

    /// The counter must agree with `str::lines` on the awkward shapes, whether the
    /// prefix is empty (the whole file is streamed) or a non-empty cut.
    #[test]
    fn the_streaming_line_count_matches_str_lines() {
        let tmp = TempWs::new("count-lines");
        for (name, body) in [
            ("empty.txt", ""),
            ("one.txt", "a"),
            ("one-nl.txt", "a\n"),
            ("two.txt", "a\nb"),
            ("two-nl.txt", "a\nb\n"),
            ("blank.txt", "\n\n\n"),
        ] {
            tmp.write(name, body);
            let mut file = std::fs::File::open(tmp.path.join(name)).unwrap();
            // No prefix: everything comes from the streaming loop.
            let (counted, capped) = continue_counting(&mut file, b"").unwrap();
            assert_eq!(counted, body.lines().count(), "{name} ({body:?})");
            assert!(!capped, "{name}");

            // A prefix that stops mid-file must give the same answer, because the
            // count starts from the newlines already in it.
            let bytes = body.as_bytes();
            for cut in 0..=bytes.len() {
                // `continue_counting` reads from the handle's current offset, which
                // `read_file` has left at the end of the prefix.
                let mut file = std::fs::File::open(tmp.path.join(name)).unwrap();
                use std::io::Seek;
                file.seek(std::io::SeekFrom::Start(cut as u64)).unwrap();
                let (counted, _) = continue_counting(&mut file, &bytes[..cut]).unwrap();
                assert_eq!(
                    counted,
                    body.lines().count(),
                    "{name} ({body:?}) with a {cut}-byte prefix"
                );
            }
        }
    }

    /// A file under the cap must keep the exact header it always had.
    #[test]
    fn a_small_file_still_reports_its_exact_total() {
        let tmp = TempWs::new("small-total");
        tmp.write("a.txt", "alpha\nbeta\ngamma\n");
        let ws = tmp.ws();
        let rendered = read_file(&ws, "a.txt", None, None, 200_000).unwrap();
        assert!(!rendered.meta["byte_limited"].as_bool().unwrap());
        assert_eq!(rendered.meta["lines_total"].as_u64().unwrap(), 3);
        assert!(rendered
            .text
            .starts_with("a.txt — lines 1-3 of 3 (17 bytes)"));
    }

    /// Images are classified by their magic bytes. Trusting the extension would
    /// base64 a file the endpoint rejects (or refuse a real PNG saved without
    /// one), and both failures are silent from the model's side.
    #[test]
    fn images_are_recognised_by_magic_bytes_not_extension() {
        let tmp = TempWs::new("image-probe");
        let png: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        let jpeg: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10];
        let gif: &[u8] = b"GIF89a\x01\x00";
        let webp: &[u8] = b"RIFF\x24\x00\x00\x00WEBPVP8 ";
        let bmp: &[u8] = b"BM\x36\x00\x00\x00\x00\x00\x00\x00";
        let ws = tmp.ws();

        for (name, bytes, media) in [
            ("a.png", png, "image/png"),
            ("no-extension", png, "image/png"),
            ("a.jpg", jpeg, "image/jpeg"),
            ("a.gif", gif, "image/gif"),
            ("a.webp", webp, "image/webp"),
        ] {
            tmp.write_bytes(name, bytes);
            match probe_image(&ws, name).unwrap() {
                ImageProbe::Vision {
                    media_type, size, ..
                } => {
                    assert_eq!(media_type, media, "{name}");
                    assert_eq!(size as usize, bytes.len(), "{name}");
                }
                other => panic!("{name} should be a vision image, got {other:?}"),
            }
        }

        // A format the vision APIs reject is named, not treated as binary noise.
        tmp.write_bytes("scan.bmp", bmp);
        match probe_image(&ws, "scan.bmp").unwrap() {
            ImageProbe::Unsupported { media_type, .. } => assert_eq!(media_type, "image/bmp"),
            other => panic!("bmp should be an unsupported image, got {other:?}"),
        }

        // Text named like an image is still text, and a PNG signature is not a
        // licence to attach a file whose extension lies about being text.
        tmp.write("fake.png", "not an image\n");
        assert_eq!(
            probe_image(&ws, "fake.png").unwrap(),
            ImageProbe::NotAnImage
        );
        tmp.write("notes.txt", "hello\n");
        assert_eq!(
            probe_image(&ws, "notes.txt").unwrap(),
            ImageProbe::NotAnImage
        );
        // A directory is not an image; the text path produces the real error.
        std::fs::create_dir_all(tmp.path.join("dir")).unwrap();
        assert_eq!(probe_image(&ws, "dir").unwrap(), ImageProbe::NotAnImage);
    }

    #[test]
    fn read_image_encodes_the_bytes_and_refuses_over_the_cap() {
        let tmp = TempWs::new("image-read");
        let png: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        tmp.write_bytes("shot.png", png);
        let ws = tmp.ws();

        let image = read_image(&ws, "shot.png", "image/png", 1024).unwrap();
        assert_eq!(image.media_type, "image/png");
        assert_eq!(image.name.as_deref(), Some("shot.png"));
        assert_eq!(image.bytes, png.len() as u64);
        assert!(image.data_url().starts_with("data:image/png;base64,"));
        // Standard base64 of the 12 PNG bytes, decoded back to the same bytes.
        use base64::Engine as _;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&image.data)
            .unwrap();
        assert_eq!(decoded, png);

        let err = read_image(&ws, "shot.png", "image/png", 4).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("max_image_read_bytes"), "{message}");
        assert!(message.contains("shot.png"), "{message}");
    }

    #[test]
    fn dense_replace_all_uses_a_capped_block_preview_not_lcs_state() {
        let tmp = TempWs::new("dense-diff");
        let body = "x\n".repeat(2500);
        tmp.write("dense.txt", &body);
        let plan = plan_edit(&tmp.ws(), "dense.txt", "x", "y", true).unwrap();
        assert_eq!(plan.occurrences, 2500);
        assert_eq!(plan.new_content, "y\n".repeat(2500));
        assert!(
            plan.diff.contains("diff preview truncated"),
            "{}",
            plan.diff
        );
        assert!(
            plan.diff.lines().count() <= DIFF_MAX_LINES + 1,
            "unbounded diff preview: {} lines",
            plan.diff.lines().count()
        );
        // A pure insertion/deletion must never enter the table path, regardless
        // of the other side's length (n*m is zero and used to bypass the guard).
        assert!(!use_lcs(0, usize::MAX));
        assert!(!use_lcs(usize::MAX, 0));
        assert!(!use_lcs(2500, 2500));
        assert!(use_lcs(10, 10));
        let additions = render_diff(&[], &["first", "second"], 80);
        assert!(additions.contains("+first") && additions.contains("+second"));
        assert!(additions.contains("-1,0") && additions.contains("+1,2"));
    }
}
