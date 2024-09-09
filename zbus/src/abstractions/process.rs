use std::{ffi::OsStr, io::Error, process::Output};

/// A wrapper around the command API of the underlying async runtime.
pub struct Command(
    #[cfg(not(feature = "tokio"))] async_process::Command,
    #[cfg(feature = "tokio")] tokio::process::Command,
);

impl Command {
    /// Constructs a new `Command` for launching the program at path `program`.
    pub fn new<S>(program: S) -> Self
    where
        S: AsRef<OsStr>,
    {
        #[cfg(not(feature = "tokio"))]
        return Self(async_process::Command::new(program));

        #[cfg(feature = "tokio")]
        return Self(tokio::process::Command::new(program));
    }

    /// Adds multiple arguments to pass to the program.
    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.0.args(args);
        self
    }

    /// Executes the command as a child process, waiting for it to finish and
    /// collecting all of its output.
    pub async fn output(&mut self) -> Result<Output, Error> {
        self.0.output().await
    }
}

/// An asynchronous wrapper around running and getting command output
pub async fn run<I, S>(program: S, args: I) -> Result<Output, Error>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(program).args(args).output().await
}
