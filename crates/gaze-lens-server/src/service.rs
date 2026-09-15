//! One authenticated v2 exchange per TLS connection. Readiness only in this slice.
use crate::{
    auth::{Authority, Pinned},
    config::{Checked, bounded_file},
    history::History,
};
use gaze_lens_protocol::{
    Error, Result,
    bounds::{self, FrameBuffer},
    wire::{
        self, Args, Failure, Operation, Prepared, Privacy, ReadinessStatus, ResultBody, Success,
        Version,
    },
};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
    sync::Semaphore,
    task::JoinSet,
};

pub async fn run(path: &Path) -> Result<()> {
    let config = crate::config::check(path).await?;
    let listener = TcpListener::bind(config.listen)
        .await
        .map_err(|_| Error::Unavailable)?;
    serve(listener, config).await
}
pub async fn serve(listener: TcpListener, config: Checked) -> Result<()> {
    let authority = Authority::parse(&bounded_file(&config.authority).await?)?;
    let history_path = config.history.clone();
    let history = Arc::new(
        tokio::task::spawn_blocking(move || History::open(&history_path, &authority))
            .await
            .map_err(|_| Error::InternalFailure)??,
    );
    let config = Arc::new(config);
    let permits = Arc::new(Semaphore::new(bounds::DEFAULT_ACTIVE_CALLS));
    let principals = Arc::new(Mutex::new(HashMap::new()));
    let mut tasks = JoinSet::new();
    loop {
        while tasks.try_join_next().is_some() {}
        tokio::select! {
            result = listener.accept() => {
                let (socket,_) = result.map_err(|_| Error::Unavailable)?;
                let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else { drop(socket); continue; };
                let config = Arc::clone(&config);
                let principals = Arc::clone(&principals);
                let history = Arc::clone(&history);
                tasks.spawn(async move {
                    let _permit = permit;
                    let _ = tokio::time::timeout(
                        Duration::from_secs(bounds::CALL_SECONDS),
                        connection(socket, config, principals, history),
                    ).await;
                });
            }
            _ = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
}
async fn connection(
    socket: tokio::net::TcpStream,
    config: Arc<Checked>,
    principals: Arc<Mutex<HashMap<String, usize>>>,
    history: Arc<History>,
) -> Result<()> {
    let mut io = tokio::time::timeout(
        Duration::from_secs(bounds::IO_SECONDS),
        config.acceptor.accept(socket),
    )
    .await
    .map_err(|_| Error::Timeout)?
    .map_err(|_| Error::Unauthorized)?;
    let p = wire::decode_prepare(&read(&mut io, bounds::PREPARE_BYTES).await?)?;
    let pin = authority(&config.authority, &history)
        .await?
        .prepare(&p, now()?)?;
    // No executable values are requested for unimplemented operations.
    let admission = if p.operation == Operation::Readiness {
        PrincipalPermit::acquire(principals, pin.binding().principal.clone())
    } else {
        Err(Error::UnsupportedOperation)
    };
    let _principal = match admission {
        Ok(permit) => permit,
        Err(code) => {
            return finish(
                &mut io,
                &wire::encode_failure(&Failure {
                    version: Version::V2,
                    id: p.id,
                    code,
                })?,
            )
            .await;
        }
    };
    let prepared = Prepared {
        version: Version::V2,
        privacy: Privacy::ClientGaze,
        id: p.id.clone(),
        operation: p.operation,
        binding: pin.binding().clone(),
    };
    write(&mut io, &wire::encode_prepared(&prepared)?).await?;
    let outcome = call(&mut io, &config.authority, &pin, &p.id, &history).await;
    let bytes = match outcome {
        Ok(bytes) => bytes,
        Err(code) => wire::encode_failure(&Failure {
            version: Version::V2,
            id: p.id,
            code,
        })?,
    };
    finish(&mut io, &bytes).await
}
async fn finish<S: AsyncWrite + Unpin>(io: &mut S, bytes: &[u8]) -> Result<()> {
    write(io, bytes).await?;
    tokio::time::timeout(Duration::from_secs(bounds::IO_SECONDS), io.shutdown())
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|_| Error::Unavailable)
}
async fn call<S: AsyncRead + Unpin>(
    io: &mut S,
    path: &Path,
    pin: &Pinned,
    id: &str,
    history: &History,
) -> Result<Vec<u8>> {
    let call = wire::decode_call(&read(io, bounds::FRAME_BYTES).await?)?;
    if call.id != id || !matches!(call.args, Args::Readiness(_)) {
        return Err(Error::InvalidRequest);
    }
    if &call.binding != pin.binding() {
        return Err(Error::BindingChanged);
    }
    authority(path, history).await?.revalidate(pin, now()?)?;
    let success = Success {
        id: call.id.clone(),
        operation: Operation::Readiness,
        binding: pin.binding().clone(),
        result: ResultBody::Readiness {
            status: ReadinessStatus::Configured,
        },
    };
    let bytes = wire::encode_success(&success, &call)?;
    // Last authorization check is the release linearization point.
    authority(path, history).await?.revalidate(pin, now()?)?;
    Ok(bytes)
}
async fn authority(path: &Path, history: &History) -> Result<Authority> {
    let authority =
        Authority::parse(&bounded_file(path).await?).map_err(|_| Error::Unauthorized)?;
    history.validate(&authority)?;
    Ok(authority)
}
fn now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|x| x.as_secs())
        .map_err(|_| Error::Unavailable)
}
async fn read<S: AsyncRead + Unpin>(io: &mut S, max: usize) -> Result<Vec<u8>> {
    tokio::time::timeout(Duration::from_secs(bounds::IO_SECONDS), async {
        let mut buffer = FrameBuffer::new(max)?;
        let mut chunk = [0u8; 1024];
        loop {
            let n = io.read(&mut chunk).await.map_err(|_| Error::Unavailable)?;
            if n == 0 {
                return Err(Error::InvalidRequest);
            }
            if buffer.push(&chunk[..n])? {
                return buffer.finish();
            }
        }
    })
    .await
    .map_err(|_| Error::Timeout)?
}
async fn write<S: AsyncWrite + Unpin>(io: &mut S, bytes: &[u8]) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(bounds::IO_SECONDS), async {
        io.write_all(bytes).await.map_err(|_| Error::Unavailable)?;
        io.flush().await.map_err(|_| Error::Unavailable)
    })
    .await
    .map_err(|_| Error::Timeout)?
}
struct PrincipalPermit {
    active: Arc<Mutex<HashMap<String, usize>>>,
    id: String,
}
impl PrincipalPermit {
    fn acquire(active: Arc<Mutex<HashMap<String, usize>>>, id: String) -> Result<Self> {
        {
            let mut counts = active.lock().map_err(|_| Error::InternalFailure)?;
            let count = counts.entry(id.clone()).or_default();
            if *count >= bounds::MAX_PRINCIPAL_CALLS {
                return Err(Error::Unavailable);
            }
            *count += 1;
        }
        Ok(Self { active, id })
    }
}
impl Drop for PrincipalPermit {
    fn drop(&mut self) {
        if let Ok(mut counts) = self.active.lock()
            && let Some(count) = counts.get_mut(&self.id)
        {
            *count -= 1;
            if *count == 0 {
                counts.remove(&self.id);
            }
        }
    }
}
