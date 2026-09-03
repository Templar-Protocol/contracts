use std::{fs, path::{Path, PathBuf}};

use url::Url;
use zeroize::Zeroizing;

use crate::error::{Error, Result};

#[derive(Clone, Debug)]
pub enum SecretProvider {
    Environment(String),
    File(PathBuf),
}

impl SecretProvider {
    pub fn read(&self) -> Result<Zeroizing<String>> {
        let value = match self {
            Self::Environment(name) => std::env::var(name)
                .map_err(|_| Error::InvalidInput(format!("environment variable {name} is unavailable")))?,
            Self::File(path) => read_secret_file(path)?,
        };
        if value.trim().is_empty() {
            return Err(Error::InvalidInput("secret provider returned an empty value".into()));
        }
        Ok(Zeroizing::new(value.trim().to_owned()))
    }

    pub fn label(&self) -> String {
        match self {
            Self::Environment(name) => format!("env:{name}"),
            Self::File(path) => format!("file:{}", path.display()),
        }
    }
}

pub fn public_origin(raw: &str) -> Result<String> {
    let url = Url::parse(raw).map_err(|_| Error::InvalidInput("RPC URL is invalid".into()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::InvalidInput("RPC URL must use http or https".into()));
    }
    let host = url
        .host_str()
        .ok_or_else(|| Error::InvalidInput("RPC URL has no host".into()))?;
    let port = url.port().map(|value| format!(":{value}")).unwrap_or_default();
    Ok(format!("{}://{host}{port}", url.scheme()))
}

fn read_secret_file(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(Error::InvalidInput(format!(
            "secret path must be a regular file: {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::InvalidInput(format!(
                "secret file must not be accessible by group or others: {}",
                path.display()
            )));
        }
    }
    Ok(fs::read_to_string(path)?)
}
