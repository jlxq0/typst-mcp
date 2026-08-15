//! The parent side: spawn a worker per compile, and outlive whatever it does.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{Semaphore, TryAcquireError};

use crate::protocol::{Job, JobResult, read_frame, write_frame};

/// The argument that puts this binary into worker mode.
pub const WORKER_FLAG: &str = "--compile-worker";

/// How the parent is configured.
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    /// The binary to re-exec. Defaults to this process's own path.
    pub exe: PathBuf,
    /// Wall-clock deadline for one compile.
    pub timeout: Duration,
    /// How many compiles may run at once.
    pub max_concurrent: usize,
    /// How long a request waits for a slot before being turned away.
    pub queue_timeout: Duration,
}

impl SpawnConfig {
    /// Re-exec this process. Correct for the server; wrong inside an integration
    /// test, where `current_exe()` is the test harness — use [`Self::for_exe`] with
    /// `env!("CARGO_BIN_EXE_typst-mcp")` there.
    pub fn new() -> std::io::Result<Self> {
        Ok(Self::for_exe(std::env::current_exe()?))
    }

    /// Spawn a specific binary.
    pub fn for_exe(exe: impl Into<PathBuf>) -> Self {
        Self {
            exe: exe.into(),
            timeout: Duration::from_secs(20),
            max_concurrent: std::thread::available_parallelism().map_or(4, |n| n.get().min(4)),
            queue_timeout: Duration::from_secs(5),
        }
    }
}

/// Why a compile never produced a result frame.
///
/// Distinct from a failed *document*, which comes back as a [`JobResult::Failed`]
/// with diagnostics. This enum is only for the worker itself going wrong.
#[derive(Debug, Error)]
pub enum SpawnError {
    #[error("compile exceeded its {}s deadline and was killed", .after.as_secs())]
    Timeout { after: Duration },
    #[error("too many compiles in flight; try again shortly")]
    Overloaded,
    #[error("compile worker exited unexpectedly ({exit}): {stderr}")]
    Died { exit: String, stderr: String },
    #[error("compile worker I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Runs compiles in short-lived worker processes.
#[derive(Debug, Clone)]
pub struct CompileService {
    config: SpawnConfig,
    permits: Arc<Semaphore>,
}

impl CompileService {
    pub fn new(config: SpawnConfig) -> Self {
        let permits = Arc::new(Semaphore::new(config.max_concurrent));
        Self { config, permits }
    }

    /// Compile one job in a fresh process.
    pub async fn compile(&self, job: &Job) -> Result<JobResult, SpawnError> {
        let _permit = self.acquire().await?;

        let mut child = Command::new(&self.config.exe)
            .arg(WORKER_FLAG)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Belt and braces: if this future is dropped (client hung up, runtime shut
            // down), the child must not outlive it.
            .kill_on_drop(true)
            .spawn()?;

        let mut stdin = child.stdin.take().expect("stdin was piped");
        let mut stdout = child.stdout.take().expect("stdout was piped");
        let mut stderr = child.stderr.take().expect("stderr was piped");

        let mut frame = Vec::new();
        write_frame(&mut frame, job)?;

        // All three pipes must be serviced concurrently. An 8 MiB job does not fit in
        // a pipe buffer, so writing it all before reading would deadlock against a
        // worker that is trying to write output; and a worker that fills stderr while
        // we only read stdout would block forever.
        let pump = async {
            let (write, out, err) = tokio::join!(
                async {
                    stdin.write_all(&frame).await?;
                    stdin.shutdown().await
                },
                async {
                    let mut buf = Vec::new();
                    stdout.read_to_end(&mut buf).await.map(|_| buf)
                },
                async {
                    let mut buf = Vec::new();
                    stderr.read_to_end(&mut buf).await.map(|_| buf)
                },
            );
            // The write error is deliberately *not* propagated here. A worker that dies
            // before draining the job frame gives us EPIPE, and reporting that would
            // discard the stderr explaining why it died — which is the only useful
            // information in that failure. Carry it and let the exit status decide.
            Ok::<_, std::io::Error>((out?, err?, write.err()))
        };

        let deadline = tokio::time::timeout(self.config.timeout, async {
            let io = pump.await?;
            let status = child.wait().await?;
            Ok::<_, std::io::Error>((io, status))
        });

        let ((out, err, write_err), status) = match deadline.await {
            Ok(result) => result?,
            Err(_) => {
                // The only thing that actually stops a runaway compile. A thread
                // spinning in `#while true {}` could never be reclaimed.
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(SpawnError::Timeout {
                    after: self.config.timeout,
                });
            }
        };

        if !status.success() || out.is_empty() {
            return Err(SpawnError::Died {
                exit: describe(&status),
                stderr: String::from_utf8_lossy(&err).trim().to_owned(),
            });
        }

        // The child exited cleanly and produced a frame, so a write error here means we
        // could not deliver the whole job — a real failure, just a rarer one.
        if let Some(err) = write_err {
            return Err(SpawnError::Io(err));
        }

        Ok(read_frame(out.as_slice())?)
    }

    async fn acquire(&self) -> Result<tokio::sync::OwnedSemaphorePermit, SpawnError> {
        match Arc::clone(&self.permits).try_acquire_owned() {
            Ok(permit) => Ok(permit),
            Err(TryAcquireError::NoPermits) => {
                // Wait briefly rather than rejecting on the first collision: bursts are
                // normal, sustained overload is not.
                tokio::time::timeout(
                    self.config.queue_timeout,
                    Arc::clone(&self.permits).acquire_owned(),
                )
                .await
                .map_err(|_| SpawnError::Overloaded)?
                .map_err(|_| SpawnError::Overloaded)
            }
            Err(TryAcquireError::Closed) => Err(SpawnError::Overloaded),
        }
    }
}

fn describe(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit code {code}"),
        None => "killed by signal".to_owned(),
    }
}
