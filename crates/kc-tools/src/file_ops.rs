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
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const DEFAULT_READ_LINES: usize = 400;
pub const MAX_READ_LINES: usize = 2000;
/// Hard cap for a single `write_file` payload.
pub const MAX_FILE_WRITE_BYTES: usize = 2 * 1024 * 1024;
/// Cap for `edit_file` (the whole file is loaded to compute the diff).
pub const MAX_EDIT_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_LIST_ENTRIES: usize = 500;
/// How many leading bytes are inspected for a NUL byte (binary sniff).
pub const BINARY_SNIFF_BYTES: usize = 8 * 1024;
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
pub fn read_file(
    ws: &Workspace,
    path: &str,
    offset: Option<u64>,
    limit: Option<u64>,
    max_bytes: usize,
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

    let want = (max_bytes as u64).saturating_add(1).min(size);
    let mut buf = Vec::new();
    let mut file = std::fs::File::open(&abs).map_err(|e| Error::io(&abs, e))?;
    let mut limited = std::io::Read::by_ref(&mut file).take(want);
    limited
        .read_to_end(&mut buf)
        .map_err(|e| Error::io(&abs, e))?;
    let byte_limited = buf.len() > max_bytes;
    if byte_limited {
        buf.truncate(max_bytes);
    }

    if let Some(pos) = buf.iter().take(BINARY_SNIFF_BYTES).position(|b| *b == 0) {
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
    let total = if byte_limited {
        match count_lines_streaming(&abs) {
            Ok(n) => n as usize,
            // The count is a nicety; never fail a readable file over it.
            Err(_) => available,
        }
    } else {
        available
    };

    let offset = offset.unwrap_or(1).max(1) as usize;
    let limit = limit
        .unwrap_or(DEFAULT_READ_LINES as u64)
        .clamp(1, MAX_READ_LINES as u64) as usize;
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
    out.push_str(&format!(
        "{rel} — lines {first}-{last} of {total} ({size} bytes)"
    ));

    // Explain the (rare) byte cap before the content so it survives output
    // truncation of a huge file.
    if byte_limited {
        out.push_str(&format!(
            "\n[note: only the first {max_bytes} of {size} bytes were read (limits.max_file_read_bytes); \
             raise the limit to see the rest]"
        ));
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
            "lines_from": first,
            "lines_to": last,
            "bytes": size,
            "byte_limited": byte_limited,
            "offset": offset,
            "limit": limit,
        }),
    })
}

/// Count the lines in a file with a streaming pass, so a file far larger than
/// `max_file_read_bytes` can still be described accurately without loading it.
///
/// Matches `str::lines` semantics: a trailing newline does not start a new line,
/// and an empty file has zero lines.
fn count_lines_streaming(abs: &Path) -> std::io::Result<u64> {
    let mut file = std::fs::File::open(abs)?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut newlines = 0u64;
    let mut last_byte = None;
    loop {
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        newlines += buf[..n].iter().filter(|b| **b == b'\n').count() as u64;
        last_byte = Some(buf[n - 1]);
    }
    // A file that does not end in a newline has one more (partial) line.
    if last_byte.is_some() && last_byte != Some(b'\n') {
        newlines += 1;
    }
    Ok(newlines)
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
    let bytes = std::fs::read(&abs).map_err(|e| Error::io(&abs, e))?;
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
    let mut items: Vec<Item> = Vec::new();
    let mut skipped_errors = 0usize;
    for entry in walker {
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

    items.sort_by(|a, b| (!a.is_dir, &a.rel).cmp(&(!b.is_dir, &b.rel)));
    let total = items.len();
    let truncated = total > max_entries;
    items.truncate(max_entries);

    let mut out = format!(
        "Listing {rel} (depth {depth}, {total} entr{}, {} shown)",
        if total == 1 { "y" } else { "ies" },
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
            "\n[truncated: showing the first {max_entries} of {total} entries; \
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
            "entries": total,
            "shown": items.len(),
            "errors": skipped_errors,
        }),
    })
}

fn is_skipped_dir(name: &std::ffi::OsStr) -> bool {
    name == ".git" || name == "node_modules"
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
pub struct EditPlan {
    pub abs: PathBuf,
    pub rel: String,
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

    let bytes = std::fs::read(&abs).map_err(|e| Error::io(&abs, e))?;
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

    let indices: Vec<usize> = content.match_indices(old).map(|(i, _)| i).collect();
    if indices.is_empty() {
        return Err(Error::Tool(format!(
            "old_string not found in `{rel}`. First 200 chars of old_string: {:?}. \
             Re-read the file and copy the exact text, including whitespace and line endings.",
            preview(old, 200)
        )));
    }
    let line_numbers: Vec<usize> = indices
        .iter()
        .map(|i| content[..*i].bytes().filter(|b| *b == b'\n').count() + 1)
        .collect();
    if indices.len() > 1 && !replace_all {
        let listed: Vec<String> = line_numbers
            .iter()
            .take(10)
            .map(|n| n.to_string())
            .collect();
        return Err(Error::Tool(format!(
            "old_string matches {} places in `{rel}` (lines {}). Add surrounding context to make it unique, \
             or pass replace_all: true.",
            indices.len(),
            listed.join(", ")
        )));
    }

    let mut new_content = String::with_capacity(content.len());
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
        new_content,
        occurrences: indices.len(),
        line_numbers,
        diff,
    })
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

fn diff_ops<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<(DiffOp, &'a str)> {
    let (n, m) = (old.len(), new.len());
    if n.saturating_mul(m) > DIFF_MAX_CELLS {
        // Too large for a table: emit a block replacement.
        let mut ops = Vec::with_capacity(n + m);
        ops.extend(old.iter().map(|l| (DiffOp::Del, *l)));
        ops.extend(new.iter().map(|l| (DiffOp::Ins, *l)));
        return ops;
    }
    let width = m + 1;
    let mut table = vec![0u32; (n + 1) * width];
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

    /// The streaming counter must agree with `str::lines` on the awkward shapes.
    #[test]
    fn the_streaming_line_count_matches_str_lines() {
        let tmp = TempWs::new("count-lines");
        let ws = tmp.ws();
        for (name, body) in [
            ("empty.txt", ""),
            ("one.txt", "a"),
            ("one-nl.txt", "a\n"),
            ("two.txt", "a\nb"),
            ("two-nl.txt", "a\nb\n"),
            ("blank.txt", "\n\n\n"),
        ] {
            tmp.write(name, body);
            let abs = ws.resolve_existing(name).unwrap();
            assert_eq!(
                count_lines_streaming(&abs).unwrap(),
                body.lines().count() as u64,
                "{name} ({body:?})"
            );
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
}
