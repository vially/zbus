#[cfg(not(feature = "tokio"))]
use std::sync::Arc;
use std::{ffi::OsString, fmt::Display, os::unix::ffi::OsStrExt, path::PathBuf, process::Stdio};

#[cfg(not(feature = "tokio"))]
use async_io::Async;
#[cfg(not(feature = "tokio"))]
use async_net::unix::UnixStream;
#[cfg(not(feature = "tokio"))]
use futures_util::AsyncReadExt;
#[cfg(feature = "tokio")]
use tracing::warn;

use crate::{process::Command, Executor};

use super::encode_percents;

/// A unixexec domain socket transport in a D-Bus address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnixExec {
    pub(super) path: PathBuf,
    pub(super) arg0: Option<OsString>,
    pub(super) args: Vec<String>,
}

impl UnixExec {
    /// Create a new unixexec transport with the given path and arguments.
    pub fn new(path: PathBuf, arg0: Option<OsString>, args: Vec<String>) -> Self {
        Self { path, arg0, args }
    }

    pub(super) fn from_options(opts: std::collections::HashMap<&str, &str>) -> crate::Result<Self> {
        let Some(path) = opts.get("path") else {
            return Err(crate::Error::Address(
                "unixexec address is missing `path`".to_owned(),
            ));
        };

        let arg0 = opts.get("argv0").map(OsString::from);

        let mut args: Vec<String> = Vec::new();
        let mut arg_index = 1;
        while let Some(arg) = opts.get(format!("argv{arg_index}").as_str()) {
            args.push(arg.to_string());
            arg_index += 1;
        }

        Ok(Self::new(PathBuf::from(path), arg0, args))
    }

    #[cfg(feature = "tokio")]
    pub(super) async fn connect(
        self,
        executor: &Executor<'static>,
    ) -> crate::Result<tokio::net::UnixStream> {
        let mut child = Command::from(self)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;

        let mut exec_stdio_stream = child.join_stdio_stream()?;

        let (transport_stream, mut unix_pipe_stream) = tokio::net::UnixStream::pair()?;

        executor
            .spawn(
                async move {
                    if let Err(e) =
                        tokio::io::copy_bidirectional(&mut unix_pipe_stream, &mut exec_stdio_stream)
                            .await
                    {
                        warn!("Error occurred while copying bidirectional streams: {}", e);
                    }
                },
                "unixexec bidirectional copy",
            )
            .detach();

        Ok(transport_stream)
    }

    #[cfg(not(feature = "tokio"))]
    pub(super) async fn connect(
        self,
        executor: &Executor<'static>,
    ) -> crate::Result<Async<std::os::unix::net::UnixStream>> {
        let mut child = Command::from(self)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;

        let (mut exec_stdin, exec_stdout) = child.take_stdio()?;

        let (transport_stream, unix_pipe_stream) = UnixStream::pair()?;
        let (unix_pipe_reader, mut unix_pipe_writer) = unix_pipe_stream.split();

        executor
            .spawn(
                futures_util::future::join(
                    async move { futures_util::io::copy(unix_pipe_reader, &mut exec_stdin).await },
                    async move { futures_util::io::copy(exec_stdout, &mut unix_pipe_writer).await },
                ),
                "unixexec bidirectional copy",
            )
            .detach();

        Arc::into_inner(transport_stream.into())
            .ok_or(crate::Error::Failure("invalid transport stream".into()))
    }
}

impl Display for UnixExec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("unixexec:")?;
        encode_percents(f, self.path.as_os_str().as_bytes())
    }
}

impl From<UnixExec> for Command {
    fn from(unixexec: UnixExec) -> Self {
        let mut command = Command::new(unixexec.path);
        command.args(unixexec.args);

        if let Some(arg0) = unixexec.arg0.as_ref() {
            command.arg0(arg0);
        }

        command
    }
}
