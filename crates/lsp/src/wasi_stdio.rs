//! Async stdin/stdout for the WASI build, where tokio's `io-std` is not
//! available. A thread does the blocking stdin reads and hands each chunk to
//! the runtime over a channel, so timers (the `did_change` debounce) still
//! fire while the server waits for the next message.

use std::io::{Read, Write};
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;

pub struct Stdin {
    rx: mpsc::Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    pos: usize,
}

pub fn stdin() -> Stdin {
    let (tx, rx) = mpsc::channel(64);
    std::thread::Builder::new()
        .name("wirescript-stdin".into())
        .spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        })
        .expect("failed to spawn stdin reader thread");
    Stdin { rx, chunk: Vec::new(), pos: 0 }
}

impl AsyncRead for Stdin {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, out: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        if self.pos == self.chunk.len() {
            match self.rx.poll_recv(cx) {
                Poll::Pending => return Poll::Pending,
                // EOF: the reader thread exited.
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Ready(Some(chunk)) => {
                    self.chunk = chunk;
                    self.pos = 0;
                }
            }
        }
        let n = out.remaining().min(self.chunk.len() - self.pos);
        out.put_slice(&self.chunk[self.pos..self.pos + n]);
        self.pos += n;
        Poll::Ready(Ok(()))
    }
}

pub struct Stdout;

impl AsyncWrite for Stdout {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Poll::Ready(std::io::stdout().write(buf))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(std::io::stdout().flush())
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.poll_flush(cx)
    }
}
