use std::{env, fmt::Display, fs, io, str::FromStr};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to read {env_var} from {path}: {source}")]
    ReadSecretFile {
        env_var: String,
        path: String,
        #[source]
        source: io::Error,
    },

    #[error("{file_env_var} points to an empty file: {path}")]
    EmptySecretFile { file_env_var: String, path: String },

    #[error("Failed to parse {env_var} from {path}: {message}")]
    ParseSecretFile {
        env_var: String,
        path: String,
        message: String,
    },

    #[error("Failed to parse {env_var}: {message}")]
    ParseEnvVar { env_var: String, message: String },
}

pub fn resolve_secret_key<T, E, F>(
    cli_value: Option<T>,
    env_var: &str,
    file_env_var: &str,
    map_err: F,
) -> Result<Option<T>, E>
where
    T: FromStr,
    T::Err: Display,
    F: Fn(ConfigError) -> E,
{
    if cli_value.is_some() {
        return Ok(cli_value);
    }

    if let Some(path) = read_env_value(file_env_var) {
        let key = fs::read_to_string(&path).map_err(|source| {
            map_err(ConfigError::ReadSecretFile {
                env_var: env_var.to_string(),
                path: path.clone(),
                source,
            })
        })?;
        let key = key.trim();
        if key.is_empty() {
            return Err(map_err(ConfigError::EmptySecretFile {
                file_env_var: file_env_var.to_string(),
                path,
            }));
        }

        return key.parse::<T>().map(Some).map_err(|err| {
            map_err(ConfigError::ParseSecretFile {
                env_var: env_var.to_string(),
                path,
                message: err.to_string(),
            })
        });
    }

    read_env_value(env_var)
        .map(|key| {
            key.parse::<T>().map_err(|err| {
                map_err(ConfigError::ParseEnvVar {
                    env_var: env_var.to_string(),
                    message: err.to_string(),
                })
            })
        })
        .transpose()
}

pub fn read_env_value(name: &str) -> Option<String> {
    let value = env::var(name).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        str::FromStr,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct ParsedValue(String);

    impl FromStr for ParsedValue {
        type Err = &'static str;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            if s == "bad" {
                Err("bad value")
            } else {
                Ok(Self(s.to_string()))
            }
        }
    }

    #[test]
    fn test_read_env_value_ignores_blank() {
        let env_name = unique_env_name("TEST_READ_ENV_VALUE");
        let original = env::var(&env_name).ok();

        env::set_var(&env_name, "   ");
        assert_eq!(read_env_value(&env_name), None);

        env::set_var(&env_name, "  value  ");
        assert_eq!(read_env_value(&env_name), Some("value".to_string()));

        restore_env(&env_name, original);
    }

    #[test]
    fn test_resolve_secret_key_prefers_cli_value() {
        let env_name = unique_env_name("TEST_SECRET");
        let file_env_name = unique_env_name("TEST_SECRET_FILE");
        let original = env::var(&env_name).ok();
        let original_file = env::var(&file_env_name).ok();

        env::set_var(&env_name, "env-value");
        env::set_var(&file_env_name, "/tmp/ignored");

        let resolved = resolve_secret_key(
            Some(ParsedValue("cli-value".to_string())),
            &env_name,
            &file_env_name,
            |err| err.to_string(),
        )
        .unwrap();

        assert_eq!(resolved, Some(ParsedValue("cli-value".to_string())));

        restore_env(&env_name, original);
        restore_env(&file_env_name, original_file);
    }

    #[test]
    fn test_resolve_secret_key_from_file_env() {
        let file_path = unique_temp_file_path("common-resolve-secret-key");
        let env_name = unique_env_name("TEST_SECRET");
        let file_env_name = unique_env_name("TEST_SECRET_FILE");
        let original = env::var(&env_name).ok();
        let original_file = env::var(&file_env_name).ok();

        fs::write(&file_path, " file-value \n").unwrap();
        env::remove_var(&env_name);
        env::set_var(&file_env_name, &file_path);

        let resolved = resolve_secret_key(None::<ParsedValue>, &env_name, &file_env_name, |err| {
            err.to_string()
        })
        .unwrap();

        assert_eq!(resolved, Some(ParsedValue("file-value".to_string())));

        restore_env(&env_name, original);
        restore_env(&file_env_name, original_file);
        fs::remove_file(file_path).unwrap();
    }

    #[test]
    fn test_resolve_secret_key_ignores_blank_env() {
        let env_name = unique_env_name("TEST_SECRET");
        let file_env_name = unique_env_name("TEST_SECRET_FILE");
        let original = env::var(&env_name).ok();
        let original_file = env::var(&file_env_name).ok();

        env::set_var(&env_name, "");
        env::set_var(&file_env_name, "   ");

        let resolved = resolve_secret_key(None::<ParsedValue>, &env_name, &file_env_name, |err| {
            err.to_string()
        })
        .unwrap();

        assert_eq!(resolved, None);

        restore_env(&env_name, original);
        restore_env(&file_env_name, original_file);
    }

    #[test]
    fn test_resolve_secret_key_reports_parse_error() {
        let env_name = unique_env_name("TEST_SECRET");
        let file_env_name = unique_env_name("TEST_SECRET_FILE");
        let original = env::var(&env_name).ok();
        let original_file = env::var(&file_env_name).ok();

        env::set_var(&env_name, "bad");
        env::remove_var(&file_env_name);

        let err = resolve_secret_key(None::<ParsedValue>, &env_name, &file_env_name, |err| {
            err.to_string()
        })
        .unwrap_err();

        assert!(err.contains(&format!("Failed to parse {}", env_name)));

        restore_env(&env_name, original);
        restore_env(&file_env_name, original_file);
    }

    fn unique_temp_file_path(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("{prefix}-{nanos}.txt"))
    }

    fn unique_env_name(prefix: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{prefix}_{nanos}")
    }

    fn restore_env(name: &str, value: Option<String>) {
        if let Some(value) = value {
            env::set_var(name, value);
        } else {
            env::remove_var(name);
        }
    }
}
