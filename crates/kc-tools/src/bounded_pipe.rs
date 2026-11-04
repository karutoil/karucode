//! Drain a subprocess pipe without retaining unbounded output.
use tokio::io::{AsyncRead, AsyncReadExt};

pub(crate) async fn read<R: AsyncRead + Unpin>(
    mut pipe: R,
    limit: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        let n = pipe.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        let keep = n.min(limit.saturating_sub(output.len()));
        output.extend_from_slice(&chunk[..keep]);
        truncated |= keep < n;
    }
    Ok((output, truncated))
}
