use std::{
    borrow::BorrowMut, ffi::OsString, fmt::Display, os::unix::ffi::OsStrExt, path::PathBuf,
    process::Stdio,
};

use crate::process::Command;

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

    pub(super) async fn connect(self) -> crate::Result<crate::connection::socket::Command> {
        Command::from(self)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?
            .borrow_mut()
            .try_into()
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
