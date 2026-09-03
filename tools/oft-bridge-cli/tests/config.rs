use std::fs;

use templar_oft_bridge_cli::config::{public_origin, SecretProvider};

#[test]
fn rpc_file_provider_reads_mode_0600_and_strips_secret_origin_parts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rpc");
    fs::write(
        &path,
        "https://user:secret@example.test:8443/private?api_key=secret\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    let value = SecretProvider::File(path).read().unwrap();
    assert_eq!(public_origin(&value).unwrap(), "https://example.test:8443");
}

#[cfg(unix)]
#[test]
fn rpc_file_provider_rejects_insecure_mode_and_symlink() {
    use std::os::unix::fs::{symlink, PermissionsExt as _};

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rpc");
    fs::write(&path, "https://example.test").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(SecretProvider::File(path.clone()).read().is_err());

    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let link = directory.path().join("rpc-link");
    symlink(&path, &link).unwrap();
    assert!(SecretProvider::File(link).read().is_err());
}
